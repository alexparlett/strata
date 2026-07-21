# P2-08 · Status bar — pager + info + selection aggregate

**Phase:** 2 — Workbench · **Status:** 🟢 · **DEV_TASKS:** U6 / Rz3 · **Depends on:** P2-03

## Goal
Fill out the results status bar: pager, row/snapshot info, elapsed, and the live selection aggregate.

## Current state
`results/status_bar.rs` renders a semantic state dot + a coarse label + a selection summary string.
Its own TODO notes the pager / snapshot / aggregate are still to come.
The theme token `hover_background` is already reserved for pager buttons.

## Build
1. **Pager** (prev/next + "page N of M" / row range) by bumping the snapshot read's page (P2-01/03).
   Style the pager buttons using the reserved `hover_background`.
2. **Info**: row count / snapshot chip / elapsed from the settled `QueryPage` + `query.read().state()`.
3. **Selection aggregate**: count / sum / avg / min / max over the *real* selected cell values
   (Rz3) — replace the current index-only summary string.

## Acceptance
- [ ] Pager navigates pages and shows the current range.
- [ ] Selecting numeric cells shows a live aggregate; non-numeric shows count.
- [ ] Row count / elapsed reflect the real run.

## Freya / references
- `results/status_bar.rs`. Core `serialize`/result types for values. Design: `StatusBar.dc.html`.
