# P5-03 · Animations & transitions

**Phase:** 5 · **Status:** ⬜ · **Depends on:** P5-01 (co-locate the shared timing consts)

## Goal
Add the motion the design implies — dialog/popover in/out, drawer/panel open/close, status
flashes — subtle and on one shared set of durations/easings.

## Current state (verified 2026-08-12)
**Exactly one app-authored animation exists**: the settings-search reveal flash
(`components/form/row.rs:11,139-144,164` — `AnimColor`, `Ease::Out`, `OnChange::Finish`). No other
`use_animation` / `AnimatedPosition` / `use_animation_transition` anywhere in `strata-freya`.

- **Dialogs**: `components/dialog.rs` hand-rolls its modal on `PopupBackground` (:287) + a
  `Layer::Overlay` wrapper (:258-260) — two static rects. The fork's own `Popup` *does* animate
  (backdrop `AnimColor` 0→150 alpha over 150ms, `popup.rs:181-187`; `AnimNum::new(0.85, 1.)`
  scale + opacity over 250ms, :191-197) and has **zero** call sites in the app — the hand-rolled
  Dialog forfeits it.
- **Menus**: 23 `Menu::new()` call sites; the fork `menu.rs` has no animation (its `opacity` at
  :308-324 is a pre-measure hide, not a fade). A menu fade is a **fork** change if wanted.
- **Drawer / panels**: resized through the layout controller as a hard layout change
  (`drawer/mod.rs:103-104`); the sidebar's filter↔label swap at `CATALOG_FILTER_MIN` is an
  instant branch.
- Inherited motion (fork components animating internally): `CircularLoader` (11 sites), `Select`,
  `Tooltip`, `Switch`, `Checkbox`, `Accordion`, `Progressbar`, `Skeleton`, `CursorBlink`.

## Build
- **Shared timing consts** (durations + easings) in P5-01's metrics module — the fork's own
  150ms/250ms `Popup` pair is the reference; the canvases carry no motion spec (`Strata.dc.html`
  declares `min-width` and has no transitions to port), so the fork is the source of truth here.
- **Dialog in/out**: give Strata's `Dialog` the fork `Popup`'s motion — prefer reusing the fork's
  shape (either adopt `Popup` itself if its anatomy fits Dialog's header/body/footer contract, or
  port the same backdrop-fade + scale pair into `dialog.rs`) over inventing new timings.
- **Drawer + side-panel collapse/expand**: animate the transition with `use_animation` on the
  panel size, terminating in the same layout-controller write that exists today. Respect P5-06's
  settled model — the animation dresses the open/close *gesture*; reflow under pressure stays
  instant (a squeeze is not a gesture).
- **Status flashes**: the form-row flash pattern is the template; reuse its shape for any other
  flash the canvases imply.
- Menu fade: optional, and a **fork** change if taken (upstream-shaped, in `menu.rs`) — never an
  app-side wrapper.

## Acceptance
- [ ] Dialogs and panel open/close animate; timings come from the one shared const set.
- [ ] Reflow-under-pressure (P5-06) remains instant; only user gestures animate.
- [ ] Any fork change pushed to the fork remote (gitlink).

## Freya / references
- Fork `popup.rs` (the 150/250ms pair), `use_animation` (skill Animations; `animation_*.rs`
  examples). `components/form/row.rs` (the one existing flash). P5-06's model in
  `views/shell.rs`'s module doc.
