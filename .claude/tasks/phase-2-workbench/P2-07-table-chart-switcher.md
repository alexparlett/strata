# P2-07 · Table / Chart switcher

**Phase:** 2 — Workbench · **Status:** ⬜ · **DEV_TASKS:** U6a · **Depends on:** P2-02 · **Pairs with:** Chart workstream

## Goal
A segmented **Table · Chart** control in the results toolbar that switches the results body.

## Current state
`results/toolbar.rs` renders only the right-cluster icon buttons (Search/Reload/Trash/Download).
There is no view segment. The Chart body itself does not exist (see `workstream-chart-view/`).

## Build
1. Add a **`Segment`** (Freya `SegmentedButton`) `Table | Chart` on the left of the results toolbar.
2. Track the selected view in per-tab (or per-result) state; switch the results body accordingly.
3. Until the Chart body lands, Chart can show its own empty/placeholder — but the **switcher is real**.

## Acceptance
- [ ] The toolbar shows a Table/Chart segment; selecting Chart swaps the body region.
- [ ] Selection persists per result set.

## Freya / references
- Freya `SegmentedButton`. Design: `Results.dc.html` toolbar. Chart body: `workstream-chart-view/`.
