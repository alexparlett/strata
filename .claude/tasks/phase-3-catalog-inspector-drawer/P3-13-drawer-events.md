# P3-13 · Drawer — Events tab

**Phase:** 3 · **Status:** ✅ · **DEV_TASKS:** U10 · **Depends on:** P3-11, P3-12

## Goal
The engine event log in the Events tab.

## What shipped

**The store** — `strata-freya/src/apps/project/state/log.rs`: `Log` (a capped `VecDeque`,
newest-first, 200) behind `LogCtx = State<Log>`, stood up by `use_init_log()` in `ProjectRoot`
**before** `use_init_project`, because the open is its first entry. A context signal on the same
terms as the history satellite, not a Radio store: one append wakes one reader.

state-arch §8's `LogCtx` was never written and the Dioxus-era `strata-model/src/log.rs`
(`LogEvent` / `LogKind` / `LogTab`) is **deleted** — `LogTab` duplicated `DrawerTab`, `open` was a
UI flag in a serde-only crate, and nothing live referenced any of it. The real shape is
app-side, since nothing here is persisted.

**The view** — `views/drawer/events.rs`: flat rows over P3-12's shared frame (`DrawerBody`
scroll container; `DrawerEmpty` for "No events yet", wearing the rail's own Events glyph). Each
row is dot · message · `HH:MM:SS`, over the `drawer` theme's new `divider_fill` hairline. The
message **wraps** (an engine error is a sentence, and this is the surface that keeps it after the
run is gone), which is why the dot and timestamp are top-aligned with a small nudge rather than
centred. Keyed by append sequence.

**Clear** — the first working one. The button and its Events/History-only rule were already in
`drawer/mod.rs`; it now has an `on_press` that empties the log and is enabled off the
`DrawerCount` the body writes, so the header's number and the button can never disagree about
whether there is anything to clear. History's stays parked (P3-14 owns its truncate).

**Where events come from.** Whoever observed the fact records it — there is deliberately no
producer hook, which is the *opposite* of the diagnostics driver and for the opposite reason (a
diagnostic can be re-derived from the buffer and the catalog; an event cannot be re-derived from
anything):

| Fact | Recorded in | Level |
|---|---|---|
| Project opened | `state::hooks::use_init_project` | Info |
| Table registered / failed (open · ↻ · row Refresh) | `state::hooks::register_defs` | Ok / Error |
| View registered / failed | same | Ok / Error |
| Saved view · saved query | `editor::actions::save_view` / `save_query` | Ok / Error |
| Table / view / query dropped | `dialogs::drop_confirm::drop_row` | Info |
| `DROP VIEW` the engine refused after the def went | same | Warning |
| Run settled: rows · plan · failure | `views::keeper` → `use_run_logging` | Ok / Error |
| Run cancelled | `editor::actions::cancel_run` | Warning |

## Decisions worth keeping

- **No `origin` field.** state-arch §8 sketched a level *and* an origin per entry. The level is
  real (the dot's tone, and an error's message tone). The origin is not: every message already
  names its subject ("Registered table 'users'"), so a structured copy would be a second copy
  that can disagree with the sentence beside it — the reason a `Diagnostic` carries no `TabId`
  (P3-12) — and nothing filters the list today. A filter, or a toast host wanting "recent
  warn+", adds the field when it is the thing being built.
- **Four levels, not five.** `Ok / Info / Warning / Error` = the sheet's four semantic slots
  (`success` / `info` / `warning` / `error`), read off the sheet like every other semantic mark
  (AGENTS.md §3). The canvas's separate `run` kind painted the same colour as `info` and differed
  in nothing else.
- **A cancel is logged at the cancel, not at the settle.** Clearing the tab's trigger unmounts
  the press's keeper in the same pass, so the entry's `Err("cancelled")` settles with nobody
  subscribed (the keeper's own doc says so). `Engine::cancel` returns the elapsed time *iff* it
  really aborted something — both the guard against logging a cancel that hit nothing, and the
  one real fact the event carries. `run_event`'s `Warning` arm stays as the mapping for a settle
  that *is* observed, so a stopped run can never be logged as an error.
  - **Correction from the PR review:** that arm originally tested `e == "cancelled"`, on the belief
    that a cancel and a supersede settle the same string. They don't — `Engine::query` settles
    `Err("superseded by a newer run")` when a press finishes after a newer one replaced it
    (`engine/mod.rs`, the `latest == false` arm), a *different* path from the abort. So a supersede
    would have logged as a red `Error` reading the engine's raw prose. The strings are now named
    consts in `strata-core::engine` with one predicate, `stopped_on_purpose`, which both this and
    the inspector's scan zone call — that zone had kept its own copy of the rule
    (`== "cancelled" || starts_with("superseded")`), so the concept existed twice and one copy had
    already drifted. Unreachable today (the pin unmounts before the superseded press settles), but
    the mapping is the guard, and a guard with a hole is not one.
  - This unified the two cancel paths: the results pane's Running body had its own copy of
    `cancel_run`'s two steps and now calls it, so one of them can't quietly stop recording.
- **Per-def scan events, no synthesized summary.** One event per answer the engine gave, for
  every width of pass (open · ↻ · row Refresh). A "re-scanned N tables" line would be a second
  derivation of facts already in the list. A view that fails a *round* is not logged — its
  dependency may land next round; only the final no-progress round records failures.
- **`now_hms` is now local, not UTC** (`strata-core::util`). A log timestamp is read against the
  clock in the user's menu bar, so unmarked UTC would be a lie the reader can't detect. The zone
  is no longer a guess: `chrono` (already in the graph via datafusion → arrow, with `clock` on) is
  now a named dep. `iso8601` — an absolute instant with no clock beside it — stays UTC-and-`Z`.
- **Profiling is deliberately not logged.** A scan's result is a freya-query entry keyed by its
  request and the inspector renders it in place; the log would be a third copy of a number.

## Acceptance
- [x] Engine/window/query events appear in the log; Clear empties it.

## Follow-ups for other tasks
- **P3-14 (History)** owns History's Clear (its truncate) and writes the same `DrawerCount`.
- **Export (P4-10) / Table Config (P4-11) / import options (P4-12)** should record their outcomes
  the same way — capture the `LogCtx` at render time and call `log_event`; there is no producer to
  register with. Each of those files now carries the pointer.
- **P4-15 · `.strata` write resiliency** picks up what this task started and deliberately scoped
  out. The review of P3-13 found `save_defs` failing was a `tracing::error!` and nothing more,
  while the new log positively claimed the mutation had stuck; the fix was `actions::persisted`,
  which records `Could not write the project file: <e>` and returns whether the write landed, with
  every success event gated on it (Save, Save-as-view, and the drop — whose event also moved to
  *after* the write). What P4-15 owns: generalising that helper out of
  `views/workbench/editor/actions.rs` into `state/`; the four writers still reporting only through
  `tracing` (the saved-query rename at `sidebar/catalog/menu.rs:385`, session autosave, its final
  save on close/re-root, the history append); `strata_core::config::save` discarding its `Result`
  outright; a **standing** indication for a condition a scrolling log row under-reports; and
  whether a failed destructive write rolls back.

## Freya / references
- state-arch §8 (the log) and §9 (what gets logged, and what is *also* shown in place). Design:
  `Strata.dc.html` lines 1304–1320 (`DrawerEvents.dc.html` is a crop of it).
