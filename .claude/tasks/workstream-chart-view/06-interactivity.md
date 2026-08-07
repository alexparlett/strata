# Chart 06 · Interactivity — bins, legend toggle, log axis, crosshair

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01–04 · Independent of 07–11.

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
- [ ] A bin count re-reads and redraws; Auto restores the engine's `√n` choice; the value
      persists per tab.
- [ ] Legend press hides/shows without any color shifting; ⌥-press isolates; pie legend is
      unchanged; all-hidden shows the notice.
- [ ] Log axis on a positive series redraws with decade ticks and no baseline; a series with
      values ≤ 0 stays linear under the banner; flipping the toggle never re-reads.
- [ ] Crosshair tracks over cartesian marks only, readout agrees with the axis format, and
      the canvas does not repaint on mouse move.
- [ ] `cargo build` + `cargo test -p strata-freya` green; preview harness
      (`chart/preview.rs`) gains log + crosshair + hidden-series fixtures.

## References
`docs/CHART_SPEC.md` §6 (sort as the view-transform precedent), §9. AGENTS.md §3 (modifier
tracking, one handler per event name, stable hook counts). `docs/reference/FREYA_UI.md`.
