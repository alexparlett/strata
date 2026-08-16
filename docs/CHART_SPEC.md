# Strata — Chart view

The results **Chart** surface, as built: what it does, where its data comes from, and how it
renders. The chart is a second body for the results pane — a plot of the same immutable snapshot
the grid pages — behind the toolbar's Table/Chart toggle. The designer's handoff bundle
(`screenshots/chart-*.png`) remains the **visual** reference for layout and control dress.

---

## 1. Principles

1. **Chart the result, not the files.** The chart is a view of the current result set — the same
   immutable snapshot the grid pages (`docs/SNAPSHOT_SPEC.md`). It never queries source files;
   a new Run reads the new snapshot.
2. **The chart computes nothing SQL can say** (renderer-first — the Athena/Snowsight shape). SQL
   is the aggregation language: it already has every `GROUP BY`, `date_bin`, percentile and
   window function the chart could ever grow, the user can read and edit it, and the editor is
   two keystrokes away. The chart maps result **columns onto marks**; it never aggregates,
   buckets, or re-orders behind the user's back. Switching mark, encoding or sort is a repaint,
   never a re-query.

   This is a settled decision, not a preference: an engine-side aggregation pipeline was built,
   reviewed and withdrawn, and the principle must not be re-litigated. The short form:
   a `GROUP BY` has no output order, so re-aggregating an already-shaped result structurally
   destroys the order the user asked for; a renderer cannot lose what it never recomputes.

   One deliberate exception: the **histogram** (§5) bins engine-side — binning a raw column
   needs a min/max pass and DataFusion 54 has no `width_bucket`. It is the only mark that
   computes.
3. **No shadow query language.** No aggregate menu, no bucket control, no engine algebra.
   Aggregation is the user's own SQL, in the editor (§8).
4. **Honest boundaries.** Never silently sample, truncate, or aggregate. Above a cap, or when
   the data's shape doesn't fit the mark, the chart refuses, and the message names the fix (§7).
5. **Rendering is `freya-plotters-backend`** (fork, `plot` feature) on a `canvas` — plotters'
   `ChartBuilder`/series machinery, never a hand-rolled axis/tick/mark stack (§9).
6. **Result order is the axis order, and it is real.** Rows draw in the order the user's query
   produced them, backed by the snapshot ordinal (`SNAPSHOT_SPEC.md` §9) — never scan order,
   which is measured-nondeterministic above 10 MB. Re-ordering is the strip's explicit sort
   toggle (§6), chosen by the user, never imposed by the engine.

## 2. Where it lives

The results toolbar carries a Table/Chart `SegmentedToggle`. The mode is **per tab** —
`ResultsView` on `Chan::View(tab)`, persisted in the tab's `TabSnapshot` — so switching tabs
restores it and it survives re-runs and a restart. Find is grid-only; Reload and Download ride
the shared toolbar in both modes (`results/toolbar.rs`).

The chart body (`results/chart/`) is two panes under that toolbar:

- **Left control strip** (232 logical px, its own scroll — `chart/strip.rs`): the mark tiles
  (six, three to a row), the X / Y / Series encoders, a histogram's bin count, the sort and scale
  toggles, and the legend.
- **Right canvas pane**: the plot, a non-blocking high-cardinality banner across the top when
  warranted, and a refusal notice in place of the canvas when there is nothing honest to draw.

## 3. Column roles (from the Arrow type)

Each result column carries a `ChartRole` beside its `Kind`, derived from its Arrow `DataType`
where that type is still in hand: `strata_arrow::column::chart_role`, called by `column_info`. A role
comes from the type — never from a column's name, and never from a type's *spelling* (which is a
rendering of a type, not the type). The measure arm **is** `DataType::is_numeric`, the same
predicate the read gates a Y on, so an encoder cannot offer a measure the read then refuses.
Roles drive the encoder menus and the defaults — they never change what the engine computes,
because it computes nothing.

| role | DataTypes |
|---|---|
| **Measure** (Y, scatter axes, histogram value — valid on X too) | anything `is_numeric` (Int*/UInt*/Float*/Decimal*) |
| **Instant** (X; defaults to line) | Date32/Date64/Timestamp* |
| **Clock** (X; defaults to line) | Time32/Time64 |
| **Dimension** (X, series) | Utf8/LargeUtf8/Utf8View/Boolean/Dictionary |
| **Other** — offered nowhere | everything else: nested, binary, interval, duration, … |

A dictionary is a dimension whatever it encodes: it is a category by construction, and a
dictionary of numbers is not a measure the read accepts. `Other` is the safe default in the
direction that matters — a variant Arrow grows later is excluded from the encoders, not
mis-plotted.

**Instant and clock are one thing on an axis and two in SQL.** Both order, both default to a
line, and the encoders read them together (`config::is_time`). They are separate roles because
they differ wherever a **stride** does: `date_bin(interval '1 day', …)` is a coarser reading of
a calendar instant and is refused outright over a time of day ("DATE_BIN stride for TIME input
must be less than 1 day"). Nothing reads the distinction today; it is kept because chart-side
bucketing needs it, and the only way to recover it later is a type's spelling — the thing this
taxonomy exists to rule out. A Utf8 column that holds a timestamp is a **cast** the user makes
in SQL.

## 4. Marks and encodings

Six marks (`strata_model::ChartMark`), in the picker's order: **Bar, Line, Area, Scatter,
Histogram, Pie**.

| Mark | X | Y | Series | Notes |
|---|---|---|---|---|
| Bar | any column, or none (row index) | one or more measures | optional dimension | grouped bars |
| Line / Area | any column, or none | one or more measures | optional dimension | NULL Y cells are gaps, never interpolated |
| Pie | dimension or temporal | exactly one measure | — | cap 24 slices |
| Scatter | measure | measure | — | raw points, non-finite coordinates dropped |
| Histogram | — | measure (the value) | — | engine-binned (§5) |

Three rules make the encoding model:

- **Multiple Y columns are multiple series**, named by column — `SELECT month, revenue, cost …`
  is two lines with no configuration. Analytical presets arrive the same way: a candlestick is
  four Y columns in named roles, not an engine computation (§10).
- **A series column pivots long → wide**: rows `(x, series, y)` become one series per distinct
  series value, named by value (`value: column` when there are also multiple Y columns). The
  pivot is a reshape, not an aggregation — and it is the **only** operation that can conflate
  rows, so it is the only thing that refuses on duplicates (§7).
- **Without a series column there is no pivot**: each row is its own mark in result order.
  Duplicate X labels draw as duplicate marks — the chart shows what the result holds.

The per-mark option sets live beside the strip (`chart/config.rs`: `x_options`, `y_options`,
`series_options`, `allows_row_index`, `takes_many_ys`, `sortable`) and both the menus and the
resolution validate against them — so an encoding a mark cannot take is **unreachable rather
than reported**: no control ever offers the column. A scatter has no series row; a pie's Y
replaces instead of accumulating; a histogram shows no X row at all.

## 5. Data: `Engine::chart` over the snapshot

One engine method (`Engine::chart` → `engine/chart.rs::run_chart`), one freya-query capability
in front of it (`query/chart.rs`), no confirm — a projected, capped read of a local snapshot is
`fetch_page`-tier work. The call holds the snapshot pin for its own duration (a histogram is two
passes, and a re-run between them must not retire the table mid-call).

```mermaid
flowchart LR
    S[("snapshot (Arrow IPC)")] -->|"projection, ORDER BY __strata_ord, LIMIT cap + 1"| R["Engine::chart"]
    C["ChartConfig (per tab)"] -->|"resolve + encode (chart/config.rs)"| Q[ChartQuery]
    D["display config (datafusion.format.*)"] --> K
    Q --> K["cache key: snapshot + query + display"]
    K --> R
    R -->|"pivot long→wide (Rows only)"| A[ChartData]
    A -->|"sort: view transform"| M["marks.rs / plotters"]
    M --> P[Skia canvas]
```

**Cache identity is `(snapshot, query, display config)`** — `ChartSpec` in `query/chart.rs`,
`stale_time(MAX)`. The third key is not optional: axis labels render through the engine's live
`datafusion.format.*` overrides (`CellFormat`), which Settings changes with no restart and no new
snapshot, so an entry keyed on the first two alone would serve labels rendered under a format the
user has since changed. `ChartSpec.display` carries `config::display_subset` of the app's engine
overrides, which makes a format change a new entry rather than a stale one.

**Vocabulary (`strata_model::chart`).** All hash/eq-able — `ChartQuery` is cache identity, built
in exactly one place (§6).

```
ChartQuery::Rows      { x: Option<String>, ys: Vec<String>, series: Option<String>, cap }
ChartQuery::Raw       { x, y, cap }                      // scatter
ChartQuery::Histogram { col, bins: Option<usize> }       // the one computed mark

ChartData::Table  { axis: Axis, series: Vec<ChartSeries> }
ChartData::Points (Vec<ChartPoint>)                      // unordered; a scatter draws marks
ChartData::Bins   (Vec<ChartBin>)
ChartData::OverCap    { unit, cap }                      // refusal: nothing to draw
ChartData::Duplicates { x, series }                      // refusal: pivot found 2 rows in one cell

Axis { labels: Vec<String>, positions: Option<Vec<Option<f64>>> }
```

`Axis.labels` render through the engine's `CellFormat` (the grid's own display config — a date
column uses `date_format`, not `timestamp_format`), clipped to `DISPLAY_CHARS`, with NULL
reading `(null)` — a **label, never a key**. `positions` is `Some` when X is numeric or temporal
(epoch milliseconds; clock times in their own ticks), so line/scatter renderers may place marks
truly rather than equally spaced; a NULL X has no position.

**Engine mechanics.** `Rows` is a *projection*, not a query: select the referenced columns plus
the ordinal, `ORDER BY __strata_ord`, `LIMIT cap + 1` — `cap + 1` rows back is `OverCap`, never
a truncated chart. Then pivot in Rust: with a series column, cell identity is the (X value,
series value) **pair of values** (`ScalarValue`s, never their renderings — a NULL and a literal
`"(null)"` stay distinct), and a second row landing in one cell answers `Duplicates` rather than
silently keeping either. NULL Y decodes to `None` (a gap). No aggregation, no bucketing, no
reordering — the engine adds nothing the result didn't contain. An *encoding* mistake (a text
column as Y, a column that doesn't exist) is an `Err` in the engine's own words, refused before
DataFusion can answer in its own.

`Raw` (scatter) filters to finite coordinates (NaN and both infinities out — Arrow's null bitmap
is unset for a NaN), caps at `cap + 1`, returns points. `Histogram` runs a min/max pass over the
finite values, then uniform bins counted engine-side: `√n` clamped to `6..=24` when the bin
count is open, at most `MAX_BINS` (200) always; the last bin closes at the measured maximum.

**Caps** (`chart/config.rs`): `ROWS_CAP` 1 000 rows, `PIE_CAP` 24 slices, `RAW_CAP` 6 000
scatter points.

## 6. Config and state

`ChartConfig` (serde, strata-model): `{ mark, x, ys, series, bins, hidden, log_y, sort }` — column
references, a bin count and three view preferences, no results. It lives on `QueryTab` under
**`Chan::Chart(tab)`** (so an encoder edit re-charts this body and wakes nothing else) and
persists via `TabSnapshot::chart`.

The config holds **intent**: `mark` and `ys` are `Option` (unset ⇒ derive), and `x` is a
three-state `ChartX { Auto, RowIndex, Column(name) }` — "not chosen" and "chosen to be the row
index" are different answers, and an `Option<String>` would let the next result's date column
overrule a deliberate row-index axis. `Some(vec![])` on `ys` is a real state too: the user
deliberately unpicked every Y, and the canvas says so rather than quietly re-deriving.

**`resolve` → `encode` (`chart/config.rs`) is the one construction site.** `resolve` merges the
schema's defaults *under* the user's choices, channel by channel: take the choice if the result
can still answer it, otherwise derive. Re-deriving is a **read-time fallback**, never a write
back into the config, so a column that disappears from one result and returns in the next brings
the user's choice back with it; a mark that takes one Y narrows the resolved encoding and leaves
the config holding the rest. `encode` then turns the resolved encoding into the `ChartQuery` —
the single place cache identity is built.

Defaults, merged under user-set keys: X = the first time column (instant or clock), else the
first dimension, else the row index; Ys = the leading measures (up to four, minus the column X
took — a column is never plotted against itself by default); mark = line when **the charted X**
is temporal, bar otherwise (the default reads the axis actually being drawn, not the result's
column list); series = none.

**`sort` is a view transform, not part of the read** (`chart/sort.rs`). `ResultOrder` (default)
| `ByX` | `ByYDesc`, applied client-side to the settled `ChartData::Table` — flipping it is a
re-render, not a re-query, and cache identity stays untouched. The comparator is **total in both
directions** (`total_cmp`; the direction is a flag inside the comparison, not a `reverse()` at
the call site): a gap or NaN is not a small value, so it sorts last either way rather than
heading a descending chart. The sort is stable, so equal keys keep result order — a refinement
of the query's own order, never a reshuffle. `ByX` sorts by true position where the axis has one
(so `"10"` sorts after `"9"`), else by label. Only a `Table` has an order to permute: scatter
points are unordered and histogram bins are ascending by construction, so the strip offers the
toggle for neither.

**`hidden` and `log_y` are the other two view transforms; `bins` is the one channel that is part
of the read.** The engine does a histogram's counting, so a bin count reaches
`ChartQuery::Histogram` and a new value is a new cache entry — clamped to `engine::MAX_BINS` where
it is encoded as well as in the engine, so the control and the read cannot disagree; an empty box
is `None`, which is the engine's own `√n`. The other two never touch identity:

- **`hidden`** (`chart/hide.rs` — `hide::applied`) blanks a hidden series' values to all-`None`
  **in place**. Positions, and therefore `Dress::series` colours, never move under a legend press,
  and `marks` needs no idea it happened. Keyed by series **name** — a label, not a key, so a
  NULL-valued series and a literal `"(null)"` one toggle together (accepted). It is applied
  **after** the sort, or hiding the first series would reshuffle a `ByYDesc` chart's category
  axis. Not pruned against the result: a stale name matches nothing and comes back with its
  column — which ⌥-press honours too, editing the set rather than rebuilding it from the current
  legend. `resolve` drops it whole for a mark whose legend cannot un-hide (a pie's rows are
  inert, and its Y is an ordinary measure a bar may have hidden earlier). ⌥-press isolates, and
  on the sole visible series shows them all again. The legend survives the **all-hidden notice**,
  which names it as the way back — and no other, since every other notice draws no plot for a
  swatch to key.
- **`log_y`** is offered only where a mark plots position rather than extent (`config::log_axis`:
  line, scatter, histogram — a bar and an area are read as area from a baseline, which a log axis
  has none of; a pie has no axis) and **never refuses**: it falls back to a linear axis under the
  non-blocking banner, and `mod.rs::log_fallback` says which of two reasons it was. One is a value
  at or below zero — a histogram's empty bins are not such a value, since a zero count paints
  nothing on either axis and blocking on one would take the scale away from the long tails it
  exists for. The other is a span whose **ratio** overflows: a log axis is bounded by `end/start`
  rather than `end - start`, and plotters turns an overflowed ratio into a `usize::MAX` tick count
  it counts down on the render thread. A result with nothing positive in it at all gets neither
  message: `log_span` answers `None` for that too, and there is no chart under it to explain.

## 7. Guardrails

Computed from the settled `ChartData` or the encoding — never re-derived in the UI. Every
refusal renders as one notice in place of the canvas (a glyph tile, a title, the condition, and
where there is a fix, the fix in prose — `chart/mod.rs::notice`), because the alternative to a
notice is a *blank pane*, indistinguishable from a bug:

| Condition | Message gist |
|---|---|
| `Rows` returned > cap rows (1 000; pie 24) | too much data to chart honestly — aggregate it in SQL |
| `Duplicates { x, series }` | more than one row per category — aggregate them in SQL |
| Scatter > 6 000 points | over the cap — aggregate it in SQL |
| No Y chosen / no numeric column | pick a column to plot / pick a numeric column (two messages — an empty pick and a result with nothing numeric are different problems) |
| Scatter with < 2 measures | pick two numeric columns |
| Pie with no category column | pick a category column |
| Histogram with no finite values | nothing to chart — no finite values to bin |
| Histogram over a single distinct value | every value is the same — no range to spread over bins |
| Scatter with no finite point | nothing to chart |
| Empty result / no series plotted | nothing to chart |
| Pie over a negative value | a pie cannot show negative values — chart it as a bar (dropping the slice would silently change the total every percentage reads against; a zero or NULL is arithmetic, drawn around) |
| Every series hidden from the legend | every series is hidden — press a legend entry to show it again (the one empty state the user caused, and the only one they can undo from the control that caused it; it sits *after* the empty shapes, so a result with no rows still says so) |

Plus the **non-blocking banner** over the canvas when `axis.labels.len() > 60` (`CROWDED`) — the
labels are already in hand, so the nudge costs no second query, and the chart still renders
beneath it, unaltered. It wears the Export window's warning banner (the `chart` theme's
`warning_*` box, the sheet's semantic `warning` for glyph and text). The same banner carries the
log axis's own fallback ("values at or below zero are shown on a linear axis"), which takes
precedence over the crowding nudge because it explains what the user is looking at rather than
suggesting they change it.

There is deliberately **no materialize cap, no sampling, and no aggregation fallback**: the
answer to "too much data" is always the user's own SQL. The refusals name the fix in prose, and
there is no control behind it (§8).

## 8. Deliberately absent

**No aggregate control in the strip.** The design handoff's Aggregate toggle and function menu
are absent, and nothing stands in their place: every control in the strip changes what is
*drawn*, none changes the data (§1.2, §1.3). A press that wrote the SQL for the user —
*Aggregate in SQL*, composing a `GROUP BY` from the resolved encoding and opening it unrun in a
new tab — was built and cut. The capability is well precedented (DBeaver's Grouping panel,
Metabase/Superset/Looker's eject-to-SQL), but every tool that has it puts it in a menu or a
surface of its own — none beside the encoders, where it was the one control in the strip that
*left* the chart rather than changing it. It also stood in for the chart-side aggregation that
is the thing actually worth revisiting; a shortcut that makes a gap tolerable is a reason not to
close it. Do not re-add it to the strip (`chart/strip.rs` records the same rule at the surface).

**The placement was re-litigated with a surface of its own, and that surface is built**
(Chart 09, settled in planning 2026-08-07 and shipped 2026-08-12): the **Shape panel** — a
modal working panel off the results toolbar, on both bodies — is the chart-side aggregation
UX the cut press was standing in for, in the placement the precedents use. It composes
visible SQL (group columns with `date_bin` strides, per-measure aggregates, an explicit
`ORDER BY`) and opens it **unrun** in a new tab; from the Chart view it arrives seeded from
the resolved encoding. The aggregate vocabulary is UI-local text rendering
(`results/shape/compose.rs`) and enters no engine type; the refusal overlays still keep no
control behind them, and the strip is still not the place.

What survived that cut on its own merits is the **instant/clock role split** (§3), because
chart-side bucketing needs exactly that distinction and it belongs where the Arrow `DataType`
still is — and the Shape panel is now what reads it (a clock column is offered only sub-day
strides).

**No regression presets in the strip's mark set.** A scatter draws raw points, plus the one
sanctioned computed overlay: the least-squares trendline of §10, built engine-side as
Chart 11 records there. Nothing else fits a curve for the user; the SQL `regr_*` family
(`docs/CHART_FUNCTIONS.md`) is the general mechanism.

## 9. Rendering

`canvas(RenderCallback)` through `PlotSkiaBackend` (`freya::plot`, fork feature), logical units
only, explicit repaint requests. The frame (data + mark + dress) is published into a slot the
render callback **peeks**, and the effect that fills the slot requests the redraw —
`RenderCallback`'s `PartialEq` is always true, so a callback that captured the frame by value
would paint the first frame forever. The `chart` component theme dresses the plot, with the
categorical ramp as `series_1`…`series_10` (a pie slice past the tenth blends a step toward the
pane, so wrapped neighbours never read as one wedge); theme changes go through
`UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.

Marks draw per §4 through plotters' own machinery: ~5 horizontal gridlines, a nice value
maximum, abbreviated ticks, thinned X labels (one per ~64 logical px), a zero baseline on
negative spans, point markers on lines up to 60 categories. Two mechanisms keep this from
becoming a hand-rolled axis stack:

- The category axis is a plotters **`Ranged`** (`chart/axis.rs::Categories`) that hands plotters
  its own key points, so every gridline and tick lands *on* a category and is labelled with that
  category's own text; thinning is the key-point stride.
- True `Axis.positions` placement is taken only when the positions are present, finite and
  **strictly increasing** — the case where result order and value order coincide. Otherwise
  placing marks by value would re-order the axis §1.6 says is the user's, so it falls back to
  equal spacing.
- The **value** axis is a second one: `axis.rs::ValueCoord`, a `Ranged` with a linear and a
  logarithmic arm. One coordinate rather than making every mark generic over its Y — that would
  split each mark into a build half and a draw half and grow a type parameter on `mesh`,
  `hit_box` and `zero_baseline`. A log axis skips `nice_max` and the proportional `EDGE_AIR`
  (both linear arithmetic that pushes the floor through zero) and rounds out to whole decades
  instead, taking the *next* decade out when a bound already sits on one — the log version of the
  same idea, and what stops a long-tail histogram drawing every count of 1 as a bar of no height.
  A log tick abbreviates **per value**, the deliberate opposite of the linear rule above: a decade
  axis has no single magnitude to choose one unit from.

**Crosshair** (`chart/paint.rs`). Two hairlines across the plot frame and a value label, ruled
through the **hovered mark** rather than under the pointer. That is a cost model, not a
simplification: Freya has no incremental rendering (`render_pipeline.rs` repaints every node every
frame) and `CanvasElement::render` calls its `on_render` on each pass, so any reactive write here
re-runs `marks::draw` — a full plotters replot plus a rebuild of every hit region, on the render
thread. A pointer-tracked crosshair paid that on every mouse sample; riding on the hover costs
nothing beyond what the readout already costs, for the same reason `Hit::anchor` exists. The price
is that the axis can only be read at a mark, which is where the numbers are.

Each `Hit` carries the mark's own point **and its value**, so the readout is never inverted back
out of the pixel row — that round trip put `11.01` under a tooltip reading `11`. `PlotArea`
(plotters' own `plotting_area().get_pixel_range()`) comes back **with** the hit regions, in
`draw`'s own answer rather than a second slot, only so the rules span the plot rather than the
pane; a pie answers none and has no crosshair. The value label **flips below its rule** where
sitting above it would leave the plot — the same flip the hover card makes, and a commoner case
than it sounds: a value axis ends on a nice number, so a maximum that is already one (any
percentage topping out at 100) puts the tallest mark exactly on the frame's top edge. The three
pieces are absolute siblings of the plot, not children of a wrapper: an absolutely positioned node
resolves against its parent's area, and a wrapper here is a *stacked* sibling of a fill-height plot
— measured, its rule landed a whole pane below the pointer.

**Hover readout** (`chart/paint.rs`). The paint that draws a mark records its **hit region** —
a box for bars, bins and points, a wedge (start angle + sweep, so the arc crossing zero tests
correctly) for pie slices — because the paint is the only place the true geometry exists;
recomputing it for the pointer would be a second answer to "where is this bar". The pointer
takes the *nearest* containing region (overlapping reaches happen), and the readout is a
standard `Tooltip` anchored to the **mark**, not the pointer — which is what lets a `!=` guard
suppress re-renders across a slow drag. The card flips to the other side of its anchor rather
than running off the pane, sits on `Layer::Relative(1)` so it paints in front of its sibling
plot, and is never a pointer target (a hit-testable card under the pointer would fire
`pointer_leave` and unmount itself). A new frame or a resize rebuilds the hit regions and clears
the hover.

**Copy Image** (`chart/capture.rs`). The results toolbar's Copy Image renders the settled frame
into an offscreen raster surface and puts its pixels on the system clipboard; nothing is written
to disk. It is the **same** `Frame` the visible canvas is painting, through the **same**
`marks::draw` — which is why `draw` takes a canvas and a `FontCollection` rather than a
`CanvasContext`, and why it returns its hit regions instead of writing them through the handle
the live plot's pointer reads. No paint pass is involved: the font collection is a root context.
The capture is a fixed 1600x900 at 2x rather than the pane's own size (a pane dragged narrow
would copy a chart with half its labels thinned away), fills `dress.background` first because
the live canvas is transparent over the pane, and reads back as unpremultiplied RGBA because
`raster_n32_premul` is BGRA on Apple. The item is **absent** over a notice state, not disabled.
The clipboard's image side is a fork addition (`freya-clipboard`: `Clipboard::set_image` /
`get_image`) made inside the crate's existing shape — the integration still provides a
`Box<dyn ClipboardProvider>` into the root context; the trait is the fork's own now and covers
images, and copypasta was replaced by arboard rather than run beside it, because text and images
are one clipboard.

**Legend** (`chart/strip.rs`, entries resolved in `chart/mod.rs::legend`). The legend lives in
the strip, not on the canvas — a plot-overlay legend has nowhere to go when it outgrows its box,
while the strip already scrolls, so the legend grows down and the plot keeps its width. Only
marks that draw in more than one colour have anything to key: a series legend names each series
in ramp order; a pie legend lists the **drawn** slices — from the same walk the wedges are drawn
from, so a colour is never keyed to a category the plot skipped — with each slice's share as a
percentage. Scatter and histogram, one colour by construction, show none.

A series row is also the control that **hides** that series (§6): a press toggles, ⌥-press
isolates, and a hidden row keeps its swatch and its slot and goes dim — the swatch is what says
which colour comes back. The modifier is mirrored from the strip's own global key handlers,
because a pointer event carries none, and it is re-read from every key event so a
key-up lost while the window is unfocused cannot leave it stuck. A pie's rows stay inert.

## 10. Analytical charts are SQL, mapped

Analytical chart shapes are ordinary SQL results whose columns play named roles, not engine
computations: a candlestick is open/high/low/close columns over a bucketed X; a box plot is
p25/p50/p75 columns the user computes with `percentile_cont(…) WITHIN GROUP`; error bands are
`y`, `y_lo`, `y_hi`. The chart has no aggregation of its own, so shaping the result is done in
the query — `docs/CHART_FUNCTIONS.md` is the practical reference for which SQL buys which chart
shape.

**The scatter trendline is the one sanctioned exception, and it is built** (Chart 11, settled
in planning 2026-08-07). A dashed least-squares line with an R² label is computed engine-side
(`Engine::trend`, one `regr_slope`/`regr_intercept`/`regr_r2` aggregation over the snapshot's
finite pairs) rather than templated into SQL, because the overlay is a function of the
**encoding** — which two columns the scatter currently plots — not of the query: templating it
would rewrite the user's SQL on every encoder gesture, which §6's "config is intent" forbids,
and would smuggle two scalars through a rows read by duplicating them onto every row. The fit
is its own read (`TrendSpec`, keyed `(snapshot, x, y)` — numbers only, no display config), so
toggling the overlay never re-reads the points; degenerate data (fewer than two pairs, no
x-variance) is an absent overlay, never an error. The engine computes nothing else new.
