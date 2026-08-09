# Chart 06 · Interactivity — bins, legend toggle, log axis, crosshair

**Workstream:** Chart (Rz2) · **Status:** ✅ · **Depends on:** 01–04 · Independent of 07–11.

## Goal
Four view-side controls that bring the chart to parity with standard data tooling, none of
which computes anything SQL can say: a histogram bin-count control, legend press to
hide/isolate a series, a log value axis, and a crosshair readout. Settled in planning
(2026-08-07, Alex): all four approved; hidden-set keyed by series *name* is accepted
coarseness; log never refuses.

## Current state
- The engine already honors `ChartQuery::Histogram { bins }` (`strata-core/src/engine/chart.rs`,
  `clamp(1, MAX_BINS)`, `MAX_BINS = 200`); `encode` always sends `None` — the strip has no
  control.
- The legend lives in the strip (`chart/strip.rs` `LegendRow`/`LegendEntry`), display-only.
- `marks.rs` hard-codes the Y coordinate as `RangedCoordf64` in
  `frame_on`/`mesh`/`zero_baseline`/`hit_box`/`hit_point`. Plotters 0.3.7 (locked) has
  `LogCoord`/`.log_scale()` whose `LogCoord<f64>` satisfies the same
  `Ranged<ValueType = f64> + ValueFormatter<f64>` bounds `mesh` already demands of X.
- `paint.rs` already has the anchored hover tooltip (`Hover` state, `Hits` slot,
  `on_pointer_move` with a `!=` guard, explicit redraw requests).

## Build

**a. Histogram bin control** (first — it exercises the ChartConfig → `encode` → `ChartQuery`
path b/c also ride):
- `strata-model/src/chart.rs`: `ChartConfig` gains `#[serde(default)] bins: Option<u16>`
  (int, hashable — the vocabulary's no-floats rule; persists via `TabSnapshot`).
- `chart/config.rs`: `Encoding` carries it; the `encode` Histogram arm sends
  `Some(usize::from(b).clamp(1, 200))`. Cache identity changes with the value — correct,
  it is a new read.
- `chart/strip.rs`: a "BINS" section only when the mark is Histogram — a compact numeric
  `Input` (placeholder "Auto"; empty/unparseable commits `None`), its own component so the
  strip's hook count stays fixed (the `SortToggle` precedent). Commit through the strip's
  single `commit()`; `InputTypography` around the `Input`.

**b. Legend toggle / isolate:**
- `ChartConfig` gains `#[serde(default)] hidden: Vec<String>` — series **names**, persisted
  like `sort`, never in `ChartQuery`. A stale name matches nothing and is harmless;
  `resolve` does **not** prune it (intent survives absence, like columns). Document on the
  field: a NULL-valued series and a literal `"(null)"` series toggle together — accepted.
- Application is a view transform beside `sort::sorted`: a hidden series keeps its slot and
  has its `values` blanked to all-`None`. Positions stay stable so `Dress::series(i)` colors
  don't shift, marks already draw `None` as nothing, and hit regions are built per finite
  value — **zero changes to `marks.rs`**. A grouped bar keeps an empty lane — accepted v1.
- `strip.rs`: `LegendEntry` gains `hidden: bool`; `LegendRow` becomes pressable — press
  toggles the name in `config.hidden`, ⌥-press isolates (hide all others; ⌥-press on the
  sole visible series restores all). Modifier state via the global key handlers, never the
  pointer event (AGENTS.md §3 — pointer events carry no modifiers). Dim hidden rows with the
  legend's existing muted tone — no new theme fields.
- Scope: multi-series marks (bar/line/area). Pie rows stay inert — hiding a slice silently
  recomputes every percentage, which the chart must not do.
- `notice()` in `chart/mod.rs` gains an "every series is hidden" arm, keyed off the
  *filtered* data, copy in the IDE register.

**c. Log value axis:**
- `ChartConfig` gains `#[serde(default)] log_y: bool` — a display transform in the sort's
  class: repaint, never a re-read, never in `ChartQuery`.
- Offered (a Linear/Log `SegmentedToggle` in the strip) only for line, scatter and histogram
  — gate predicate in `config.rs` beside `sortable`. Bars/areas read area-from-baseline,
  which a log axis has none of; pie has no axis.
- `marks.rs`: genericize the Y coordinate over `Ranged<ValueType = f64> +
  ValueFormatter<f64>`; build with `(lo..hi).log_scale()` when on; skip `zero_baseline`;
  compute the range over **positive finite** values only and bypass the linear
  `EDGE_AIR`/`nice_max` padding arithmetic in log mode (verify this interaction early — it
  is the item's one real risk).
- Values ≤ 0 present → draw **linear** and show the existing non-blocking `Banner`
  ("Values at or below zero are shown on a linear axis") — never a refusal.

**d. Crosshair readout** — `paint.rs` only, no repaint per mouse move:
- A `Mapping` slot beside `Hits` (same `Rc<RefCell<…>>` pattern): plot pixel rect
  (plotters' `plotting_area().get_pixel_range()`) + value ranges + log flag, recorded by
  each cartesian mark after `frame_on`; pie records `None`.
- `ChartCanvas`: pointer position in a `use_state`; render 1px vertical + horizontal
  `rect()`s (`Layer::Relative(1)`, non-interactive) clipped to the plot rect, plus a small
  readout label at the plot edge — Y value by inverse mapping (linear or log), formatted
  through `axis::readout`. If per-sample label re-render measures badly, quantize to whole
  pixels.

## Acceptance
- [x] A bin count re-reads and redraws; Auto restores the engine's `√n` choice; the value
      persists per tab.
- [x] Legend press hides/shows without any color shifting; ⌥-press isolates; pie legend is
      unchanged; all-hidden shows the notice.
- [x] Log axis on a positive series redraws with decade ticks and no baseline; a series with
      values ≤ 0 stays linear under the banner; flipping the toggle never re-reads.
- [x] Crosshair tracks over cartesian marks only, readout agrees with the axis format, and
      the canvas does not repaint on mouse move.
- [x] `cargo build` + `cargo test -p strata-freya` green; preview harness
      (`chart/preview.rs`) gains log + crosshair + hidden-series fixtures
      (`chart-hidden-series`, `chart-log-histogram`, `chart-log-refused`, `chart-crosshair`).

## What was built, and where it differs from the plan

Landed as planned except for the nine below. Five of them came out of the `adversarial-review`
pass, which returned **BLOCK** on the first cut (5 critical / 10 warning / 4 note); the rerun
notes below are what those findings changed. The rules are in `docs/reference/INVARIANTS.md`
(the chart entry, "the interactivity pass"), one-lined in AGENTS.md 2, and spelled out in
`docs/CHART_SPEC.md` 6/7/9.

- **The bin cap is shared, not restated.** `engine::MAX_BINS` is now `pub` and the encode site
  clamps to it, rather than a literal `200` in the UI — a box that accepts 5 000 over a read
  that answers 200 shows one thing and means another.
- **The log axis is one `Ranged`, not a type parameter.** `axis::ValueCoord` has a linear and a
  log arm, so `mesh` / `hit_box` / `zero_baseline` / `frame_on` keep one Y type and no mark had
  to split into a build half and a draw half. `Categories` was already the precedent.
- **`log_span` takes the *next* decade out when a bound already sits on one.** The plan said to
  bypass the linear padding; that alone floors a long-tail histogram at exactly 1 and draws
  every count of 1 as a bar of no height. This is the log version of `EDGE_AIR`.
- **A histogram's empty bins do not block the log axis.** The plan's rule is "any value at or
  below zero draws linear under the banner". Applied literally to counts it would take the log
  scale away from nearly every long tail, and a zero-count bin paints nothing on either axis —
  so `chart/mod.rs::log_blocked` answers `false` for `Bins` and keeps the literal rule for the
  shapes whose zeros *are* drawn (tables and points). Flagged as a judgment call.
- **Hiding is applied *after* the sort** (`hide::applied` over `sort::sorted`), and it is its
  own module rather than living in `sort.rs`. Hiding the first series before a `ByYDesc` sort
  would reshuffle the whole category axis — visibility has to be the last thing that happens.
- **The crosshair's pieces are absolute siblings of the plot**, not children of a wrapper. A
  wrapper is a *stacked* sibling of a fill-height plot, so its area starts below the canvas;
  measured, the horizontal rule landed one whole pane below the pointer. Pinned by
  `paint.rs::a_crosshair_rules_through_the_hovered_mark_and_reads_its_row_off_the_axis`, which
  asserts the hairlines' geometry — a unit test on the mapping alone did not catch it.
- **The crosshair rules through the hovered mark, not under the pointer.** The plan's shape —
  pointer position in a `use_state`, quantized to whole pixels if it measured badly — measures
  *worse* than badly: Freya has no incremental rendering (`render_pipeline.rs` repaints every
  node every frame) and `CanvasElement::render` calls `on_render` on each pass, so every
  reactive write re-runs `marks::draw`, a full plotters replot plus a rebuild of every hit
  region, on the render thread. Quantizing to whole pixels still leaves ~600 replots crossing
  the plot. Riding on `hover` instead honours the plan's own acceptance line ("the canvas does
  not repaint on mouse move") and costs nothing over today, for the same reason `Hit::anchor`
  exists. **The trade-off is real and worth a look:** the value axis can now only be read at a
  mark, not at an arbitrary height. Reading it anywhere needs incremental rendering in the fork
  (AGENTS.md §6), which is a task of its own.
- **The value is carried on the `Hit`, not inverted out of the pixel row.** Round-tripping
  value → pixel → value through integer pixels put `11.01` under a tooltip reading `11`. That
  removed `Mapping`'s value range and log flag entirely; what is left is `PlotArea`, the rect
  the rules are clipped to.
- **`resolve` gates `hidden` by the mark**, the way it already gates `bins` and `log_y`. A pie's
  Y is an ordinary measure a bar may have hidden earlier, and a pie's legend rows are inert —
  honouring the set there blanked the pie with no control on screen to bring it back.
- **The legend is built whether or not a notice replaces the plot.** Built only on the drawable
  path (the obvious place, since that is where the data is), it vanished exactly when the
  all-hidden notice told the user to press it — and `hidden` is persisted, so the tab carried
  that dead end across a re-run and a restart. Over a single-measure result there was no way
  out of it at all.
- **⌥-press edits the hidden set rather than rebuilding it from the current legend**, so a name
  this result cannot answer survives the gesture the way it survives an ordinary press. The two
  legend gestures now agree about the same field.
- **The bin box bounds its input and re-echoes on blur**, and parses wide before it clamps.
  Without the first two it showed `5000` over a 200-bin chart — the "shows one thing and means
  another" failure the shared `MAX_BINS` was introduced to prevent (AGENTS.md §3, and
  `NumberField`'s own contract). Without the third, a count over 65 535 failed a `u16` parse and
  read as Auto rather than as the cap.
- **`log_span` refuses a span whose *ratio* overflows, and the banner says which reason it
  was.** A log axis is bounded by `end/start`, not `end - start`: `LogCoord::key_points` turns
  an overflowed ratio into a `usize::MAX` bold-tick count and then counts it down one at a time
  on the render thread — a frozen window, from a column holding both 1e-300 and 1e300. The
  guard needed a second banner message, so `log_blocked -> bool` became
  `log_fallback -> Option<&'static str>`.

## References
`docs/CHART_SPEC.md` §6 (sort as the view-transform precedent), §9. AGENTS.md §3 (modifier
tracking, one handler per event name, stable hook counts). `docs/reference/FREYA_UI.md`.
