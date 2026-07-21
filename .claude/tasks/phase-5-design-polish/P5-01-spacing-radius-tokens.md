# P5-01 · Spacing & radius token scale across surfaces

**Phase:** 5 · **Status:** ⬜ · **DEV_TASKS:** F3 · **Depends on:** surfaces exist

## Goal
Every padding / gap / corner-radius across the Freya app snaps to the design's spacing + radius scale,
not ad-hoc literals.

## Current state
The Dioxus app did this as F3 (a `--sp-1..9` / `--r-xs..4` scale). Freya surfaces use `Gaps` /
`CornerRadius` values directly today; make them token-driven.

## Build
- Add the spacing + radius scale to the Freya theme (theme fields / consts) sourced from
  `Design.dc.html` §03 (`--sp-*`, `--r-*`).
- Sweep components/surfaces to pull padding/gap/radius from the scale; kill stray literals (keep the
  deliberate exceptions the design keeps literal, e.g. the mac traffic-light inset).

## Acceptance
- [ ] Padding/gap/radius come from the scale app-wide; a scale change reflows consistently.

## Freya / references
- Design: `Design.dc.html` §03 token scale. `theme.rs` / the JSON themes. DEV_TASKS F3.
