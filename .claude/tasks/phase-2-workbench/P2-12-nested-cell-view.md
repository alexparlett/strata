# P2-12 · Cell double-click → nested-data view

**Phase:** 2 — Workbench · **Status:** ⬜ · **DEV_TASKS:** U5 · **Depends on:** P2-03 · **Related:** P2-10 (gutter → whole row)

## Goal
**Double-clicking a cell** (`onCellDbl` in the canvas) opens the cell's value in the **nested-cell
view**. One of the design's two grid double-click targets: **cell → nested value**, **gutter → whole
row** (P2-10).

## Current state
Not built. Body cells have no double-click handler (`datagrid/cell.rs`).

## Presentation (from `Strata.dc.html`, `cellViewOpen`)
A **centred backdrop modal** — **not** an anchored popover:
- Full-window backdrop: `position: fixed; inset: 0; z-index: 64;` dim `rgba(4,6,10,.6)` + `blur(3px)`,
  flex-centred. Backdrop click closes (`onCellBackdrop`).
- Card: **460px** wide (`max-width: 92vw; max-height: 80vh`), `--c-pop` bg, `--c-border3`, `--r-4`,
  shadow, column layout.
- Header: cell **name** (mono 12.5) + a **type badge** (mono 10, cyan `#8ad4ff` on
  `rgba(138,212,255,.12)`, `--r-xs`) + ghost **close** (`onCloseCell`).
- Body: a `<pre>` scroll area (`--c-panel` bg, JetBrains Mono `12px/1.6`, `--c-text2`) showing the
  value — pretty JSON for nested (`serialize::cell_pretty_json`).

## Build
1. Add a double-click handler to body cells (`datagrid/cell.rs`) using
   `EventsCombos::pressed(loc).is_double()` inside the existing pointer handler, so it coexists with
   single-click selection.
2. Open the nested-cell view as a centred backdrop modal on the Freya overlay family (backdrop +
   centred card / `Dialog`) matching the tokens above.

## Acceptance
- [ ] Double-clicking a cell opens the centred modal with the value as pretty JSON; backdrop/close dismiss.
- [ ] Single-click still selects; double-click doesn't corrupt the selection.

## Freya / references
- `datagrid/cell.rs`, `EventsCombos::is_double()` (as the resize grip uses). Freya overlay/`Dialog`
  family. Core `serialize::cell_pretty_json`. Design: `Strata.dc.html` `cellViewOpen`, DEV_TASKS U5.
