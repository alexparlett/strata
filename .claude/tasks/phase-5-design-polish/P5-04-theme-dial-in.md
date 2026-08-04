# P5-04 · Theme dial-in (Midnight / Daylight)

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** W5 · **Depends on:** —

## Goal
Tune the Midnight/Daylight themes to match the canvases across every surface.

## Current state
The Freya theme system + both built-in themes exist; per-surface colour accuracy needs a pass once the
surfaces are built.

## Build
- Preview with the Freya component gallery / `FreyaThemeGallery.dc.html`; adjust the **roles**
  in both theme files (and, where a role is genuinely over-shared, split it in `roles!` + the
  mapping table) so each surface matches its canvas (Midnight = JetBrains-style tiers;
  Daylight = comfort zone). Attend first to the reconciliations theme v2 flagged for visual
  judgment: the translucent `elevated_element.hover`, the merged `accent.selection` intensity,
  `ghost_element.hover` absorbing the sidebar/launcher hovers, `data_type.timestamp` on tan,
  and the entity hues agreed between catalog and completion.
- **After any theme change**, regenerate + verify the schema:
  `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.

## Acceptance
- [ ] Both themes match the canvases across surfaces; `schema_in_sync` passes.

## Freya / references
- `themes/*.json` + `strata-core`'s `roles!` + `strata-freya/src/theme/components.rs`,
  `FreyaThemeGallery.dc.html`, the `schema_in_sync` test, `docs/FREYA_THEME_SPEC.md`.
  DEV_TASKS W5.
