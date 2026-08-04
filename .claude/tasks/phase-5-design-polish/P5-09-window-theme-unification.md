# P5-09 · Window-theme unification (settings / export / launcher → `window`)

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** theme v2 (landed)

## Goal
The four utility-window themes (`window`, `settings`, `export`, `launcher`) restate the same
roles under different field names. Migrate `settings`, `export` and `launcher` onto the shared
`window` component theme, keeping only genuinely window-specific fields on their own themes.

## Current state
Theme v2 made the duplication legible: in the mapping table
(`crates/strata-freya/src/theme/components.rs`) the four groups now sit side by side, and the
shared rows are exact — `border_fill` → `Border` in all four; `icon_color`/`icon_background` →
`Accent`/`AccentBadge` in three; the selection fields (`window.row_selected_background`,
`settings.item_active_background`/`table_selection_background`, `export.card_active_background`,
`launcher.nav_background`) all → `AccentSelection`. The migration was already named at
`components/window.rs:21` before v2; v2 removed the old blocker (these tones now resolve from
roles, not the palette-only namespace).

Watch one seam: `window`/`export` put `background` on `ElevatedSurface` while
`settings`/`launcher` put it on `SurfaceRaised` (with their rails on `ElevatedSurface`) — the
same word means different elevations, so unification must pick per-field which window keeps an
override rather than averaging.

## Build
- Move the shared fields to `window`; consumers (`window_theme()` + the ~29 call sites on the
  three window themes) re-point to it. Keep per-window fields (Settings' nav tree + cards +
  keymap slot, Export's format cards + warning banner, Launcher's title/label tones) on their
  own, smaller themes.
- Delete each field from the mapping table as it collapses; the compiler enforces the remainder.

## Acceptance
- [ ] One `window` theme carries every shared field; the three window themes hold only fields
      no other window could name.
- [ ] `cargo test -p strata-freya` green; visual pass on all four windows in both themes.

## Freya / references
- `crates/strata-freya/src/theme/components.rs` (the four groups), `components/window.rs`,
  `apps/settings/mod.rs`, `apps/export/mod.rs`, `apps/launcher/mod.rs`.
