# P4-15 · `.strata` write resiliency — one funnel, nothing silent

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** project lifecycle · **Depends on:** P4-13, P4-14, P3-13

## Goal
Make a **failed write** to the user's project a stated fact rather than a `tracing` line nobody
sees. Every `.strata` writer goes through one funnel, the failure is visible in the app while it
lasts, and the destructive case has a decided answer rather than an accidental one.

## Why this is its own task, and why it is phase 4

**P4-01 item 5 owns the read side**: a defs or session file that won't *load* closes the window
(replacing today's interim `panic!`). This is the same problem from the other end, and nobody owns
it:

| | Read failure (at open) | Write failure (mid-session) |
|---|---|---|
| State | the window can't exist | the window is fine; the durable copy is behind |
| Recoverable | no | **yes** — fix the permission, reconnect the volume, free the disk |
| Decision | one: close the window | a per-mutation policy |
| Owner | P4-01 item 5 | **this task** |

And it belongs *early* in the remaining phase-4 work, because three phase-4 tasks add **new**
def-mutation sites — **P4-11** (register-table modal), **P4-12** (import read options), **P4-10**
(export, which writes files of its own) — and every existing site's local idiom is
`if let Err(e) = … { tracing::error!(…) }`. Decide the policy before the writers land, or each one
copies the silence.

Not design polish, so **not** phase 5: P5 is tokens, interaction states, animation, theme dial-in
and the drift audit.

## Current state (verified 2026-07-26)

`write_atomic` already guarantees the good half: a failed write leaves the previous file **intact**
and strands no temp, so a failure is never "your catalog file is corrupt" — always "your catalog
file is one revision behind the screen". What's missing is anyone saying so.

| Writer | Path | On failure today |
|---|---|---|
| `save_view` / `save_query` | `project.json` | ✅ `actions::persisted` — logs `Could not write the project file: <e>` and gates the success event (P3-13) |
| `drop_row` (3 arms) | `project.json` | ✅ same funnel; the drop event is logged only if the write landed (P3-13) |
| Saved-query **rename** | `project.json` | ❌ `tracing` only — `views/sidebar/catalog/menu.rs:385` |
| Session **autosave** (debounced) | `session.json` | ❌ `tracing` only — `state/hooks.rs:591` |
| Session **final save** on close / re-root | `session.json` | ❌ `tracing` only — `state/hooks.rs:622`. The highest-stakes one: there is no later write to make up for it, and the window is going away |
| History append | `history.jsonl` | ❌ `tracing` only — `state/history.rs:143` |
| App config (settings · recents · open-set) | OS config dir | ❌ **unreportable by signature**: `strata_core::config::save` returns `()` and swallows the `Err`, so `write_config` — documented as the sole write path — cannot know it failed |

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

1. **One funnel, in `state/`.** Generalise P3-13's `persisted(&project, log) -> bool` into the
   place the stores live (e.g. `state::persist`), and route **every** writer above through it.
   It is currently in `views/workbench/editor/actions.rs`, which is the wrong home for something
   the drop confirm already imports and the config modal will — that was expedient in P3-13 and is
   this task's to fix. No write site may keep a bare `tracing::error!`.
2. **Make the config write reportable.** `strata_core::config::save` must return its `Result` and
   `write_config` must act on it. A sole-write-path funnel that cannot fail-report is a hole in
   the invariant, not a simplification.
3. **A standing condition, not just an event.** A failed write is a *state* (the file is behind and
   stays behind), so a log row alone under-reports it: the row scrolls away while the condition
   holds. Add one persistent indication, cleared by the next successful write — the status bar
   already has the state dot this shape belongs on (`views/workbench/results/status_bar.rs`), and
   the Events row stays as the record of when it happened. **Do not** stack a second message
   restating the row (AGENTS.md §3).
4. **Decide the destructive case.** A drop whose write fails leaves the store and disk out of step
   and reverts on reopen. Either snapshot the section and roll it back on failure, or state
   plainly that we don't and let the log and the indicator carry it. Today it is the latter *by
   default rather than by decision* — pick one and record the reasoning here. Note the constraint:
   `save_defs` writes `self.defs()`, a **pure projection of the store**, so the store must change
   before there is anything to write; "write first, then mutate" is not available.
5. **Adopt the export's failure path** (P4-10, already shipped). The Export window records both
   arms straight into P3-13's log — `log_event(log, LogLevel::Ok, "Exported n rows to <path>")`
   and an `Error` row on failure (`apps/export/views/footer.rs`), skipping
   `stopped_on_purpose` settles. It does that because this funnel did not exist yet; P4-10's own
   file said to route through here once it does.

   **But check whether it belongs**, rather than folding it in reflexively: an export writes to a
   destination the *user picked*, not into `.strata`, so items 3 and 4 don't apply to it — there
   is no standing "the project is behind" condition and nothing to roll back. It may be that the
   funnel is the right home for the reporting shape and the wrong home for this writer. Decide,
   and record which.
6. **Which window hears about an app-config failure?** The config store is app-global; the event
   log is per-window. Options: every open window's log, the focused window's, or neither (an inline
   error on the Settings surface that owns the edit — P4-04..P4-09). Settle it here; a settings
   write that fails while the Settings window is open should not be reported only in a project
   window's drawer.
6. **Messages name the file.** `project_io::save_defs`'s `create_dir_all` arm maps to a bare
   `e.to_string()` while its `write_atomic` arm carries the path, so the same failure reads two
   ways. One shape, path included, for every writer.
7. **Read and write as one policy.** Align the wording and the reporting surface with P4-01 item
   5's close-the-window path, so a user who cannot write and a user who cannot read are not told
   in two unrelated registers.

## Non-goals (this is how a "resiliency" task becomes a quarter)

- Retry loops, backoff, or a write queue.
- File watching / reload-on-external-change (CLAUDE.md: disk is a startup input, read once).
- Conflict resolution or merge, and anything about two app instances writing one project — the
  single-instance story is P4-01's.
- Rewriting `write_atomic`. It already does its job; this task is about who hears when it can't.

## Acceptance
- [ ] No `.strata` (or app-config) write path reports failure only through `tracing`.
- [ ] `strata_core::config::save` returns its `Result`; `write_config` handles it.
- [ ] With a read-only `.strata/`: a save, a drop, a rename, a session autosave and a history
      append each surface the failure, and no success is claimed for any of them.
- [ ] The failure is visible for as long as it holds, not only in the moment it happened.
- [ ] The destructive-case decision (build item 4) is implemented and its reasoning is in this file.
- [ ] Tests cover at least one write failure per family (defs · session · history · config) —
      `drop_confirm`'s `a_drop_whose_project_write_fails_is_logged_as_the_failure` (P3-13) is the
      pattern: chmod the directory `0o500`, act, assert.

## Freya / references
- P3-13 (`.claude/tasks/phase-3-catalog-inspector-drawer/P3-13-drawer-events.md`) — the event log
  this reports into, and the `persisted` helper to generalise.
- P4-01 build item 5 (the read-side counterpart), P4-13 (open/load), P4-14 (session + history IO).
- `strata_core::util::write_atomic` (and its tests) for what is already guaranteed;
  `strata_core::project::{save_defs, save_session, append_history}`; `strata_core::config::save`.
- AGENTS.md §1 (fail loud on the unrecoverable, never a silent fallback), §2 (one app-global config
  store — `write_config` is the sole write path), §3 (user-facing text; no stacked near-duplicates).
