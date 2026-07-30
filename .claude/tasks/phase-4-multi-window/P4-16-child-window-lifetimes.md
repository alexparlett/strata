# P4-16 · Child-window lifetimes across an engine restart

**Phase:** 4 · **Status:** ✅ · **DEV_TASKS:** — (defect follow-up) · **Depends on:** P4-10, P4-11

## Goal
A child window that holds handles from the project subtree must not outlive that subtree. The
re-root case was closed for Configure; the **engine restart** case was closed for neither, and the
re-root case was open for Export.

## The defect
`ProjectRoot::render_key` is `(root, generation)`
([project.rs](crates/strata-freya/src/apps/project/project.rs)). `generation` comes from
`EngineRestart` — a `State<u64>` owned by **`ProjectApp`**, the window layer, deliberately placed
above the subtree so it survives the remount it causes
([state/engine_config.rs](crates/strata-freya/src/apps/project/state/engine_config.rs)). Changing
any `datafusion.runtime.*` property in Settings ▸ Engine makes `use_engine_config`'s effect call
`restart.restart()`, which bumps it, which changes the diff key, which drops the whole
`ProjectRoot` scope and mounts a fresh one — the same machinery a re-root uses.

That frees everything created in the scope: `use_init_project`'s
`RadioStation<ProjectState, ProjChan>`, `use_init_log`'s `LogCtx`, `use_init_catalog`'s `Catalog`
and `use_init_catalog_rescan`'s `CatalogRescan`. All `GenerationalBox`-backed, so the storage is
reclaimed and reused.

**The Configure window holds all four**, carried as launch values because it is its own OS window
and cannot inherit the project window's context. **The Export window holds one** (`LogCtx`), so it
has the same exposure, more narrowly.

`use_configure_pin` closed the window when its owner left the registry *or when the owner's project
changed* — which covers a re-root, because that changes the folder. An engine restart changes
neither the window id nor the folder, only the generation. `use_export_pin` checked only that its
owner was still open, so Export was exposed to **both**. Nothing told either window, and its
handles were now dangling: the next read panics on a reclaimed box (`Footer`'s `catalog.read()`,
`Hive`'s `project.peek()`, `use_watch_registration`'s subscription — whichever repaints first; a
keystroke is enough). Press Save before that and it writes into a store nothing is left to serve,
and bumps a scan counter with no driver mounted.

Reachable, if not common: Settings is one ⌘, away and stays open alongside a Configure window.

## As built
The rule is **one predicate over one value**, in a new
[platform/owner.rs](crates/strata-freya/src/platform/owner.rs) — because the thing a child window
must be bound to is not a *window*, it is a **mount of `ProjectRoot`**, and that is exactly the
subtree's own diff key:

```rust
pub struct Subtree {
    pub project: String,        // half of the diff key — what a re-root changes
    pub generation: u64,        // the other half — what a restart changes
    pub restart: EngineRestart, // the live handle, for reading the current generation back
}
```

- `ProjectRoot` **provides** one (built from `self.root` + `self.generation`, so a value is only
  ever true of the mount that built it), before it stands up any of the handles a child borrows.
  So a new child window cannot assemble a mismatched trio, and the two openers — `ConfigureLauncher`
  and the results pane — just `use_consume::<Subtree>()` and put it in the launch struct.
- `use_owner_pin(app, owner, subtree)` is the whole rule and replaces both `use_configure_pin` and
  `use_export_pin`. It compares what the owner window **shows now** (from the registry) against
  `subtree.project`, and the **live** generation against `subtree.generation`. An owner that has
  closed shows nothing, so it fails the same comparison — "my owner closed" needs no clause of its
  own.
- **`EngineRestart` is the one handle here that is safe to hold**, for exactly the reason it exists:
  it is owned by `ProjectApp`, above the subtree, so it outlives every remount it triggers. Made
  `pub` from `apps::project` with that reasoning on the re-export.
- `open_export` now takes the whole `ExportLaunch`, like `open_configure` takes `ConfigureLaunch`
  — the five-argument form was already at its limit and the subtree would have made it six.

Two registry fields came **out** in the process, both because the pin was their only reader and a
second copy of a fact is a fact that can go stale:

- `WindowKind::Configure` loses `project`. The focus-if-open check keys on `owner` + `target`, and
  one owner window shows one project, so the owner is what says which; a window whose owner has
  re-rooted is being closed in the same reactive batch and is filtered out by the existing
  dangling-entry guard (`ctx.windows().contains_key`).
- `WindowKind::Export { owner }` becomes the unit variant `WindowKind::Export`. Export has no
  focus-if-open rule, so once the pin reads its owner from the launch value nothing read that field
  at all. What the registry still needs from the entry is `is_workspace()`.

**Cross-window reactivity is by construction, not by luck.** A subscriber's `ReactiveContext` holds
its *own* window's wake channel (`Message::MarkScopeAsDirty` over that window's sender —
`freya-core/src/reactive_context.rs`), so a write in the project window wakes the child window's
effect regardless of which scope owns the value. That is the same mechanism P4-11's
`use_watch_registration` already relies on.

## Acceptance
- [x] Build + `cargo test --workspace --locked` green (763 tests, `schema_in_sync` included); no
      new warnings.
- [x] With a Configure window open, change a `datafusion.runtime.*` property in Settings ▸ Engine:
      the Configure window closes rather than panicking on the next repaint or keystroke.
- [x] The same for an Export window open across a restart.
- [x] A re-root still closes both — and now closes Export, which it did not before.

The last three are behavioural (a native Skia window, no automation) and were verified by hand:
`cargo run -- sample`, open a table's Configure (or run a query and press Download), then ⌘, →
Engine, set `datafusion.runtime.memory_limit`, Apply. Worth recording for whoever re-runs these —
the restart **confirm only appears if a query is actually running**; with an idle engine Apply
restarts straight away, so the absence of a dialog is not the absence of a restart.

## Notes
The general rule this is an instance of: **a window's lifetime must be at least as short as the
lifetime of the shortest-lived thing it holds.** The Configure window held four
`ProjectRoot`-scoped values and was tied to a window id, which outlives them twice over. Anything
that later hands a child window a subtree handle takes a `Subtree` and calls `use_owner_pin`,
rather than growing a third copy of the predicate.

**What a closed child window costs, and why it is still right.** A restart or a re-root aborts
whatever is in flight, which is why both already go through the one T2 confirm — and a running
export *is* in-flight work (`Engine::publish_inflight` counts `lc.exports > 0`), so the user is
asked before an Export window is taken with its `COPY`. A Configure window mid-`Registering` is the
one gap: `Engine::register` is not counted as in-flight, so a restart can close a window whose form
was waiting on a pass, without asking. That is pre-existing behaviour for the re-root case (P4-11
already closed Configure there) and this change extends it consistently rather than making it
worse; the alternative on the table was a panic. If it turns out to matter, the fix belongs with the
confirm's predicate — one more thing `is_running` counts — not with a second lifetime rule here.

**Left open, deliberately: the pin overrides the Export window's own "not while writing" rule.**
That window refuses Esc while `Status::Writing`, on the stated ground that "the file is half-written
and the window is the only thing that will report how it ends" — and `use_owner_pin` closes it
anyway, so a restart confirmed mid-`COPY` kills the write (the footer's task and the last
`Arc<Engine>` clone both go with the scope) and leaves a partial file with nothing on screen to say
so. Before P4-16 a re-root left the window open and the write finished — and then panicked on the
dangling `log_event`, so this is a worse *file* outcome and a better *process* one. Not fixed here,
for two reasons. The user has already consented: an export in flight makes `guard.running` true, so
the T2 confirm asked before the restart. And the honest fix is not a lifetime rule but P4-10's
choice to record the outcome in the **opener's** log — the export window's one subtree-scoped handle,
and the only reason it has to close at all. A pin that declined to close while writing would just
re-open the dangling read it exists to prevent. Revisit with P4-10 (a write that outlives its
window needs somewhere to report that is not the opener's log), or with the confirm's wording.
