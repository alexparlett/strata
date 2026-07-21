# P4-04 · Settings ▸ Appearance

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** U12 · **Depends on:** P4-03

## Goal
The Appearance category: theme selection (live) + related display prefs.

## Current state
Not built.

## Build
- Theme cards (Midnight / Daylight / any custom) with a **source badge**; selecting one writes the
  shared `theme` signal live (P4-03). **Sync with OS** toggle (follows the OS dark/light).
- Match the canvas structure (Appearance already matched structurally in the Dioxus app).

## Acceptance
- [ ] Selecting a theme previews live across windows; Sync-with-OS follows the OS; persists on Save.

## Freya / references
- Design: `Settings.dc.html` Appearance. Themes from `theme.rs` / `FreyaThemeGallery.dc.html`. DEV_TASKS U12.
