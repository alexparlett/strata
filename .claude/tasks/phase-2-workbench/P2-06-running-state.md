# P2-06 · Running state (spinner + elapsed + cancel)

**Phase:** 2 — Workbench · **Status:** ⬜ · **Depends on:** P2-02 · **Related:** P2-15 (Cancel)

## Goal
A real "query running" body: spinner, elapsed time, and a Cancel affordance.

## Current state
`results/running.rs` centres the text "Running query…". No spinner, no timer, no cancel.

## Build
1. Reuse Freya **`CircularLoader`** (or `ProgressBar`) for the spinner.
2. Show **elapsed time** ticking from run start (a `use_future`/interval; store the start instant
   in a local signal when the query enters `Loading`).
3. A **Cancel** button → `query.cancel()` + the engine cancel command (shares P2-15's Run→Cancel path).

## Acceptance
- [ ] Running a query shows the spinner + a live elapsed timer.
- [ ] Cancel stops the run and returns to the previous/empty state.

## Freya / references
- Freya `CircularLoader` / `ProgressBar` / `Skeleton` (plan §5 DS map). Design: `Results.dc.html` running state.
