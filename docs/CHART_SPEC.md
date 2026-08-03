# Strata — Chart view (renderer-first spec)

The results **Chart** surface: what it does, where its data comes from, and how it renders. This
file supersedes the designer's handoff CHART_SPEC (whose `screenshots/chart-*.png` remain the
**visual** reference for layout and control dress) *and* the previous committed revision of this
file, which specced an engine-side aggregation pipeline — `AggFn`, `Bucket`/`Stride`/`Width`,
auto-stride resolution, engine-imposed category order. That pipeline was implemented,
adversarially reviewed twice, and **withdrawn**: see §1.2 for the principle and
`docs/reference/INVARIANTS.md` (the chart entry) for the evidence trail. Where this file and
anything else disagree on mechanism, this file wins.

---

## 1. Principles

1. **Chart the result, not the files.** The chart is a view of the current result set — the same
   immutable snapshot the grid pages (`docs/SNAPSHOT_SPEC.md`). It never re-queries source files.
2. **The chart computes nothing SQL can say** (renderer-first — the Athena/Snowsight shape). SQL
   is the aggregation language: it already has every `GROUP BY`, `date_bin`, percentile and
   window function the chart could ever grow, the user can read and edit it, and the editor is
   two keystrokes away. The chart maps result **columns onto marks**; it never aggregates,
   buckets, or re-orders behind the user's back. Switching mark type is a repaint, not a
   re-query.

   *Why the reversal:* the withdrawn pipeline's hardest defects all lived in its aggregation
   machinery, and its ordering semantics fought the user's own `ORDER BY` — a `GROUP BY` has no
   output order, so re-aggregating an already-shaped result structurally destroys the order the
   user asked for, and everything the engine then imposed (measure-descending, bucket-ascending)
   was a workaround for a loss the design itself caused. A renderer cannot lose what it never
   recomputes.

   One deliberate exception: the **histogram** (§5) bins engine-side — binning a raw column
   needs a min/max pass and DataFusion 54 has no `width_bucket`, so hand-writing it is genuinely
   tedious. It is the only mark that computes.
3. **No shadow query language** — now literal. No aggregate menu, no bucket control, no engine
   algebra. Aggregation lives in SQL, reached through the scaffold (§8).
4. **Honest boundaries.** Never silently sample, truncate, or aggregate. Above a cap, or when
   the data's shape doesn't fit the mark, the chart refuses with a message and a CTA into SQL.
5. **Rendering is `freya-plotters-backend`** (fork, `plot` feature) on a `canvas` — plotters'
   `ChartBuilder`/series machinery, never a hand-rolled axis/tick/mark stack.
6. **Result order is the axis order, and it is real.** Rows draw in the order the user's query
   produced them, backed by the snapshot ordinal (`SNAPSHOT_SPEC.md` §9) — never scan order,
   which is measured-nondeterministic above 10 MB. Re-ordering is an explicit view toggle in the
   strip (§6), chosen by the user, never imposed by the engine.

## 2. Where it lives (already built — P2-07)

The Table/Chart `SegmentedToggle`, the per-tab `ResultsView` on `Chan::View(tab)` (persisted),
and the `ChartView` placeholder body all exist (`results/mod.rs`, `results/toolbar.rs`,
`results/chart.rs`). Find is grid-only; Refresh/Export ride the shared toolbar. This workstream
replaces the placeholder tile with:

- **Left control strip** (~232 logical px, own scroll): mark picker, X, Y (multi), Series, and
  the sort toggle.
- **Right canvas pane**: the chart, a non-blocking high-cardinality banner across the top when
  warranted, and the guardrail empty-state overlay in place of the canvas when refused.

## 3. Column roles (from the Arrow type)

Derived from each result column's Arrow `DataType`, not name strings — and derived where that
type is still in hand: `engine::catalog::chart_role`, called by `column_info`, so every column
carries its `ChartRole` beside its `Kind`. The measure arm **is** `DataType::is_numeric`, the
same predicate the read gates a Y on, so an encoder cannot offer a measure the read then
refuses. Roles drive the encoder menus and the defaults — they never change what the engine
computes, because it computes nothing:

| role | DataTypes |
|---|---|
| **measure** (Y, scatter axes, histogram value — valid on X too) | Int*/UInt*/Float*/Decimal* |
| **temporal** (X; defaults to line) | Date32/Date64/Timestamp*/Time* |
| **dimension** (X, series) | Utf8/LargeUtf8/Boolean/Dictionary |
| **nested** — excluded from encoders | Struct/List/Map/Union |

Secondary signal only: a Utf8 column whose name matches the handoff's temporal-name regex may be
*offered* as temporal, but the Arrow type wins.

## 4. Marks and encodings

| Mark | X | Y | Series | Notes |
|---|---|---|---|---|
| Bar | any column, or none (row index) | one or more measures | optional dimension | grouped or stacked |
| Line / Area | any column, or none | one or more measures | optional dimension | NULL Y cells are gaps, never interpolated |
| Pie | dimension | exactly one measure | — | cap 24 slices |
| Scatter | measure | measure | — | raw points, non-finite coordinates dropped |
| Histogram | — | measure (the value) | — | engine-binned (§5) |

Three rules make the encoding model:

- **Multiple Y columns are multiple series**, named by column — `SELECT month, revenue, cost …`
  is two lines with no configuration. This is what replaces the withdrawn multi-measure
  machinery, and it is how analytical presets arrive later (§10): a candlestick is four Y
  columns in named roles, not an engine computation.
- **A series column pivots long → wide**: rows `(x, series, y)` become one series per distinct
  series value, named by value (`value: column` when there are also multiple Y columns). The
  pivot is a reshape, not an aggregation — and it is the **only** operation that can conflate
  rows, so it is the only thing that refuses on duplicates (§7).
- **Without a series column there is no pivot**: each row is its own mark in result order.
  Duplicate X labels draw as duplicate marks — the chart shows what the result holds.

## 5. Data: `Engine::chart` over the snapshot

One engine method, one freya-query capability in front of it, keyed `(SnapshotId, ChartQuery,
display config)`, no confirm — a projected, capped read of a local snapshot is `fetch_page`-tier
work. The read holds the snapshot pin for the call, like `export`.

The third key is not optional: `Axis.labels` render through the engine's live
`datafusion.format.*` (below), which Settings changes with no restart and no new snapshot, so an
entry keyed on the first two alone serves labels rendered under a format the user has since
changed. `ChartSpec` carries `config::display_subset` of the app's engine overrides, which makes
a format change a new entry rather than a stale one.

**Vocabulary (strata-model, `chart.rs`).** All hash/eq-able — `ChartQuery` is cache identity.

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
reading `(null)` — a label, never a key. `positions` is `Some` when X is numeric or temporal
(epoch ms), so line/scatter renderers may place marks truly rather than equally spaced; a NULL X
has no position.

**Engine mechanics (strata-core, `engine/chart.rs`).** `Rows` is a *projection*, not a query:
select the referenced columns plus the ordinal, `ORDER BY __strata_ord`, `LIMIT cap + 1` —
`cap + 1` rows back is `OverCap`, never a truncated chart. Then pivot in Rust: with a series
column, cell identity is the (X value, series value) **pair of values** (never their renderings —
a NULL and a literal `"(null)"` stay distinct), and a second row landing in one cell answers
`Duplicates` rather than silently keeping either. NULL Y decodes to `None` (a gap). No
aggregation, no bucketing, no reordering — the engine adds nothing the result didn't contain.

`Raw` (scatter) filters to finite coordinates, caps at `cap + 1`, returns points. `Histogram`
keeps the previous engine implementation: a min/max pass over finite values, then uniform bins
(`√n` clamped to 6..=24 when open, ≤ 200 always), counted engine-side.

## 6. Config and state

`ChartConfig` (serde, strata-model): `{ mark, x, ys, series, sort }` — column references and a
view preference, no results. Lives on `QueryTab` under **`Chan::Chart(tab)`**, persists via
`TabSnapshot`, re-derives when a new result's columns no longer match.

**`sort` is a view transform, not part of the read.** `ResultOrder` (default) | `ByX` |
`ByYDesc` — applied client-side to the settled `ChartData::Table`, so flipping it is a
re-render, not a re-query, and cache identity stays untouched. Any float comparison in that
reorder must be total (`total_cmp`, NaN last) — the withdrawn pipeline's `sort_by` panic is the
standing lesson.

Defaults, merged under user-set keys: x = first temporal else first dimension else none;
ys = the measure columns (first few); mark = line if x is temporal else bar; series = none.

## 7. Guardrails

Computed from the settled `ChartData` or the config — never re-derived in the UI:

| Condition | Message gist | CTA |
|---|---|---|
| `Rows` returned > `cap` rows (default 1 000; pie 24) | too many rows to chart honestly | **Aggregate in SQL** (§8) |
| `Duplicates { x, series }` | more than one row per category — aggregate in SQL | **Aggregate in SQL** |
| Scatter > `raw_cap` (6 000) points | too many raw points | **Aggregate in SQL** |
| No Y column chosen / none valid | pick a numeric column | — |
| Histogram with no numeric column | pick a numeric column | — |

Plus the non-blocking high-cardinality banner over the canvas when `axis.labels.len() > 60`.
There is deliberately **no materialize cap and no sampling** — and no aggregation fallback: the
answer to "too much data" is always the user's own SQL, one click away.

## 8. The scaffold — the bridge into SQL (promoted)

The scaffold is no longer an escape hatch beside the chart's own aggregation — it **is** the
aggregation path. It builds a real query from the current encoding and opens it in a **new tab**
through the existing funnel (`session.open_named`), never auto-run:

```sql
SELECT country, SUM(amount) AS sum_amount
FROM ( <the tab's SQL, verbatim> )
GROUP BY 1
ORDER BY 2 DESC;
```

Temporal X scaffolds with `date_bin(interval '1 day', <x>)` (a starting stride the user edits —
the engine no longer guesses spans) and orders by the bucket ascending; `COUNT(*) AS n` when no
measure is chosen; a series column adds a second grouping column. Identifier quoting follows
export's `select_sql`. Offered from every refusal in §7 **and** from the healthy chart — it is
how a raw-data tab becomes a chartable one, which is the normal workflow, not the failure path.

## 9. Rendering

Unchanged from the previous revision in mechanism — `canvas(RenderCallback)` through
`PlotSkiaBackend` (`freya::plot`, fork feature; `draw_pixel` is a reachable `todo!()` to fix and
push first), logical units only, explicit repaint requests, marks per §4, plotters mesh with ~5
gridlines / nice max / abbreviated ticks / thinned X labels / zero baseline on negative spans, a
`chart` component theme plus the categorical palette as 10 palette slots, then
`UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`. Line/area may use `Axis.positions`
for true placement when present; equally-spaced labels are the fallback, not the rule.

As built (02), two mechanisms are worth naming because they are what keep §9 from becoming a
hand-rolled axis stack:

- The category axis is a plotters **`Ranged`** (`chart::axis::Categories`) that hands plotters
  its own key points, so every gridline and tick lands *on* a category and is labelled with that
  category's own text; thinning is the key-point stride, at one label per ~64 logical px.
- True `Axis.positions` placement is taken only when the positions are present, finite and
  **strictly increasing** — the case where result order and value order coincide. Otherwise
  placing marks by value would re-order the axis §1.6 says is the user's, so it falls back to
  equal spacing.

## 10. Later (owned follow-on, not scope creep)

**Analytical charts are column-role presets over SQL the user owns**, not engine computations:
a candlestick maps open/high/low/close columns; a box plot maps p25/p50/p75 (the user writes
`percentile_cont(…) WITHIN GROUP`); error bands map `y`, `y_lo`, `y_hi`. Each preset is a role
mapping plus a scaffold/snippet template that writes the SQL producing those columns —
`docs/CHART_FUNCTIONS.md` is the survey of what SQL can express and which roles each preset
consumes. A scatter trendline (`regr_slope`/`regr_intercept`/`regr_r2` in one engine call) is
the one candidate for a computed overlay, weighed when 05 is picked up.

## 11. Acceptance (workstream-level)

- [ ] Selecting Chart renders the current result with visible, overridable defaults; switching
      mark type is a repaint, never a re-query.
- [ ] Rows draw in result order; the sort toggle re-orders as a view transform; multiple Y
      columns and/or a series column split series correctly; NULL Y cells render as gaps.
- [ ] The pivot refuses duplicates with a working **Aggregate in SQL** CTA; over-cap results
      refuse the same way; the scaffold opens an editable new tab and never runs it.
- [ ] Paging, charting and export of the same snapshot agree on row order (the ordinal,
      `SNAPSHOT_SPEC.md` §9); the ordinal never appears in any user-visible schema or file.
- [ ] Chart re-themes with the app theme and redraws on resize; config persists per tab; the
      chart never issues a query against source files.
