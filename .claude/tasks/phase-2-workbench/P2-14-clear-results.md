# P2-14 · Clear results

**Phase:** 2 — Workbench · **Status:** ✅ **built** · **DEV_TASKS:** Rz8 · **Depends on:** P2-02

## Goal
The toolbar trash button clears the active tab's result back to the empty state.

## Current state
The `Trash` button renders in `results/toolbar.rs` but is inert.

## Build
Wire it to `request.set(None)` (the active query drops → Empty) and clear the tab's find query.
No-op mid-run (guard on the query's `Loading` state). Destructive-red hover on the button.

**As built:** Trash drops the Run trigger (threaded `ResultsBody` → `DataGrid` → toolbar as
struct-field props) and resets the shared `Selection`, so a later run starts clean. The mid-run
guard is structural — the toolbar only renders inside the settled grid body (running shows the
Running body instead), so the button can't fire mid-run. Destructive hover = `colors.error` icon
over 15%/45% red-tinted fill/border (the Dioxus `.res-clear` recipe). The find query doesn't exist
yet — clearing it joins the Trash reset when P2-09 lands.

## Acceptance
- [x] Trash clears results → Empty; disabled/no-op while a query is running.

## Freya / references
- `results/toolbar.rs`. State-arch §6 (the `request` signal). Depends on P2-02.
