# P2-07 · Table / Chart switcher

**Phase:** 2 — Workbench · **Status:** ✅ **built** · **DEV_TASKS:** U6a · **Depends on:** P2-02 · **Pairs with:** Chart workstream

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
- [x] The toolbar shows a Table/Chart segment; selecting Chart swaps the body region.
- [x] Selection persists per result set (per **tab**, per CHART_SPEC §1 — survives re-runs
      and tab switches).

## Freya / references
- Built as the bespoke `SegmentedToggle` (`components/segmented_toggle.rs`) + its
  `segmented_toggle` theme component — the design handoff replaced the ad-hoc text pair with
  a bespoke icon segmented toggle (handoff CHANGELOG), not Freya's `SegmentedButton`. View
  mode is `QueryTab::view` on `Chan::View(id)`; chart body is a placeholder
  (`results/chart.rs`) until `workstream-chart-view/` lands.
