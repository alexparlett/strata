# P4-05 · Settings ▸ Data-display

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** U12 · **Depends on:** P4-03

## Goal
The Data-display category (grid/formatting prefs — e.g. default column width, null/date/timestamp
formatting, type-colour toggles).

## Current state
Not built. `format.*` prefs wire into the grid cell formatter.

## Build
- Render the data-display fields (numeric inputs / toggles / selects) editing the draft; on Save they
  apply to the grid formatter + defaults. Uniform divider-separated list (no ALL-CAPS section labels).

## Acceptance
- [ ] Fields edit the draft; Save applies to the grid (formatting, default col width, etc.).

## Freya / references
- Design: `Settings.dc.html` Data-display. Grid formatter (`CellFormat`). DEV_TASKS U12.
