# P3-08 · Column inspector (facts box)

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U9 · **Depends on:** P3-01

## Goal
A right-panel inspector showing a selected column's real, footer-derived metadata.

## Current state
Not built. Free stats come from DataFusion `Statistics` via `ListingTable::list_files_for_scan`
(footers only, no data pages) → `ColumnInfo.stats`. The inspector **selection** is per-window client
state (Radio station, plan §4).

## Build
1. A **bordered facts box** of dynamic key/value rows showing **footer-derived metadata only**
   ("only real facts"; `varies by source format; never fabricated`), a title source-format badge, and
   depth-indented nested rows.
2. Selecting any column — **including nested** (the gap P3-02/P3-07 leaves) — populates it via the
   inspector-selection channel.
3. The **completeness bar** renders only with a real null count (footer or profile) — never computed
   off the result page. A `null_count == num_rows` is dropped (ambiguous in DataFusion), leaving the
   profile to answer it.

## Acceptance
- [ ] Selecting a column (top-level or nested) shows its real metadata; no fabricated stats.
- [ ] Completeness bar shows only when a real null count exists.

## Freya / references
- Bespoke — hand-roll (plan §5). Core `ColumnInfo.stats` / `CatalogTable.rows`. Design:
  `Sidebar.dc.html` / inspector canvas. DEV_TASKS U9 (the "only real facts" reasoning + honesty calls).
