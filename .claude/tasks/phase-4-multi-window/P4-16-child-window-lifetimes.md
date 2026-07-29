# P4-16 · Child-window lifetimes across an engine restart

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** — (defect follow-up) · **Depends on:** P4-10, P4-11

## Goal
A child window that holds handles from the project subtree must not outlive that subtree. The
re-root case is closed; the **engine restart** case is not.

## The defect
`ProjectRoot::render_key` is `(root, generation)`
([project.rs:496](crates/strata-freya/src/apps/project/project.rs:496)). `generation` comes from
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

`use_configure_pin` closes the window when its owner leaves the registry *or when the owner's
project changes* — which covers a re-root, because that changes the folder. An engine restart
changes neither the window id nor the folder, only the generation. Nothing tells the child window,
and its handles are now dangling: the next read panics on a reclaimed box (`Footer`'s
`catalog.read()`, `Hive`'s `project.peek()`, `use_watch_registration`'s subscription — whichever
repaints first; a keystroke is enough). Press Save before that and it writes into a store nothing
is left to serve, and bumps a scan counter with no driver mounted.

Reachable, if not common: Settings is one ⌘, away and stays open alongside a Configure window.

## Build
Carry `EngineRestart` as a launch value and close the child window when the generation it opened
at no longer matches. It is **safe to hold** for exactly the reason it exists — it is owned by
`ProjectApp`, above the subtree, so it outlives every remount it triggers.

- `ConfigureLaunch` / `ConfigureApp` gain the handle and the generation observed at open;
  `WindowKind::Configure` gains the generation beside the `project` it already carries.
- The check goes in `use_configure_pin`'s existing effect, beside the project-path comparison it
  is exactly symmetric with — one predicate, one place, no new machinery.
- Do the same for **Export** (`platform/export.rs` / `use_export_pin`). Its exposure is one handle
  rather than four, but the shape is identical and a second copy of the rule is how the two drift.
  If both windows end up with the same three-part check, that is the argument for one
  `use_owner_pin` helper — the two pins are already near-verbatim copies of each other.

## Acceptance
- [ ] With a Configure window open, change a `datafusion.runtime.*` property in Settings ▸ Engine:
      the Configure window closes rather than panicking on the next repaint or keystroke.
- [ ] The same for an Export window open across a restart.
- [ ] A re-root still closes both (no regression on the case that is already covered).

## Notes
The general rule this is an instance of: **a window's lifetime must be at least as short as the
lifetime of the shortest-lived thing it holds.** The Configure window holds four
`ProjectRoot`-scoped values and was tied to a window id, which outlives them twice over — first
across a re-root, now across a restart. Anything that later hands a child window a subtree handle
inherits this and should be routed through whatever `use_owner_pin` ends up being, rather than
growing a third copy of the predicate.
