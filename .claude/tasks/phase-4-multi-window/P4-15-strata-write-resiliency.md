# P4-15 · `.strata` write resiliency — one funnel, nothing silent

**Phase:** 4 · **Status:** ✅ *(item 8 — one wording pass shared with P4-01 item 5 — waits on that task)* · **DEV_TASKS:** project lifecycle · **Depends on:** P4-13, P4-14, P3-13

> **🟡 Landed (2026-07-30): nothing is silent any more.** The funnel moved to
> `state::persist` and grew to cover all three families; the four writers that reported through
> `tracing` alone now report like the rest, and the app-config write can finally know it failed.
>
> - **`state/persist.rs`** — `persisted(log, ProjectFile, write) -> bool`, with the write passed
>   in because the families spell it differently (a store projection, a snapshot, an append, a
>   rewrite) while the reporting is identical. `ProjectFile` is the **only** place a writer's
>   wording lives, so the terminal tag and the user's sentence can't drift (item 7). Three
>   conveniences over it — `persisted_defs` / `persisted_session` / `persisted_history` — and only
>   the first leaves `state/`, because the session and history writers live in there beside their
>   stores.
> - **The four bare sites** now report: the saved-query rename
>   (`sidebar/catalog/menu.rs`), the debounced session autosave and the final save on close /
>   re-root (`state/hooks.rs`), and the history append (`state/history.rs`).
> - **`strata_core::config::save` returns its `Result`** (item 2) and `write_config` answers
>   `bool`. `SettingsCtx::apply` is the one caller that acts on it, and the acting is the
>   interesting half: a failed Apply **leaves the window open** with the reason in the footer's
>   existing error strip, because the commit still reached every live window — closing would look
>   exactly like success, and the setting would be gone at the next launch with nothing said.
> - **Item 6 settled, and narrowly.** Only Settings Apply reports. The other eight `write_config`
>   callers are bookkeeping the user never asked for (a recent pushed, the open-set updated, a
>   dead recent pruned) with nothing to undo — and nine call sites each announcing the same
>   failure of the same file is the stacked near-duplicate AGENTS.md §3 rules out. Making
>   *those* visible is one standing condition, which is item 3 and is not built.
> - **Item 5 settled: neither direct reporter joins the funnel.** The export writes where the
>   user pointed a file dialog, so "the project is behind" is not what its failure means. The
>   history **Clear** is a `.strata` write, but it *removes* a file — the funnel's sentence is
>   "Could not write the …", and making Clear say that to share a helper would be trading an
>   accurate message for a shared one. Both keep their own `log_event`.
> - **Tests** (`state::persist::tests`): session, history-append and history-rewrite failures each
>   assert the event and the `false`, plus one that a write which *lands* records nothing. The
>   `probe` helper is worth knowing about — a `LogCtx` is a `State` and can only be created inside
>   a Freya scope, so the runner's setup hook hosts the whole write and nothing has to render.
>
> **Not covered by a test: the config family.** Forcing `config::save` to fail means redirecting
> the OS config dir, which needs a production seam this doesn't otherwise want (the same reason
> P4-13 left "Remember" untested — `write_config` funnels to the developer's real settings file).

## Goal
Make a **failed write** to the user's project a stated fact rather than a `tracing` line nobody
sees. Every `.strata` writer goes through one funnel, the failure is visible in the app while it
lasts, and the destructive case has a decided answer rather than an accidental one.

## Why this is its own task, and why it is phase 4

**P4-01 item 5 owns the read side**, and it is built: a defs or session file that won't *load*
shows the fault dialog (`ProjectLoadFailed`) and closes the window. This is the same problem from
the other end, and nobody owns it:

| | Read failure (at open) | Write failure (mid-session) |
|---|---|---|
| State | the window can't exist | the window is fine; the durable copy is behind |
| Recoverable | no | **yes** — fix the permission, reconnect the volume, free the disk |
| Decision | one: close the window | a per-mutation policy |
| Owner | P4-01 item 5 | **this task** |

And it belonged *early* in the remaining phase-4 work, because two phase-4 tasks add **new**
def-mutation sites — **P4-11** (the Configure-table window, which registers, edits and renames
table defs) and **P4-10** (export, which writes files of its own) — and every existing site's local idiom is
`if let Err(e) = … { tracing::error!(…) }`. Decide the policy before the writers land, or each one
copies the silence.

**Both landed first, and neither copied the silence** — which changes what this task is walking
into. P4-11 routed through P3-13's `persisted` and gated its own success on the answer; P4-10 and
P3-14's history **Clear** each call `log_event` directly. So the reporting *shape* is already
settled by practice at five sites and the remaining question is narrower than "decide a policy":
it is which of the three existing idioms — the funnel, a direct `log_event`, a bare `tracing` —
each writer should end on, and why the two non-funnel ones are or aren't exceptions. See the
table.

Not design polish, so **not** phase 5: P5 is tokens, interaction states, animation, theme dial-in
and the drift audit.

## Every writer, and where it ended (re-verified 2026-07-30 — line numbers move, so trust the symbol names)

`write_atomic` already guarantees the good half: a failed write leaves the previous file **intact**
and strands no temp, so a failure is never "your catalog file is corrupt" — always "your catalog
file is one revision behind the screen". What was missing was anyone saying so.

| Writer | Path | On failure |
|---|---|---|
| `save_view` / `save_query` | `project.json` | ✅ `persisted_defs` — logs `Could not write the project file: <e>` and gates the success event (P3-13, funnel moved here by P4-15) |
| `drop_row` (3 arms) | `project.json` | ✅ same funnel; the drop event is logged only if the write landed (P3-13) |
| Configure-table **register / edit / rename** | `project.json` | ✅ same funnel — `apps/configure/views/footer.rs`. The one caller that does something *with* the `false`: it still asks for the registration pass (else the row spins forever) but sets `Status::Failed` rather than closing as though it saved (P4-11) |
| History **Clear** | `history.jsonl` | ⚠️ **reports, deliberately not through the funnel** — `state/history.rs` calls `log_event` directly with `Could not clear the query history: <e>` (P3-14). It *removes* a file, so the funnel's "Could not write the …" would be less accurate, not more consistent. Nothing to gate either: the satellite is emptied before the file is touched |
| Export | user's chosen path | ⚠️ same, and it stays that way — `apps/export/views/footer.rs` logs both arms directly (P4-10), skipping `stopped_on_purpose` settles. It writes where the user pointed a dialog, so "the project is behind" is not what its failure means (item 5) |
| Saved-query **rename** | `project.json` | ✅ `persisted_defs`. It had been `persisted`'s body **minus the `log_event`** — the funnel was written and this site was never switched to it |
| Session **autosave** (debounced) | `session.json` | ✅ `persisted_session`; a `false` also declines to record the snapshot as written, so the next change retries rather than believing the file current |
| Session **final save** on close / re-root | `session.json` | ✅ same — but see the caveat on `persisted_session`: on a *close* the event lands in a log about to be dropped with its window, so making this one genuinely visible still wants item 3 |
| History **append** | `history.jsonl` | ✅ `persisted_history` (both the append and the rewrite arm) |
| App config (settings · recents · open-set) | OS config dir | ✅ `save` returns its `Result`, `write_config` answers `bool`. **Settings Apply** reports and stays open; the eight bookkeeping writes deliberately don't — see the landed note, item 6 |

**There were four bare sites, and the two adjacent pairs above were the argument for build item 1
on their own.** `rename_saved_query` is the funnel's own body with the reporting line
absent, and the history append sits beside a Clear that reports; in both cases the writer that
missed out is the *older* one, left behind when the newer one was written. A helper only stays
adopted if it is somewhere every mutation site already looks, which `views/workbench/editor/`
is not.

What a silent failure costs, per mutation (worked through in the P3-13 session):

- **Save query** — lives in the store and the sidebar; nothing durable. On reopen the row is gone
  and the tab is bound to a `SavedQuery(Uuid)` the project no longer knows (its text is safe in
  `session.json`; the next successful ⌘S re-creates the row).
- **Save view** — the worst half-done: `CREATE OR REPLACE VIEW` **succeeded**, so the view is live
  and queryable all session; only the def is missing. Reopen loses it.
- **Drop** — the def is out of the store and, for a table, the engine has already deregistered,
  but `project.json` still lists it. **The drop silently reverts on reopen** — a destructive action
  the user deliberately confirmed.
- **Settings** — the preference reverts at the next launch with nothing said.

## Build

1. ✅ **One funnel, in `state/`.** Generalise P3-13's `persisted(&project, log) -> bool` into the
   place the stores live (e.g. `state::persist`), and route **every** writer above through it.
   It is currently in `views/workbench/editor/actions.rs`, which is the wrong home for something
   the drop confirm and the Configure window both already import across app boundaries — that was
   expedient in P3-13 and is this task's to fix. No write site may keep a bare `tracing::error!`.

   Note the shape it has to generalise *to*, which `persisted` doesn't have today: it takes
   `&ProjectState` and calls `save_defs` itself, so it is a defs-only helper, and three of the
   four bare sites write `session.json` / `history.jsonl` instead. The reusable part is
   "attempt → on `Err`, `tracing` + `log_event` → hand back whether it landed", with the write
   passed in. Widening it that way is what lets the session and history writers join at all.
2. ✅ **Make the config write reportable.** `strata_core::config::save` must return its `Result` and
   `write_config` must act on it. A sole-write-path funnel that cannot fail-report is a hole in
   the invariant, not a simplification.
3. ✅ **A standing condition, not just an event.** A failed write is a *state* (the file is behind
   and stays behind), so a log row alone under-reports it: the row scrolls away while the
   condition holds. One persistent indication, cleared by the next successful write; the Events
   row stays as the record of when it happened, and nothing stacks a second message restating it
   (AGENTS.md §3).

   **Built as the Problems drawer's `Project` scope, not the status bar.** The item nominated the
   status-bar dot and then said the quiet part itself — *"the point of the item is the standing
   condition, not that particular glyph"* — which is what settled it. That dot renders
   `ResultsState` and lives in the results footer: per-window, but reading as per-*result*. A
   persist failure is neither, so taking the glyph over would have made one surface answer two
   unrelated questions.

   What landed instead satisfies the requirement more completely than a dot would have:

   - **`PersistFaults`** holds the condition — a row per file that is behind, retracted by the
     next successful write to it, which is exactly "cleared by the next successful write".
   - **The rail badge totals it** with the SQL errors, so the condition is visible *without
     opening anything* — the property a drawer row alone would not have had.
   - **The Events row is untouched**, still the record of when it started (and only the
     transition, so a repeating writer cannot bury the log).

   It also pays for itself twice, which the dot would not have: registration failures moved onto
   the same surface, where before they were visible only as a triangle on one catalog row plus an
   event that scrolled away.
4. ✅ **The destructive case: a drop whose write fails is rolled back.**

   A drop is the only mutation whose silent failure **resurrects** what the user destroyed — the
   row leaves the catalog, `project.json` still lists it, and it is back at the next open. A
   destructive action they deliberately confirmed, undone later with nothing said.

   It is also the only one that *can* be rolled back, which is what decided it rather than taste.
   `save_defs` writes `self.defs()`, a pure projection, so "write first, then mutate" is not
   available — but that only fixes the *order*, not the outcome: at the moment the write fails
   nothing else has happened yet. Every arm of `drop_row` mutates the store and persists inside
   one guard, and the engine and session calls all come after. So the removal hands the row back
   (`remove_table`/`remove_view`/`remove_saved_query` now return it and its slot) and a failed
   write puts it exactly back **inside that same guard** — subscribers see one notification
   carrying the original state, never a row that vanishes and returns.

   Returning the row rather than cloning the section is the load-bearing detail: it keeps `Clone`
   off `TableRow`/`ViewRow`/`Reg`, and it restores the **registration state** too. A row that came
   back as `Reg::Loading` would spin in the catalog forever, because nothing is going to answer
   for it.

   **The policy is "roll back what can be rolled back", not "mutations are atomic"** — and the
   asymmetry is deliberate rather than an oversight. `save_view` genuinely cannot: `CREATE OR
   REPLACE VIEW` has already succeeded on the engine, so the view is live and queryable for the
   rest of the session, and undoing it needs a second fallible engine call. The two situations
   differ in what has already become true. Item 3's `Project` scope carries the other half either
   way, naming the file that is behind for as long as it is.

   Covered by `drop_confirm`'s `a_drop_whose_project_write_fails_is_rolled_back`,
   `a_rolled_back_drop_keeps_the_catalogs_order` (the row returns to its slot, not the end) and
   `a_dropped_query_whose_write_fails_stays_in_the_catalog` (the arm with no engine at all).
5. ✅ **Settle the two writers that report *directly*** — the export (P4-10) and the history Clear
   (P3-14). Both call `log_event` themselves rather than going through the funnel, each because
   the funnel did not exist when they were written, and P4-10's own file said to route through
   here once it does.

   **But check whether they belong**, rather than folding them in reflexively. The export writes
   to a destination the *user picked*, not into `.strata`, so items 3 and 4 don't apply to it —
   no standing "the project is behind" condition, nothing to roll back. The Clear is the opposite
   case and worth a look for the same reason: it *is* a `.strata` write, so item 3 applies, but it
   has nothing to gate — the satellite is emptied before the file is touched, so its failure mode
   is the drawer already showing the cleared state while `history.jsonl` still holds the rows.
   It may be that the funnel is the right home for the reporting shape and the wrong home for one
   or both of these writers. Decide, and record which.
6. ✅ **Which window hears about an app-config failure?** The config store is app-global; the event
   log is per-window. Options: every open window's log, the focused window's, or neither (an inline
   error on the Settings surface that owns the edit — P4-04..P4-09). Settle it here; a settings
   write that fails while the Settings window is open should not be reported only in a project
   window's drawer.
7. ✅ **Messages name the file.** `project_io::save_defs`'s `create_dir_all` arm maps to a bare
   `e.to_string()` while its `write_atomic` arm carries the path, so the same failure reads two
   ways. One shape, path included, for every writer.
8. ⬜ **Read and write as one policy.** Align the wording and the reporting surface with P4-01 item
   5's close-the-window path, so a user who cannot write and a user who cannot read are not told
   in two unrelated registers.

## Non-goals (this is how a "resiliency" task becomes a quarter)

- Retry loops, backoff, or a write queue.
- File watching / reload-on-external-change (CLAUDE.md: disk is a startup input, read once).
- Conflict resolution or merge, and anything about two app instances writing one project — the
  single-instance story is P4-01's.
- Rewriting `write_atomic`. It already does its job; this task is about who hears when it can't.

## Acceptance
- [x] No `.strata` (or app-config) write path reports failure only through `tracing`.
- [x] `strata_core::config::save` returns its `Result`; `write_config` handles it.
- [x] With a read-only `.strata/`: a save, a drop, a rename, a session autosave and a history
      append each surface the failure, and no success is claimed for any of them.
- [x] The failure is visible for as long as it holds, not only in the moment it happened — the
      Problems drawer's `Project` scope holds the condition and the rail badge totals it, so it
      reads without opening anything and retracts on the next successful write (item 3).
      **One case it cannot cover:** the *final* session save on close records into a log and a
      store that are dropped with their window a moment later. That is not an indicator problem —
      there is no "while it holds" for a window that is going away — and it is filed with P4-16.
- [x] The destructive-case decision (build item 4) is implemented and its reasoning is in this file.
- [x] Tests cover a write failure per family (defs · session · history) —
      `drop_confirm`'s `a_drop_whose_project_write_fails_is_logged_as_the_failure` (P3-13) is the
      pattern: chmod the directory `0o500`, act, assert; `state::persist::tests` follows it for
      the other two. **Config is the exception** and stays untested: forcing it to fail means
      redirecting the OS config dir, i.e. a production seam added for a test.

## Freya / references
- P3-13 (`.claude/tasks/phase-3-catalog-inspector-drawer/P3-13-drawer-events.md`) — the event log
  this reports into, and the `persisted` helper to generalise.
- P4-01 build item 5 (the read-side counterpart), P4-13 (open/load), P4-14 (session + history IO).
- `strata_core::util::write_atomic` (and its tests) for what is already guaranteed;
  `strata_core::project::{save_defs, save_session, append_history}`; `strata_core::config::save`.
- AGENTS.md §1 (fail loud on the unrecoverable, never a silent fallback), §2 (one app-global config
  store — `write_config` is the sole write path), §3 (user-facing text; no stacked near-duplicates).
