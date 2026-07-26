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

**From P3-11 via P3-12:** this task owns the first working **Clear**. The button and its
Events/History-only rule are already in `drawer/mod.rs`, shipped **parked** (`enabled(false)`)
because there is nothing to clear yet — give it an `on_press` and `enabled(!log.is_empty())`;
nothing else at the call site changes. The header's **count** is a `DrawerCount`
(`State<usize>`) the shell owns and the mounted body writes (see P3-12) — write the log's length
into it, and reset it on unmount as `Problems` does. The shared **frame** is `drawer/frame.rs`:
`DrawerBody` (scroll container) and `DrawerEmpty` (centred glyph + copy). Colours come from the
`drawer` component theme; extend it rather than adding a second one, and read `LogKind`'s dot
colours off the sheet's semantic ramp (`success`/`warning`/`error`/`info`), per AGENTS.md §3.

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
