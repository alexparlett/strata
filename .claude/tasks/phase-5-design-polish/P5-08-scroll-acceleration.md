# P5-08 · Scroll acceleration for long lists

**Phase:** 5 — Design polish · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** —

## Goal
A wheel scroll moves a fixed distance per notch, so a list of tens of thousands of rows cannot
practically be scrolled with the wheel at all — dragging the scrollbar thumb is the only way to
cross it. Found on P2-25's value tree with `config.json`'s `contentBlocks` open (19,311 sibling
keys, paged 100 at a time), and it applies equally to the results grid at a 1,000-row page and to
any long `VirtualScrollView`.

## Where the fix goes
**The fork**, not the app: `crates/freya/crates/freya-components/src/scrollviews/`. The wheel delta
is turned into a scroll position in `shared::get_scroll_position_from_wheel`, which every scroll
surface routes through — `ScrollView`, `VirtualScrollView`, and therefore the grid, the tree, the
drawer and the editor. Fixing it there fixes all of them at once; fixing it per surface would be
the workaround shape AGENTS.md §6 exists to prevent.

## What to build
Some care is needed to make this feel right rather than merely fast:

- **Acceleration is about gesture speed, not list length.** A long list scrolled slowly should still
  move slowly — the reader is looking for something. Scale the delta by how fast successive wheel
  events are arriving (there is already a `WheelGestureClock` in `scrollviews::shared` for the axis
  lock, so the timing is to hand), not by `length` or `inner_size`.
- **A trackpad is not a mouse wheel.** macOS already delivers accelerated, high-resolution pixel
  deltas for a trackpad; multiplying those again produces a surface that flies off at a flick.
  Check what winit reports (`MouseScrollDelta::LineDelta` vs `PixelDelta`) and accelerate the line
  deltas only — this is the trap most naive implementations hit, and Strata ships on the platform
  where it bites hardest.
- **Cap it.** Past some multiplier the list is a blur and the reader has lost their place; the
  ceiling matters more than the curve.

## Not in scope
Kinetic / inertial scrolling after the gesture ends is a different feature and a bigger one; it can
follow if this proves insufficient.

## Acceptance
- A fast wheel gesture crosses a 19,311-row tree (or a 1,000-row grid page) in a few flicks.
- A slow wheel gesture still moves at the current, readable rate.
- A trackpad two-finger scroll is unchanged in feel.
- The fork's own scroll examples still behave; the change is in `scrollviews::shared`, so it lands
  for every surface at once.
