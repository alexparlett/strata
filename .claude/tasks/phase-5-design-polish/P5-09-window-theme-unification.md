# P5-09 · Window-theme unification (settings / export / launcher → `window`)

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** theme v2 (landed) ·
**Coupled with:** P5-10 (run this first — see below)

## Goal
The utility-window themes (`window`, `settings`, `export`, `launcher`) restate the same roles
under different field names. Migrate `settings`, `export` and `launcher` onto the shared `window`
component theme, keeping only genuinely window-specific fields on their own themes.

## Current state (verified 2026-08-12)
**Three windows to migrate, and the target is already proven.** The two newest utility windows —
the connection editor (`apps/connection/`) and the Configure window (`apps/configure/`) — declare
**no component theme group of their own** and read `window_theme()` at 15 call sites
(`connection/mod.rs:341`, `connection/views/{title_bar,form,status,footer}.rs`,
`configure/mod.rs:363`, `configure/views/{title_bar,status,options,paths,hive,footer}.rs`). They
are the pattern this task extends to the older three.

The duplication in the mapping table (`crates/strata-freya/src/theme/components.rs`):
`launcher` :296-307, `settings` :309-335, `export` :337-358, `window` :429-438. The shared rows
are exact — `border_fill` → `role(Role::Border)` in all four (:300, :312, :342, :433);
`icon_color`/`icon_background` → `Accent`/`AccentBadge` in three (:314-315, :345-346, :435-436);
the selection fields (`window.row_selected_background` :434, `settings.item_active_background`
:319, `settings.table_selection_background` :330, `export.card_active_background` :351,
`launcher.nav_background` :303) all → `AccentSelection`. The migration is already named at
`components/window.rs:20-21`.

Watch one seam: `window`/`export` put `background` on `ElevatedSurface` while
`settings`/`launcher` put it on `SurfaceRaised` (with their rails on `ElevatedSurface`) — the
same word means different elevations, so unification must pick per-field which window keeps an
override rather than averaging.

## Coupling with P5-10 (why this task runs first)
The connection/configure windows read `Role::Text`/`Role::TextMuted` directly beside their
`window_theme()` reads (`connection/mod.rs:342`, `connection/views/title_bar.rs:33`,
`connection/views/form.rs:721`, `connection/views/status.rs:35`, `configure/mod.rs:364`,
`configure/views/status.rs:34`, …). Today that is legitimate — `window` has no text fields. The
moment this task gives `window` text-tone fields, those reads become P5-10 violations.
**Decision: this task adds the text-tone fields to `window` and moves the connection/configure
reads onto them in the same change** — P5-10 then sweeps what remains, rather than re-homing
against a field set about to change. P5-10's file states the same hand-off.

## Build
- Move the shared fields to `window`; consumers (`window_theme()` + the ~29 call sites on the
  three window themes) re-point to it. Keep per-window fields (Settings' nav tree + cards +
  keymap slot, Export's format cards + warning banner, Launcher's title/label tones) on their
  own, smaller themes.
- Add the text-tone fields `window` needs and re-point the connection/configure direct reads
  (the coupling above).
- Delete each field from the mapping table as it collapses; the compiler enforces the remainder.

## Acceptance
- [ ] One `window` theme carries every shared field; the three window themes hold only fields
      no other window could name.
- [ ] The connection/configure text reads sit on `window`'s new fields, not on the roles.
- [ ] `cargo test -p strata-freya` green; visual pass on all six utility windows in both themes.

## Freya / references
- `crates/strata-freya/src/theme/components.rs` (the four groups), `components/window.rs`,
  `apps/settings/mod.rs`, `apps/export/mod.rs`, `apps/launcher/mod.rs`, `apps/connection/`,
  `apps/configure/`.
