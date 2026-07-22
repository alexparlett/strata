# P2-08 · Status bar — pager + info + selection aggregate

**Phase:** 2 — Workbench · **Status:** 🟢 · **DEV_TASKS:** U6 / Rz3 · **Depends on:** P2-03

## Goal
Fill out the results status bar: pager, row/snapshot info, elapsed, and the live selection aggregate.

## Current state
`results/status_bar.rs` renders a semantic state dot + a coarse label + a selection summary string,
plus a **minimal working pager** from P2-03: `Pager { page: State<usize>, total, page_size }` threaded
from `ResultsBody`, two flat chevron buttons + a "1–100 of N" range, right-pinned. The *mechanism* is
done — bumping the page `State` re-keys the grid's `FetchSnapshotPage` read — but the cluster is far
off the comp (`Strata.dc.html` `data-rg="statusbar"`). The theme token `hover_background` is reserved
for the pager buttons and still unpainted.

## Build
1. **Pager to the comp**: page-size dropdown ("100 / page", upward menu) · 1px divider ·
   first / prev / **page-number input** ("of M") / next / last as 28×26 ghost buttons with
   `hover_background` + disabled styling. Mechanism stays P2-03's page `State`.
   *Wrinkle:* changing page size must re-read **page 1 through `FetchSnapshotPage`* too — the Run's
   embedded page 1 is only valid for the Run's own `page_size` (today the grid short-circuits page 1
   to the Run output).
2. **Info**: sub-label / snapshot chip (clock icon + snapshot tooltip) / elapsed from the settled
   `QueryPage` + `query.read().state()`.
3. **Selection aggregate**: count / sum / avg / min / max over the *real* selected cell values
   (Rz3) — replace the current index-only summary string; accent-coloured per the comp.

## Acceptance
- [ ] Pager navigates pages and shows the current range.
- [ ] Selecting numeric cells shows a live aggregate; non-numeric shows count.
- [ ] Row count / elapsed reflect the real run.

## Freya / references
- `results/status_bar.rs`. Core `serialize`/result types for values. Design: `StatusBar.dc.html`.
