# P6-02 · Native menu bar (decision + menu-follows-opener)

**Phase:** 6 · **Status:** ⬜ **needs decision** · **DEV_TASKS:** F8 · **Depends on:** P4-01

## Goal
A native macOS menu bar (File/Edit/Window…), or a deliberate decision not to.

## Current state
Not built. Freya has tray menus only. **Decision pending:** `madsmtm/menubar` vs an in-app menu
(plan §8). Native key events remove much of the *reason* the Dioxus menu existed (⌘A/⌘C swallowing,
the whole F8 muda/shortcut tangle).

## Build
- Decide + document: `madsmtm/menubar` (native) vs in-app menu vs none-for-now.
- If built: **menu-follows-opener** (launcher → light menu; project → full; settings → match its
  opener). Predefined Edit items where possible (native Cut/Copy/Paste/Undo); the grid's ⌘A/⌘C are
  native events now (P2-20), so the custom-item shims that caused the muda crash aren't needed.

## Acceptance
- [ ] A decision is recorded; if a menu ships, it follows the opener and uses predefined items where possible.

## Freya / references
- Plan §8 (native menu open item). DEV_TASKS F8 (the muda/shortcut analysis + the crash). `platform/`.
