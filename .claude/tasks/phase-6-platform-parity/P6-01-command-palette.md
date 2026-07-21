# P6-01 · Command palette (⌘K) + depth

**Phase:** 6 · **Status:** ⬜ · **DEV_TASKS:** U11 / T3 · **Depends on:** P2-20

## Goal
The ⌘K command palette, with the "depth" niceties.

## Current state
Not built. P2-20 binds ⌘K; this is the palette surface it opens.

## Build
- A palette overlay (filter + list) over the command registry (the P2-20 command table + actions).
- **Depth (T3):** grouping, keyboard navigation (↑↓ + enter), per-item **type icons** + **shortcut
  hints** (from `keymap::hint`), a columns group. The footer already advertises "↑↓ navigate".

## Acceptance
- [ ] ⌘K opens the palette; typing filters; ↑↓/enter navigate + run; items show icons + shortcut hints.

## Freya / references
- Freya overlay/`Popup`. Command table from P2-20 / `Strata.dc.html` `_commands()`. DEV_TASKS U11/T3.
