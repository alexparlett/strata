# P3-13 · Drawer — Events tab

**Phase:** 3 · **Status:** ⬜ · **DEV_TASKS:** U10 · **Depends on:** P3-11, P3-12

## Goal
The engine event log in the Events tab.

## Current state
Not built — and **neither is the store**. state-arch §8's `LogCtx` was never written: nothing in
`strata-freya`, `strata-core` or `strata-forms` holds an event log, and the Dioxus-era event router
it describes went with the `Command`/`Event` protocol in P2-01. This task builds the store as well
as the view. Errors already reach the UI as each query's own `Err` state (state-arch §7), so the
log is an addition, not a rerouting.

`strata-model/src/log.rs` (`LogEvent` / `LogKind` / `LogTab`) is **dead Dioxus vocabulary** —
referenced by no live crate, `LogTab` duplicates `DrawerTab`, and `LogEvent.open` is a UI expansion
flag in a serde-only crate (without a `Serialize` derive, so it isn't even that). Define the real
shape here and delete it; don't build on it.

**From P3-11:** this task owns the first working **Clear** — the drawer header's Clear button and
its Events/History-only rule land with P3-12 (which owns the rule) and this task (the action).

## Build
- The store: a window-scoped log appended by whichever layer observes the fact (a settled query's
  `Err`, a mutation's result, a load summary), ephemeral, with a level + origin per entry.
- List the entries newest-first. Rows are **flat** — dot · message · timestamp with a bottom rule,
  per the canvas; the sticky group headers are Problems', not this tab's. Reuse P3-12's scroll
  container + empty state ("No events yet").
- **Clear** empties the log — the first working one. P3-12 owns the button's show/hide rule.

## Acceptance
- [ ] Engine/window/query events appear in the log; Clear empties it.

## Freya / references
- state-arch §8 (the log) and §9 (what gets logged, and what is *also* shown in place). Design:
  `Strata.dc.html` lines 1304–1320 (`DrawerEvents.dc.html` is a crop of it).
