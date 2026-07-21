# P4-03 · Settings window shell

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** W1 / U12 · **Depends on:** P4-01 · **Unblocks:** P4-04…P4-09

## Goal
The settings window frame: single canonical instance, category nav, draft/save, and live theme.

## Current state
Not built.

## Build (to `Settings.dc.html`, DEV_TASKS W1)
- Its own OS window (Freya `App` root), same chrome as a project window; **single instance,
  focus-if-open**; opened from the header gear, launcher gear, ⌘, and the menu.
- **Category nav** (Appearance · Data-display · System · Engine · Keymap) + the search box (P4-09).
- **Draft/save:** controls edit a local `Settings` draft; **Save** commits to the shared `applied`
  settings (all windows) + persists; **Cancel** / Esc-close discards.
- **Theme is live:** picking a theme / Sync-with-OS writes the shared `theme` signal so every window
  re-themes at once, still persisted only on Save, reverted on Cancel.
- Drop the "appearance & behavior" subtitle (U12 drift).

## Acceptance
- [ ] One settings window; re-invoking focuses it. Draft edits; Save applies app-wide + persists;
      Cancel/Esc discards. Theme changes preview live across windows.

## Freya / references
- Design: `Settings.dc.html`. DEV_TASKS W1/U12. Shared settings+theme (P4-01, `create_global`).
