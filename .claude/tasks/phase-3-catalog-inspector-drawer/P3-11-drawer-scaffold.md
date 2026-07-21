# P3-11 · Drawer scaffold (tabbed bottom panel)

**Phase:** 3 · **Status:** ⬜ · **DEV_TASKS:** U10 · **Depends on:** P3-01 · **Unblocks:** P3-12, P3-13, P3-14

## Goal
The bottom-drawer shell the three tabs render into: tab switcher, shared header, resizable height,
and the common list frame (sticky group headers, green-check empty states, indented rows).

## Current state
Not built. P3-01 provides the bottom panel region; this fills it with the drawer container.

## Build
1. Drawer container in the bottom panel (from P3-01): **Problems · Events · History** tabs + active-tab
   state (per-window layout/Radio). Collapsible + resizable height (Freya `ResizableContainer`).
2. **Shared header** with the tab switcher and a **Clear** button that shows on **Events / History**
   but is **hidden on Problems** (deliberate, DEV_TASKS U10 — Problems self-clear; a Clear there would
   lie). The scaffold owns this show/hide.
3. **List frame** the tabs reuse: sticky group headers, the green-check empty-state pattern, `--sp-7`
   row indent, `VirtualScrollView`.

## Acceptance
- [ ] Drawer opens/collapses/resizes; switching tabs swaps the body; empty states render.
- [ ] Clear shows on Events/History, hidden on Problems.

## Freya / references
- Freya `ResizableContainer` (height), `VirtualScrollView` (lists), a tab switcher. state-arch §8.
- Design: `DrawerProblems.dc.html`, `DrawerEvents.dc.html`, `DrawerHistory.dc.html`.
