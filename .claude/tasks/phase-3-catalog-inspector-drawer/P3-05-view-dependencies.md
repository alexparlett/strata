# P3-05 · View dependencies (UI consumer)

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** D10 · **Depends on:** P3-02/04

## Goal
Use the core-derived view→base-table deps in the UI.

## Current state
Core derives `CatalogView.deps` from the planner (not by parsing SQL). No UI consumer yet.

## Build
1. Feed P3-04's validity check from `deps`.
2. The **table-drop confirm** names the views that will be **left invalid** (flagged), not "stop
   working" (DF-54: they run until reload). Wire this into the drop context-menu action (P3-06).

## Acceptance
- [ ] Dropping a table lists its dependent views in the confirm dialog.

## Freya / references
- Core `CatalogView.deps`. DEV_TASKS D10 (the planner-derived deps + DF-54 nuance).
