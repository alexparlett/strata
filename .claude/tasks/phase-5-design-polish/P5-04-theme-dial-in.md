# P5-04 · Theme dial-in (Midnight / Daylight)

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** W5 · **Depends on:** —

## Goal
Tune the Midnight/Daylight themes to match the canvases across every surface.

## Current state
The Freya theme system + both built-in themes exist; per-surface colour accuracy needs a pass once the
surfaces are built.

## Build
- Preview with the Freya component gallery / `FreyaThemeGallery.dc.html`; adjust sheet + component
  tokens so each surface matches its canvas (Midnight = JetBrains-style tiers; Daylight = comfort zone).
- **After any theme change**, regenerate + verify the schema:
  `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.

## Acceptance
- [ ] Both themes match the canvases across surfaces; `schema_in_sync` passes.

## Freya / references
- `theme.rs` + JSON themes, `FreyaThemeGallery.dc.html`, the `schema_in_sync` test. DEV_TASKS W5.
