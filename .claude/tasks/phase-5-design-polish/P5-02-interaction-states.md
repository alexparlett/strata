# P5-02 · Hover / focus / active interaction states

**Phase:** 5 · **Status:** ⬜ · **Depends on:** surfaces exist

## Goal
Consistent hover / focus / active / disabled treatments across all interactive elements.

## Current state
Individual components set their own hover colours; no systematic focus-ring / active pass.

## Build
- Drive interaction states from the theme (`Button`/`Input`/etc. `*ThemePreference` hover/focus fields;
  our own components' theme fields) so they're uniform and theme-aware.
- Keyboard **focus rings** on focusable elements (`use_focus` → render a ring on `Focus::Keyboard`).
- Verify against the canvases' hover/focus treatments.

## Acceptance
- [ ] Hover/focus/active/disabled look consistent and theme-driven; keyboard focus shows a ring.

## Freya / references
- Freya theming (`get_theme!`), `use_focus` / `Focus::Keyboard`. Design canvases' state styles.
