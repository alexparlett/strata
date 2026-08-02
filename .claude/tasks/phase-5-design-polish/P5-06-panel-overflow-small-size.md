# P5-06 · Panel overflow & small-size behaviour

**Phase:** 5 · **Status:** 🟡 · **DEV_TASKS:** — · **Depends on:** P3-01 (shell) + the content tasks

## Goal
One consistent rule set for how every region degrades when the window runs out of room: what gives
way, what folds, what scrolls — instead of clipping, spilling, or being crushed.

## The model (settled — do not re-litigate)

The reference is **RustRover/IntelliJ**, not the design canvas. `Strata.dc.html:22` declares
`min-width: 1180px` on the app root and scrolls the page below it, so the canvas has **no narrow
states at all** and nothing to port. JetBrains states the opposite of a minimum outright: *"it is
not possible to enforce minimal tool window size, and it is up to users to resize it to their
needs"*. A RustRover window squeezed to ~680px keeps both tool windows **open** (wrapping their
text) while the editor between them is a ~45px stub, both rails uncompressed, the status bar
clipped from the left, and the top toolbar reduced to two ellipsized chips.

Five rules, in order. The full text lives in `views/shell.rs`'s module doc, which is where a
reader looks for the frame's contract:

1. **Nothing has a usability floor; everything has a stub floor.** A stub exists so a panel cannot
   become a sliver too thin to grab, not to keep it useful.
2. **Space is given up in a stated order** — the proportional main pane first and entirely, then
   the pixel side panels in equal measure. That order *is* the sizing model, not a policy.
3. **Chrome shrinks its flexible run, then folds its actions into `⋯`, then drops them.**
4. **Pressure never collapses a panel; only a drag does.**
5. **A body scrolls; chrome never does.** Vertically only — a long identifier ellipsizes.

Anti-overlap rests on four checkable properties, not on arithmetic closing against a minimum:
a flex panel is never measured negative; pixel panels reflow rather than overflow; every chrome row
folds or ellipsizes inside its own box; every panel rect is `Overflow::Clip`.

## Built

**Fork** (`crates/freya`, 3 files — push the gitlink):
- `torin/src/measure.rs` — **`flex_available_*` clamped at zero.** It could go negative, so a flex
  child was measured past its own origin and painted over its siblings. The single most direct
  cause of overlap. Covered by `flex_does_not_measure_negative_when_siblings_overflow`
  (`torin/tests/flex.rs`), which was verified to fail without the clamp (`left: -100.0`).
- `resizable_container.rs` —
  - `min_size` now reaches the **layout node** (`min_width`/`min_height`), not just the drag clamp.
  - `ResizablePanel::max_size` — the canvas's 520 / 560 / 480 / 680, which had nowhere to live
    (the fork gap P3-01 parked).
  - `ResizablePanel::min_pixels` — the container-pressure floor, always in pixels. Separate from
    `min_size` **because a flex weight cannot say how many pixels a panel needs**; for a pixel
    panel the two coincide and it defaults across.
  - `Panel::desired` vs `Panel::size` — what the user asked for vs what fits. `ResizableContext::reflow`
    re-derives `size` from `desired` on every container measurement, so shrinking squeezes and
    growing restores. Six unit tests.
  - `ResizablePanel::on_resized` (drag-sourced only) and `on_collapse` (drag past the floor),
    plus `ResizeOutcome` so a clamped drag reports the over-drag instead of discarding it.

**App:**
- Window minimum **880×600 → 360×240** (`project.rs`).
- `components/toolbar.rs` — the shared fold mechanism. Leading run + ranked items + pinned tail;
  an item is declared once and knows its width, its inline form and its menu-row form; `fold_plan`
  is arithmetic over the item list, so adding a button moves the fold point with nothing restated.
  Ten unit tests.
- Shell: stub floors, canvas maxes, `min_pixels` on the two proportional panels, `on_resized`
  replacing the `on_sized` probes, `on_collapse` into the existing close funnels.
- Editor toolbar and results toolbar moved onto `Toolbar`; inspector and drawer headers given
  rule 3's structure (`Content::Flex` + a flexing ellipsizing title) — they were `SpaceBetween`
  over `Content::Normal` with no clip, so their clusters drew over each other.
- `DrawerEmpty` made scroll-safe.
- `the_header_stays_inside_the_panel_at_its_stub_width` (inspector tests) sweeps 84 → 292px.

### Corrections this task carried
- **The task's own premise was wrong**: "softer mins + graceful degradation" understates it. The
  answer is **no usability minimums at all**.
- **`give_order` was designed and then dropped.** The give-order falls out of the sizing model
  (proportional gives first, pixel panels give equally), so the knob would have been unreferenced
  pre-work (AGENTS.md §5).
- **The drawer-expand clamp was dropped too**: `reflow` already bounds a 560px expand against the
  room above it, so a separate clamp would be a second copy of the rule.
- Three surfaces the old task file named **do not exist**: no drawer tab strip or filter box (the
  rail *is* the switcher), no inspector tabs, no app-level status bar. The bodies already scroll.

## Left to do
- [ ] **Status-bar pager** (`results/status_bar.rs`) onto `Toolbar`. Ranks exist for exactly its
  ladder (size dropdown → jump box → First/Last → Prev/Next). Needs the row itself to become the
  `Toolbar` with the info cluster as its leading run, which means hoisting the jump-input state out
  of `PagerCluster`. Today it clips at the panel edge rather than overlapping, so this is polish.
- [ ] **Explain toolbar** (`explain_plan/mod.rs`) and the **sidebar header** onto `Toolbar`. The
  sidebar header is already structurally correct; it just doesn't fold.
- [ ] **Find's popover anchor.** Find is a `Custom` item with no folded form, so at a stub width it
  drops and ⌘F (handled in the datagrid) toggles state nothing renders. The fix is to host the
  popover on the results pane root rather than on the toolbar trigger — a change to the popover
  stack, which is high-risk ground (AGENTS.md §8) and wants its own pass.
- [ ] **Inspector body at stub widths.** An identifier like `customer_shipping_address_line_one`
  has no break opportunity, so its run is wider than an 84px panel whatever the layout does, and
  centred rows then start at a negative x. Wants a character-level ellipsis or a horizontally
  clipping body — a content decision, which is why the stub-width test is scoped to the header.
- [ ] Manual pass on a Mac build (see the plan's verification list).

## Findings routed elsewhere
- The three panel headers are **48 / 40 / 36px** and each documents itself as matching the other
  two. Token/drift question → **P5-05** or **P5-01**.
- Icon-button sizes come in **six** (30 / 28 / 24 / 22 / 20 / 16) with `TOOL_SIZE = 28.` the only
  shared constant. Same routing.

## References
- `views/shell.rs` module doc (the five rules), `components/toolbar.rs` module doc (the fold).
- [Toolbar — IntelliJ Platform SDK](https://plugins.jetbrains.com/docs/intellij/toolbar.html),
  [tool window minimum size — JetBrains](https://intellij-support.jetbrains.com/hc/en-us/community/posts/23267015298194-How-to-set-the-minimum-width-of-the-Tool-Window).
