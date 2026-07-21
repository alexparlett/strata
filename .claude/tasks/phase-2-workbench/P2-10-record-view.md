# P2-10 · Gutter double-click → row detail (record view)

**Phase:** 2 — Workbench · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** Rz5 · **Depends on:** P2-03 · **Related:** P2-12 (cell → nested)

## Goal
**Double-clicking the row-number gutter** (`onRowOpen`) opens the **entire row** in the record view.
The design's other grid double-click target (cell → nested is P2-12).

## Current state
Not built. The gutter cell exists (`CellRole::Row`) but has no double-click handler.

## Presentation (from `Strata.dc.html`, `rowViewOpen`)
A **centred backdrop modal** (same overlay pattern as P2-12, not a popover):
- Backdrop: `fixed; inset: 0; z-index: 64;` dim + `blur(3px)`, centred; backdrop click closes
  (`onRowBackdrop`).
- Card: **540px** (`max-width: 92vw; max-height: 82vh`), `--c-pop`, `--c-border3`, `--r-4`, shadow.
- Header: `rowViewLabel` (e.g. `Row n of total`) + **Copy row as JSON** (`onCopyRecordJSON`) + **Copy
  row as CSV** (`onCopyRecordCSV`) + divider + **prev** (`onRowPrev`) / **next** (`onRowNext`) + ghost
  **close** (`onCloseRow`).
- Body: scroll list of `rowViewFields`; each field = a 150px left column (name mono 12.5 + type in the
  type-colour dot), then either a nested `<pre>` block (`max-height: 190px`, `--c-panel`, `--r-2`,
  `11px/1.55`) or a scalar value span (mono 12.5, value colour).

## Build
1. Double-click on the row gutter (`datagrid/cell.rs`, `CellRole::Row`) opens the record view.
2. Build the centred backdrop modal to the tokens above; render fields from the current row
   (name · type-coloured Arrow type; nested → `serialize::cell_pretty_json`, scalar → grid-coloured).
3. Prev/next move within the page (clamped). Copy row as **JSON / CSV** per the canvas header (the
   fuller format set is the grid-selection copy, P2-11).

## Acceptance
- [ ] Double-click a row number → centred modal with all columns; nested cells show pretty JSON.
- [ ] Prev/next move within the page; Copy JSON/CSV produce the row.

## Freya / references
- `datagrid/cell.rs` (`CellRole::Row` double-click). Freya overlay/`Dialog` family. Core
  `serialize::cell_pretty_json`. Design: `Strata.dc.html` `rowViewOpen`, DEV_TASKS Rz5.
