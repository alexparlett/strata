# P5-08 · Scroll acceleration for long lists

**Phase:** 5 — Design polish · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** —

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

## As built

In the fork, across `freya-core`, `freya-winit`, `freya-components` and `freya-testing`.

**The platform layer was discarding the fact the policy needs.** `renderer.rs` turned both
`LineDelta` and `PixelDelta` into pixels (×53 and ×2) and told nobody which it had been, so a
component could not tell a wheel from a trackpad. Deltas still arrive in pixels, but
`WheelEventData` now carries a `WheelGranularity` (`Line` / `Pixel`) alongside them, and
`WheelGranularity::LINE_SIZE` is the one place the notch size is named — `renderer.rs` pixelizes
with it, `accelerate_wheel_delta` recognises a whole notch with it. Verified against winit 0.30.13:
on macOS `hasPreciseScrollingDeltas` picks the variant, so a trackpad and a Magic Mouse report
pixels and a plain wheel reports lines.

**Only a whole line is accelerated**, which covers a second trap the task did not name: Windows
reports a precision touchpad as *fractional* `LineDelta` through `WM_MOUSEWHEEL`, so granularity
alone would have accelerated it. A sub-notch delta is left alone for the same reason a pixel delta
is: the system has already accelerated it.

**One event, one reading.** `WheelGestureClock::advance` now takes the event's own timestamp
(stamped once by the platform, carried on `WheelEventData`) rather than `Instant::now()`, and
returns a `WheelGesture { start, acceleration }`. This is the non-obvious part: `advance` is called
once *per view* an event propagates through, so measuring the gap against per-view arrival time
would read the second view's gap as zero and slam it to the ceiling. Keying on the platform
timestamp makes a repeat call exactly identifiable. For the same reason the a11y `scroll_to` pass
stamps all of its events alike: one focus movement is one event, not a burst of maximum-speed ones.

**A rate is only measured between events measured the same way.** `advance` also takes the
granularity and restarts the measurement when it changes, keeping the gesture's identity. Without
it a trackpad's momentum tail (pixels, every few milliseconds) leaves the reading at the ceiling
for the next wheel notch, and a single notch jumps a whole viewport.

**Latching got sharper, not just re-keyed.** `ScrollView`'s latch reads `gesture.start ==
e.timestamp` and its duplicate `advance` call is gone, which widens eligibility: previously each
view compared against its own `Instant::now()`, so only the innermost latching view could ever
latch. Now every latching view that saw the gesture's first event is a candidate and the innermost
one able to move takes it, so an outer latching view gets the gesture exactly when the inner
declined it. That is what the rule always said; `scroll_view_wheel_latching_nested` pins it.

**The curve and its two bounds.** Speed only, never list length: no acceleration at or below a 90ms
gap, ceiling at or above 15ms, squared ramp between, ×10 max. The first event of a gesture has no
rate to measure and is never accelerated, so one notch is always one notch. The tighter bound is
**a viewport per event** — the ceiling is there to keep the reader's place, and a small pane loses
it sooner than a large one, which a bare multiplier cannot say. An unmeasured viewport caps against
nothing rather than against zero. Both axes go through one `accelerate_wheel_movement` in
`scrollviews::shared`, so neither scroll view carries a copy of the rule.

Tests: unit tests on the curve, the clock and the delta rule in `scrollviews::shared`; integration
tests in `freya-components/tests/{scrollview,virtual_scrollview}.rs` asserting the same two-notch
gesture at 15ms vs 90ms vs a trackpad. `freya-testing` gained `scroll_lines` / `scroll_lines_at`
(the explicit timestamp is what keeps such a test independent of how long the test itself takes);
`scroll` is unchanged and stays pixel-granularity.
