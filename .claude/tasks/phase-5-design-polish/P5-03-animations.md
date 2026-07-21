# P5-03 · Animations & transitions

**Phase:** 5 · **Status:** ⬜ · **Depends on:** surfaces exist

## Goal
Add the motion the design implies — drawer/panel open/close, popover/modal in/out, tab drag, toasts.

## Current state
Surfaces mount/unmount without transitions.

## Build
- Use Freya `use_animation` / `use_animation_transition` for: drawer + side-panel collapse, the
  cell/row detail modals (P2-10/12) and other overlays, tab drag feedback, and any status flashes
  (e.g. settings-search field flash, P4-09).
- Keep them subtle and consistent (shared durations/easings).

## Acceptance
- [ ] Panels/overlays/tab-drag animate smoothly with consistent timing.

## Freya / references
- Freya `use_animation` / `use_animation_transition` (skill Animations; `animation_*.rs` examples).
