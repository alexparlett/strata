# P2-14 · Clear results

**Phase:** 2 — Workbench · **Status:** 🟢 · **DEV_TASKS:** Rz8 · **Depends on:** P2-02

## Goal
The toolbar trash button clears the active tab's result back to the empty state.

## Current state
The `Trash` button renders in `results/toolbar.rs` but is inert.

## Build
Wire it to `request.set(None)` (the active query drops → Empty) and clear the tab's find query.
No-op mid-run (guard on the query's `Loading` state). Destructive-red hover on the button.

## Acceptance
- [ ] Trash clears results → Empty; disabled/no-op while a query is running.

## Freya / references
- `results/toolbar.rs`. State-arch §6 (the `request` signal). Depends on P2-02.
