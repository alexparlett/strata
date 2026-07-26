# P4-04 · Settings ▸ Appearance

**Phase:** 4 · **Status:** ⬜ · **DEV_TASKS:** U12 · **Depends on:** P4-03

## Goal
The Appearance category: theme selection (live) + related display prefs.

## Current state
The **shell is up** (P4-03): `Route::Theme` renders `ThemePane` in `apps/settings/mod.rs`, which
today is a `Pane::not_built(..)` placeholder. Replace that component's body — nothing else changes.

**The live-theme mechanism is already built and tested**; this task is the control that drives it.
Write `SettingsCtx::draft`'s `theme` / `sync_os` (`use_consume::<SettingsCtx>()`) and the window's
root effect mirrors them into the app-global `ThemePreview`, which every window's
`use_strata_theme` resolves ahead of the committed settings. Do **not** write the preview slot
directly, and do not reach for `write_config` — Apply is the footer's, and it is what persists.
Pinned by `theme::tests::a_preview_outranks_the_committed_theme_until_it_is_dropped`.

The breadcrumb (`Appearance & behaviour › Theme`) and the scroll frame are the shell's — the pane
renders content only.

**This task makes the draft editable, which arms a gap P4-03 left**: `SettingsCtx::apply` writes
the *whole* `Settings` struct, so a setting another window commits while Settings is open is
reverted on Apply (P4-03's file has the worked example — the T2 confirm's "Don't ask again").
Settle it here, before any pane can dirty the draft: either re-seed untouched fields from the
store on commit, or commit a per-field diff against the seed.

## Build
- Theme cards (Midnight / Daylight / any custom) with a **source badge** (`ThemeEntry::source`,
  from the shared `ThemesCtx` registry — `themes.entries()`); selecting one writes the draft's
  `theme`. **Sync with OS** toggle writes `sync_os` (follows the OS dark/light; while it is on the
  cards are inert, as the canvas shows).
- Match the canvas structure (Appearance already matched structurally in the Dioxus app).
- Colours come from the `settings` component theme; `hint_color` is each setting's subtext.

## Acceptance
- [ ] Selecting a theme previews live across windows; Sync-with-OS follows the OS; persists on Save.

## Freya / references
- Design: `Settings.dc.html` Appearance. Themes from `theme.rs` / `FreyaThemeGallery.dc.html`. DEV_TASKS U12.
