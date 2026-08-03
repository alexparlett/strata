# Chart 02 · Chart body + plotters renderer

**Workstream:** Chart (Rz2) · **Status:** ✅ · **Depends on:** 01

## Goal
The Chart results body: a `canvas` drawing the five marks (bar/line/area/pie/scatter) plus the
histogram through `PlotSkiaBackend`, fed by a freya-query subscription over `Engine::chart`,
themed, with a mark picker. Replaces the placeholder tile in `results/chart.rs`. Spec:
`docs/CHART_SPEC.md` §2, §4, §9.

## Current state
Done. `results/chart/` is six modules — `mod.rs` (the body: roles, encoding defaults, the
subscription, the legend, the notice states), `strip.rs` (the mark picker + the legend),
`paint.rs` (the frame, the canvas, the hit map and the hover readout), `axis.rs` (the plotters
`Ranged` category coord, nice max, and the per-axis tick formatter), `marks.rs` (a render fn per
mark), `preview.rs` (an `#[ignore]`d headless render of all six marks to `target/chart-*.png` —
a chart is the one surface whose correctness is visual, and no unit test can see a legend that
has run off the pane).

Two fork changes, both in `freya-plotters-backend`: `draw_pixel` implemented (it was a reachable
`todo!()`), and `set_anti_alias` on `draw_line` / `fill_polygon` — only `draw_circle` had it, so
every diagonal line and every pie wedge was drawn hard-aliased.

## What it settled

- **A column's chart role is resolved from the Arrow `DataType`, in `column_info`.** `ColumnInfo`
  carries `role: ChartRole` beside `kind`, and `engine::catalog::chart_role` matches the type
  itself — the measure arm **is** `DataType::is_numeric`, the same predicate `engine::chart`'s
  read gates a Y on, so the encoder cannot offer a measure the read then refuses. A type's
  *spelling* is a rendering of a type, not the type, and `Kind` is the display taxonomy (it folds
  a union in with the strings), so neither is the source. Every `ColumnInfo` fixture in the
  workspace now goes through `column_info` for the same reason.
- **The chart read's cache identity is `(snapshot, query, display config)`.** Axis labels render
  through the engine's live `datafusion.format.*` (`CellFormat`), which `set_config` changes with
  no restart and no new snapshot — so `ChartSpec` carries `config::display_subset` of the app's
  engine overrides and a format change is a *new entry* rather than a stale one. Subscribed from
  the app config, not peeked off the engine: Freya's runner drains a write's dirty scopes before
  it polls the tasks queued alongside them, so `use_engine_config`'s `set_config` has landed by
  the time the capability runs.
- **A `canvas` repaints from a slot, not from its closure.** `RenderCallback`'s `PartialEq` is
  always-true, so the callback stored in the tree is the one from the first render — a closure
  that captured the frame would paint it forever. The frame goes into a `State` the callback
  peeks, and the side effect that fills it also asks the platform for a redraw
  (`feature_plot_3d`'s idiom). No fork-level revision key was needed.
- **A value axis and a measurement axis are two different axes.** `value_range` is read against
  zero, because the length of a bar and the height of a line *are* the magnitude. A scatter's X
  and Y are measurements — years, latitudes, prices — and `data_range` spans the data itself:
  the first build used the value axis for both and drew `year` in 2000..2024 on an axis of
  0..5 000, with every point inside one pixel column. The histogram is the mixed case and gets
  one of each: real bin edges across, counts from zero up.
- **An answer with nothing in it is a message, never a blank pane.** An empty bin set, an empty
  point set and a pie whose measure is all zero or missing each bail out *before* `ChartBuilder`,
  so the pane has no axes either — indistinguishable from a bug. `notice` is the one place that
  decides drawable-or-not, so a state cannot be drawable in one reading and blank in another.
  A pie also **refuses a negative value** rather than dropping it: percentages are read against
  a total, and quietly leaving a row out of that total is the silent truncation spec §1.4 rules
  out. A zero or a NULL is not the same thing — a zero-area slice is arithmetic.
- **The category axis is a plotters `Ranged`, not a hand-rolled tick stack.** `Categories` hands
  plotters its own key points, so every tick lands *on* a category and is labelled with that
  category's own text. True `Axis.positions` placement is taken only when the positions are
  present, finite and **strictly increasing** — the case where result order and value order are
  the same order; otherwise placing by value would silently re-order the axis spec §1.6 says is
  the user's.

## What review and first use settled about the axis and the legend

- **Abbreviation is a property of the axis, not of a value** (`axis::ticks(range)`). `2 000` is
  `2k` on an axis spanning thousands and a lie on one spanning 2 000..2 024, where all five
  gridlines abbreviate to the same `2k`. The unit is chosen once from the span and only when the
  span is at least one unit wide; below that ticks are written out with thousands separators.
  The **hover readout never abbreviates at all** — it is the one place the exact figure is being
  asked for.
- **A tick keeps two *significant* figures, not two decimal places.** Two absolute places erase a
  rate column outright: a 0..0.004 axis captioned every gridline `0`, and a tick at 0.025 read
  `0.03` — a gridline labelled with a number it is not.
- **An axis label is clipped to a tick's room (12 chars), not a cell's.** The engine's own clip is
  `DISPLAY_CHARS` (400), which under a tick is a wall of text.
- **The legend lives in the control strip, not on the plot** — a deliberate divergence from the
  design, which draws a key inside the canvas. A plot-overlay legend has nowhere to go when it
  outgrows its box: plotters sizes the box to its entries and draws it *inside* the plotting
  area, so four column names push it over the pane's edge and a 24-slice pie has no honest
  layout at all. The strip already scrolls, so the legend grows down instead of over. The pie's
  own wedge labels went with it — its names and shares are legend rows, which is also what stops
  24 slices becoming a pile of overlapping text.
- **The legend's rows come from the same walk the plot draws from** (`marks::pie_slices`), so it
  cannot name a colour the plot gave to another category.

## Hover

Every mark carries a hover readout (`purchase · amount: 2,480`, `login: 412 (38%)`), naming the
series because a grouped bar draws several bars per category and nothing else on the plot says
which is which. Two things worth keeping:

- **The hit regions are recorded by the paint that drew them**, through plotters' own coordinate
  mapping into an `Rc<RefCell<Vec<Hit>>>` — not recomputed for the pointer. A second copy of the
  layout arithmetic would be a second answer to *where is this bar*, and it drifts the first time
  a margin changes. (The headless preview has to force a paint before moving the cursor for the
  same reason; in the app a frame is always drawn first.)
- **The readout is the standard `Tooltip`, on `Layer::Relative(1)`.** The card is a built-in, so
  it carries the app's tooltip dress and its own width cap; the wrapper does nothing but place
  it. The layer is not optional — see AGENTS.md §3: a layer's nodes are an unordered set, and
  without it the readout painted *behind* the marks and read as though it had alpha.

## Ownership seams left open

- The **refusals** (`OverCap`, `Duplicates`) and the encodings a schema cannot satisfy render as
  a centred title + body `Notice`. The *Aggregate in SQL* CTA under them is **04's**, and lands
  as a button beneath the same copy — nothing else at those call sites changes.
- The **mark** lives in a `use_state` on the body. **03** moves it onto `QueryTab` under
  `Chan::Chart(tab)` with the rest of `ChartConfig`, which is what makes it survive a re-run;
  the strip's tile press becomes a write of `config.mark`.
- The strip carries `CHART TYPE` and `LEGEND`. X / Y / Series / Sort are **03's**, and go
  between them.

## Acceptance
- [x] Selecting Chart renders the current result with schema-default encoding; every mark
      renders correctly and re-renders on mark change, data settle, theme change, and resize.
- [x] NULL Y cells show as gaps, never interpolated; rows draw in result order; the chart reads
      every colour/font from the theme.

## References
`docs/CHART_SPEC.md` §9. Design visuals: handoff `Results.dc.html` + `screenshots/chart-*.png`.
Fork: `crates/freya/crates/freya-plotters-backend/`, `examples/feature_plot_3d.rs`,
`freya-components/src/canvas.rs`. `apps/project/query/run_query.rs` (`PageSpec` shape).
