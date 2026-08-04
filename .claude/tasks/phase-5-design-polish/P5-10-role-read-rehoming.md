# P5-10 · Role-read re-homing (component-themed surfaces off the direct reads)

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** theme v2 (landed)

## Goal
Enforce FREYA_UI's "a surface with its own component theme reads colours from that theme, not
also from the roles": ~52 non-semantic direct `use_roles().get(Role::…)` reads sit inside
surfaces that have a `define_theme!` of their own. Each becomes a field on the owning component
theme (a new mapping-table row), or reuses a field the theme already has.

## Current state
Theme v2's rename pass moved every read onto the role vocabulary but deliberately did **not**
re-home them — the vocabulary changed, the architecture violation stayed. The audit's worst
offenders (all counts are role reads inside a themed surface): `apps/launcher/views/row.rs`
(10), `apps/settings/views/keymap/table.rs` (5), `apps/settings/views/engine/table.rs` (3), the
close/drop confirm dialogs (7 between them, on `cancel_button`), `apps/launcher/views/projects.rs`
(3), `apps/settings/views/engine/mod.rs` (3), and one-liners in `export`, `agents`, `status_bar`,
`cell_view`/`record_view` (their `shadow`), `tab_bar/bar.rs` (`DropTarget`), `tab.rs`
(`TextDisabled` for the close ×). `components/divider.rs` is the one sanctioned dual-read
(hooks run unconditionally; the role picks after) — leave it.

Semantic reads through `tones()` are correct and stay.

## Build
- Per surface: add the missing field(s) to its `define_theme!`, add the mapping-table row
  (`theme/components.rs`) targeting the same role the direct read used, and swap the call site
  to the theme field. Name fields for the role they play, not the first consumer (FREYA_UI).
- Where the surface's theme already has a field resolving to the same role, reuse it instead of
  adding one.

## Acceptance
- [ ] `grep -rn "use_roles()" crates/strata-freya/src` hits only: un-themed surfaces,
      `components/tones.rs`, `components/divider.rs`, and `theme/`.
- [ ] `cargo test -p strata-freya` green; `schema_in_sync` untouched (roles didn't change).

## Freya / references
- `docs/reference/FREYA_UI.md` ("reads colours from that theme"), `theme/components.rs`,
  the file list above.
