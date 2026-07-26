# P4-05 · Settings ▸ Data-display

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** U12 · **Depends on:** P4-03

## Goal
The Data-display category (grid/formatting prefs — e.g. default column width, null/date/timestamp
formatting, type-colour toggles).

## Current state
Not built. `format.*` prefs wire into the grid cell formatter.

## Wiring into the P4-03 shell
The Settings window shell is built: `Route::DataDisplay` renders `DataDisplayPane` in `apps/settings/mod.rs`, which
today is a `Pane::not_built(..)` placeholder. Replace that component's body; nothing else changes.

Every control edits `SettingsCtx::draft` (`use_consume::<SettingsCtx>()`) and stops there. The
footer's **Apply** is the only thing that commits — `write_config(.., &[ConfigChan::Settings], ..)`,
once, for the whole struct — so a page must never persist a field itself. The breadcrumb and the
scroll frame are the shell's; the pane renders content only, and reads its colours from the
`settings` component theme (`hint_color` is a setting's subtext).

## Build
- Render the data-display fields (numeric inputs / toggles / selects) editing the draft; on Save they
  apply to the grid formatter + defaults. Uniform divider-separated list (no ALL-CAPS section labels).

## Acceptance
- [ ] Fields edit the draft; Save applies to the grid (formatting, default col width, etc.).

## Freya / references
- Design: `Settings.dc.html` Data-display. Grid formatter (`CellFormat`). DEV_TASKS U12.
