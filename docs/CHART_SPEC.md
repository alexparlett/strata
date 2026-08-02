# Strata — Chart view (grounded spec)

The results **Chart** surface: what it does, where its data comes from, and how it renders. This is
the committed, engineering-grounded successor to the designer's handoff spec
(`.claude/design-handoff/…/project/CHART_SPEC.md`); the handoff bundle and its
`screenshots/chart-*.png` remain the **visual** reference (layout, spacing, control dress), but
where the two disagree on mechanism, this file wins. Everything stated here was verified against
the actual stack: DataFusion **54** (`strata-core/Cargo.toml`), the Freya fork's
`freya-plotters-backend` (Skia `DrawingBackend` for plotters 0.3.7), the fork's `canvas`
component, and the engine's snapshot system (`docs/SNAPSHOT_SPEC.md`).

---

## 1. Principles

The designer's principles hold, with one replaced:

1. **Chart the snapshot, not the files.** The chart is a view of the current result set — the same
   immutable snapshot the grid pages over. It never re-queries source files on its own.
2. **No auto-charting.** The schema constrains and pre-fills the encoding controls; the user
   decides intent. Defaults are visible and overridable, never hidden magic.
3. **No shadow query language.** Aggregation the user wants to *own* is SQL: the chart can scaffold
   a real, editable `GROUP BY` query into a new tab. No `stats … by` dialect.
4. **Honest boundaries.** Never silently sample or truncate. Above a cap the chart refuses with a
   message and a CTA. (DataFusion has no `TABLESAMPLE` — refusal is also the only honest option.)
5. ~~Dependency-free canvas rendering~~ → **Rendering is `freya-plotters-backend`.** The fork ships
   a plotters `DrawingBackend` over Skia (`freya::plot`, behind the `plot` feature). We use
   plotters' `ChartBuilder`/series machinery rather than hand-rolling axes, ticks and marks.

And one structural correction, the load-bearing change from the handoff:

6. **Aggregation runs in DataFusion over the snapshot, not in the client.** Every snapshot is
   already registered in the engine's `SessionContext` as an Arrow IPC table
   (`__snap_{id}`, `engine/query.rs`); `read_page` (DataFrame ops) and export's `select_sql`
   (SQL over the snapshot) are the two existing precedents. A new `Engine::chart` method runs the
   grouped/binned/raw read there and returns a small chart-ready model. Consequences:
   - The handoff's `CHART_MATERIALIZE_CAP` (200k rows) **does not exist** — an aggregated chart
     over a multi-million-row snapshot is a normal hash aggregation.
   - The handoff's client-side reducer table (`_chartAgg`) is replaced by real DataFusion
     aggregates.
   - Temporal X is bucketed with **`date_bin` inside the engine query** — the handoff's known flaw
     (grouping on raw instants) never exists here, not only in the SQL scaffold.

## 2. Where it lives (already built — P2-07)

The Table/Chart `SegmentedToggle`, the per-tab `ResultsView` on `Chan::View(tab)` (persisted), and
the `ChartView` placeholder body all exist (`results/mod.rs`, `results/toolbar.rs`,
`results/chart.rs`). Find is grid-only; Refresh/Export ride the shared toolbar. This workstream
replaces the placeholder tile with:

- **Left control strip** (~232 logical px, own scroll): chart type, X, Y, Series, Aggregate
  controls, and the bucket-stride control when X is temporal.
- **Right canvas pane**: the chart, a non-blocking high-cardinality banner across the top when
  warranted, and the guardrail empty-state overlay in place of the canvas when blocked.

## 3. Column roles (from the Arrow type)

Derived from each result column's Arrow `DataType`, not name strings:

| role | DataTypes |
|---|---|
| **measure** (Y, scatter axes, histogram value — and valid on X, below) | Int*/UInt*/Float*/Decimal* |
| **temporal** (X; defaults to line) | Date32/Date64/Timestamp*/Time* |
| **dimension** (X, series) | Utf8/LargeUtf8/Boolean/Dictionary |
| **nested** — excluded from encoders | Struct/List/Map/Union |

The role partition drives **defaults and ordering**, not what X may hold: a numeric column is a
measure first but is also offered on X for bar/line/area (a step number, an integer code, an epoch
column). A numeric X groups by value, ordered ascending, and is binnable with a uniform width —
the numeric analog of the temporal stride (§5).

Secondary signal only: a Utf8 column whose name matches the handoff's temporal-name regex
(`(^|_)(ts|date|time|day|month|created|updated|_at)$` etc.) may be *offered* as temporal, but the
Arrow type wins.

## 4. Chart types and encodings

Six types. Each constrains which encoders are shown/valid:

| Type | X | Y | Series | Aggregate |
|---|---|---|---|---|
| Bar | dimension/temporal/numeric or none | measure or `count(*)` | yes | yes |
| Line | temporal/dimension/numeric | measure or `count(*)` | yes | yes |
| Area | temporal/dimension/numeric | measure or `count(*)` | yes | yes |
| Pie | dimension | measure or `count(*)` | no | yes (always groups by X) |
| Scatter | numeric | numeric | no | no (raw points) |
| Histogram | — | numeric (the value) | no | no (engine bins) |

Bar/Line/Area/Pie share the aggregate pipeline; Scatter and Histogram are their own `ChartQuery`
shapes (§5).

## 5. Data: `Engine::chart` over the snapshot

One engine method, one freya-query capability in front of it.

**Vocabulary (strata-model).** `ChartQuery` — the request, resolved from config + schema, no UI
types: `Aggregate { x, series, measures: Vec<Measure>, bucket: Option<Bucket>, group_cap }` (`Measure`
is `{ y, agg_fn }` — plural because a box plot or candlestick is extra measures on the same
group, `CHART_FUNCTIONS.md` §2, though the core charts always send one; `Bucket` is a time
stride for temporal X or a uniform width for numeric X),
`Raw { x, y, cap }` (scatter), `Histogram { col, bins }`. All fields hash/eq-able so `ChartQuery`
can be cache identity — which is why a bin width is a **`Width`**, a newtype over the `f64`'s own
bits with a validating constructor: a width is unavoidably a float, and cache identity may not be
approximate, so identity is exact and a zero/negative/NaN width has no representation at all.

`ChartData` — the chart-ready answer: `Grouped { categories, series, bucket }` (per-series
`Vec<Option<f64>>`, wide, pivoted in the engine), `Points`, `Bins`, or **`OverCap { unit, cap }`**.
A refusal is its own variant rather than a `capped` flag beside a half-filled payload, because
"honest boundaries" (§1.4) means there is no such thing as a truncated chart to hand a renderer.
There is no `group_count` field either: `categories.len()` **is** the group count (the axis is the
groups), and a second copy of a live fact is a thing to keep in step for nothing. `bucket` reports
the width actually used, since the request's may have been `None`. Series run **measure by measure
in request order**, and within a measure by series value ranked on what it measures — that order is
how a multi-measure preset reads its parts back.

**Engine (strata-core).** `Engine::chart(snapshot: SnapshotId, q: ChartQuery)` reads
`ctx.table(__snap_{id})` — the `read_page` DataFrame precedent — applies the grouping/binning, and
pivots to `ChartData` in Rust. Specifics:

- **Aggregate fns**: `sum | avg | min | max | count | median | count-distinct`, all DataFusion
  built-ins, named per measure. A measure with no Y is `count(*)` whichever function it names, and
  nulls in Y are skipped by the aggregates natively.
- **Temporal X buckets with `date_bin`** (`Timestamp`, `Date32`, `Date64` — all cast to
  `Timestamp(Millisecond)` first, because `date_bin`'s signature takes neither date type and only
  `Date32` coerces. A `Time32`/`Time64` column groups on its raw value like any other dimension.)
  The stride is auto-derived from the column's span — span > ~2y → `1 month`, > ~60d → `1 day`,
  > ~2d → `1 hour`, else `5 minutes` — **and then widened until the axis fits under `group_cap`**,
  because the ladder alone produces axes that cannot be drawn: 60 days at the hourly rung is 1 440
  buckets against a default cap of 1 000, so "chart my last two months" would refuse by
  construction, and a default that guarantees a refusal is not a default. A stride the *request*
  names is never widened — the user asked for it, and a refusal is the honest answer to a bucket
  that doesn't fit. Resolution happens **engine-side**, and `ChartData::Grouped.bucket` reports
  which width was used, so the strip shows the real answer rather than running a span query of its
  own. Bucketed axes order by bucket ascending. `date_bin` emits no row for an empty bucket: the
  engine fills the sequence back in with `None` values (that is why series values are
  `Option<f64>`), so a renderer shows a **gap** and never interpolates across missing buckets.
- **A numeric X groups by value, ordered ascending.** Optionally binned with a uniform width —
  the same control slot as the temporal stride, and the same honesty rule: the bin sequence is
  filled, so an empty bin is a gap, never interpolated and never a zero. The group key is the bin
  **index** (`floor(x / w)`), with the start computed as `index × w`: identical buckets to
  `floor(x / w) * w`, which is what the SQL scaffold writes, but keyed on an integer — matching
  returned bucket starts against a generated sequence would compare floats that agree
  mathematically and differ in their last bit.
- **Category order is the measure, descending** for categorical X, ties broken by label ascending.
  This spec previously proposed `min(row_number() OVER ())` to keep a result the user `ORDER BY`ed
  in that order on the axis, and asked for the assumption to be tested. **It was, and it is false.**
  An Arrow *File* scan is range-split across `target_partitions` once the file passes
  `datafusion.optimizer.repartition_file_min_size` (10 MB), and a window with no `PARTITION BY` then
  sits above a `CoalescePartitionsExec`, whose own contract is "no guarantees are made about the
  order of the resulting partition". Measured on stock config: a 200k-row snapshot came back in
  perfect file order, a 3M-row one put 2 975 424 of 3 000 000 rows out of it. The property would
  therefore have held at every test size and silently reversed itself on exactly the large results
  the chart exists for. Measure-descending is deterministic, independent of scan parallelism, and
  needs no window function at all. Temporal and numeric axes order by value and never face the
  question.
- **Caps are detected, not silently applied**: the aggregate runs with `LIMIT group_cap + 1`, and
  `cap + 1` rows back means *refused* — `ChartData::OverCap`, never a truncated chart. Same pattern
  for scatter's raw-point cap (counted **after** null coordinates are filtered, so the cap counts
  points that can be drawn). `group_cap` bounds **aggregate rows — categories × series** — which is
  exactly the category count for the common unsplit chart and an honest budget for a split one. A
  bucketed axis is capped a second time on the buckets it would *span*: two rows a decade apart are
  two aggregate rows and eighty-seven thousand hourly buckets.
- **A missing (category, series) cell is `None`** — bars render it as zero-height, lines as a gap.
- **A NULL X or series is its own group, labelled `(null)` — and keyed by the value, not by that
  label.** A column that genuinely holds the string `(null)` keeps its own category; merging the
  two would drop one group's rows and say nothing about it. On a bucketed axis the NULL group sits
  **after** the sequence, where it implies no position in time or on a number line.
- **A bucket of the wrong kind for the column is refused, not ignored** — a stale width left on a
  temporal X by an encoding change would otherwise chart something the strip isn't showing.

**UI subscription (strata-freya).** A `QueryCapability` shaped exactly like `PageSpec` /
`FetchSnapshotPage`: keys `(SnapshotId, ChartQuery)`, built in one place, no confirm dialog (a
grouped read over a local snapshot is cheap — this is *not* the `ProfileActions::ask` tier).
Cache identity is the request; a config change is a new key; the entry dies with its subscribers.
The chart never holds results in the store.

## 6. Config and state

`ChartConfig` (serde, strata-model): `{ chart_type, x, y, series, agg: bool, agg_fn, stride: Option }`
— column references, no results. It lives on `QueryTab` under its own **`Chan::Chart(tab)`**
channel (so encoder edits never wake the editor or grid), and persists with the tab via
`TabSnapshot`, like `view` does.

Defaults are **derived from the result schema, merged under user-set keys** (the handoff's §5
rules): x = first temporal else first dimension; y = first measure else `count(*)`; type = line if
x temporal, scatter if no dimension and ≥2 measures, else bar; agg = on; agg_fn = sum (count when
y is none). When a new result's column set no longer matches the stored config's columns, the
config re-derives — stale column references never reach `ChartQuery`.

## 7. Guardrails

Computed before rendering, shown as an overlay empty-state (icon + title + body + optional CTA) in
place of the canvas:

| Condition | Message gist | CTA |
|---|---|---|
| Aggregate produced > `group_cap` (default 1 000; pie 24) groups | too many groups to chart honestly (a temporal/numeric X also nudges to a wider bucket) | **Add GROUP BY in SQL** |
| Aggregate OFF (scatter/raw) and rows > `raw_cap` (default 6 000 points) | too many raw points | **Add GROUP BY in SQL** |
| Histogram with no numeric column | pick a numeric column | — |
| Scatter without numeric X and Y | pick two numeric columns | — |

Plus the **non-blocking high-cardinality banner** over the canvas when an aggregated chart has
more than 60 distinct X groups (`categories.len()` — no extra query, no second copy of the fact,
and the first two rows above are one answer from the engine: `ChartData::OverCap { unit, cap }`,
where `unit` is the noun the message names). The chart
still renders. There is deliberately **no materialize cap and no sampling** (§1.4, §1.6).

## 8. The "Add GROUP BY in SQL" scaffold

Builds a real query from the current encoding and opens it in a **new tab** through the existing
funnel (`session.open_named`), never auto-run. The source is the tab's own SQL as a derived table:

```sql
SELECT country, SUM(amount) AS sum_amount
FROM ( <the tab's SQL, verbatim> )
GROUP BY 1
ORDER BY 2 DESC;
```

Temporal X scaffolds with `date_bin` using the currently-selected stride and orders by the bucket
ascending; a binned numeric X scaffolds the same shape with `floor(<x> / <w>) * <w> AS bucket`.
`COUNT(*) AS n` when Y is none; series adds a second grouping column. This is the
user-owned escape hatch the guardrails point at, and it is also offered on the healthy chart
(promotion, not only refusal).

## 9. Rendering

A `canvas(RenderCallback)` body drawing through `PlotSkiaBackend` (`freya::plot`, `plot` feature —
not yet enabled in `strata-freya`; enabling it is part of the workstream).

- **Units**: `CanvasContext.size` is logical and the canvas pre-scales by the scale factor — draw
  in logical units, never touch the scale factor (AGENTS.md §3 holds with no work).
- **Redraw**: `RenderCallback`'s `PartialEq` is always-true, so a new closure does not dirty the
  canvas. When the chart model or theme changes, request a repaint explicitly
  (`Platform` → `RequestRedraw` — the `feature_plot_3d` example's own idiom). If that proves
  racy or wasteful, the fix is a fork-level revision/diff-key on `CanvasElement`, not an app-side
  workaround.
- **Marks**: Bar = grouped `Rectangle`s per series; Line = 2px `LineSeries` + point dots; Area =
  `AreaSeries` at low alpha over its line; Scatter = ~55%-alpha circles; Histogram = contiguous
  bars from engine bins; Pie = plotters' `Pie` element (verified: draws via `fill_polygon` + text
  — it does **not** hit `draw_pixel`), with a right-side legend + percentages.
- **Fork hygiene (do first)**: the backend's `draw_pixel` is a reachable `todo!()` panic —
  implement it (1×1 fill) in the fork and push, per AGENTS.md §6.
- **Axes/ticks**: plotters mesh configured to the handoff's look — ~5 horizontal gridlines, nice
  max, abbreviated tick labels (`1.2k` / `3.4M`), thinned X labels, zero baseline when data spans
  negatives.
- **Theme**: a `chart` component theme in `strata-freya`'s `theme.rs` (axis/grid/label/banner
  colours) plus the **categorical palette as palette slots** (10 colours — never repeated
  `specific`s), then `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`. Text styles take
  the theme's font family through the backend's text API — no hardcoded `"sans"`.
- Redraw on: chart data settle, encoding change, theme change, container resize (layout drives the
  canvas size; no manual resize handling).

## 10. Later (owned follow-on, not scope creep)

The full map of what the engine's function registry can turn into charts — box plots, candlesticks,
ECDF, Pareto, heatmaps, Top-N + Other, share-of-total, gap-fill via `generate_series`, and the
tiered build order — is **`docs/CHART_FUNCTIONS.md`**, the survey of the pinned DataFusion 54
registry this spec's mechanism was designed against. Headlines that stay true here:

- **Scatter trendline**: `regr_slope/regr_intercept/regr_r2` in the same `Engine::chart` call —
  never a client-side least-squares.
- **Line/area overlays** (moving average, running total): window functions
  (`avg(y) OVER (ORDER BY x ROWS BETWEEN k PRECEDING AND CURRENT ROW)`, `sum … UNBOUNDED
  PRECEDING`) engine-side, drawn as dashed overlay series folded into the y-range.
- **Richer aggregate menu**: `percentile_cont`, `stddev`, count-distinct — cheap once the fn enum
  exists.

## 11. Acceptance (workstream-level)

- [ ] Selecting Chart shows a chart of the current result with visible, overridable defaults.
- [ ] Bar/line/area/pie honor Aggregate + fn; series split by palette colour; a categorical
      axis orders by the measure and a temporal or numeric one by value (§5); temporal X is
      `date_bin`-bucketed with a working stride control.
- [ ] Empty temporal buckets render as gaps, never interpolated lines.
- [ ] Scatter/histogram enforce their encoding guardrails; over-cap results refuse with a working
      **Add GROUP BY in SQL** that opens an editable new tab and does not run it.
- [ ] An aggregated chart over a snapshot far above 200k rows renders normally (no materialize cap).
- [ ] Chart re-themes with the app theme (no per-theme chart code) and redraws on resize.
- [ ] Chart config persists per tab; the chart never issues a query against source files.
