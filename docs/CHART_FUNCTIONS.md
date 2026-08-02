# Chart view — the DataFusion function survey

What SQL over the snapshot can express, and which chart shapes each family buys. Companion to
`docs/CHART_SPEC.md` (the buildable spec — **renderer-first**: the chart computes nothing SQL can
say, so every capability below is something the *user writes* — or the scaffold/snippets write
for them — never an engine-side chart pipeline). This file is the **capability map**: every
aggregate, window and scalar family in the engine considered for what it buys a chart, including
the ones considered and rejected.

> **Framing revision.** An earlier revision of this file read the survey as the slots of an
> engine-side query algebra (`ChartQuery ≈ { group, measures, window, derived }`) that
> `Engine::chart` would compose. That pipeline was built, reviewed and withdrawn — the spec's
> §1.2 records why. The survey itself is unchanged and still the ground truth for what a chartable
> query can contain; what changed is **who writes the query**: the user, in SQL, with the
> scaffold and snippet templates as the on-ramp, and the chart mapping the resulting columns onto
> marks.

**Ground truth:** the enumeration below was read from the **pinned DataFusion 54.0.0 sources**
(`datafusion-functions{,-aggregate,-window,-nested}-54.0.0` in the cargo registry —
`all_default_*_functions` in each crate's `lib.rs`), not from the upstream docs pages, which track
a newer version. The live registry is the truth (`docs/reference/ENGINE.md`); anything built from
this file re-verifies against it.

---

## 1. The survey

### 1.1 Aggregate functions (`datafusion-functions-aggregate` 54)

| Family | Functions | What it buys the chart |
|---|---|---|
| Basic reducers | `sum` `avg` `min` `max` `count` | The core Y pipeline (spec §5). `count(*)` is the Y-less default. |
| Robust center | `median`, `approx_median` | A skew-honest alternative to `avg` in the aggregate menu. |
| Percentiles | `percentile_cont(p) WITHIN GROUP`, `approx_percentile_cont`, `approx_percentile_cont_with_weight` | **Box plots** (p25/p50/p75 in one pass), percentile bands on lines. Exact for grouped charts, `approx_` for very high row counts. |
| Spread | `stddev`, `stddev_pop`, `var_samp`, `var_pop` | **Error bars / ±σ bands** around an `avg` series — one extra measure expr, same GROUP BY. |
| Regression | `regr_slope` `regr_intercept` `regr_r2` `regr_avgx` `regr_avgy` `regr_count` `regr_sxx` `regr_syy` `regr_sxy`, `corr`, `covar_samp` `covar_pop` | **Scatter trendline** with an honest R², computed in-engine — never a client-side fit. `corr` labels the relationship without drawing it. |
| Order-sensitive | `first_value(y ORDER BY x)`, `last_value(y ORDER BY x)`, `nth_value` | **OHLC/candlestick**: `first_value`/`last_value` ordered by time + `min`/`max`, grouped by `date_bin` bucket — a candle series is *one* GROUP BY. Also "value at bucket start/end" summaries. |
| Cardinality | `approx_distinct` (HyperLogLog) | Cheap distinct counts: series-count preflight, count-distinct as a Y reducer. |
| Set/sample | `array_agg`, `string_agg` | Per-group example values for tooltips/legends (bounded — never a whole column). |
| Grouping meta | `grouping()` + `ROLLUP` / `CUBE` / `GROUPING SETS` (expr layer, verified in `datafusion-expr` 54) | Subtotal rows in the same pass: **Pareto totals, stacked-bar grand totals, drill-down summaries** without a second query; `grouping()` tells the pivot which rows are subtotals. |
| Considered, no chart use | `bool_and/or`, `bit_and/or/xor` | Nothing a chart encodes; excluded from the menu rather than "supported but pointless". |

Any aggregate is also a window function via `OVER` — that composition is most of §1.2's value.

### 1.2 Window functions (`datafusion-functions-window` 54)

| Family | Functions | What it buys the chart |
|---|---|---|
| Numbering | `row_number` | Explicit row numbering inside user SQL. (Not order preservation for the chart — that is the snapshot ordinal, `SNAPSHOT_SPEC.md` §9; `row_number() OVER ()` with no `ORDER BY` follows scan order, which is nondeterministic above 10 MB.) |
| Ranking | `rank`, `dense_rank` | **Top-N + Other**: rank series/categories by measure, fold the tail into `Other` with a CASE — the *constructive* answer to high cardinality, where the current design only refuses. |
| Distribution | `percent_rank`, `cume_dist` | **ECDF** — a cumulative-distribution chart is `cume_dist() OVER (ORDER BY x)`, essentially free, and often the honest replacement for a histogram. |
| Bucketing | `ntile(n)` | **Equal-count bins**: decile/quartile summaries — the complement of the histogram's equal-width bins for skewed data. |
| Offsets | `lag(expr, k, default)`, `lead` | **Period-over-period**: delta and growth-rate series (`(y - lag(y)) / lag(y)`) over a bucketed axis; waterfall segments. |
| Positional | `first_value`, `last_value`, `nth_value` (window forms) | **Indexed comparison**: normalize every series to its first bucket (`y / first_value(y) OVER (PARTITION BY series ORDER BY x) * 100`) so different-scale series compare on one axis. |
| Aggregates over frames | `avg/sum/min/max(y) OVER (… ROWS BETWEEN …)` | **Moving average** (frame `k PRECEDING`), **running total** (`UNBOUNDED PRECEDING`), rolling min/max envelope. |
| Whole-partition | `sum(y) OVER (PARTITION BY x)` / `OVER ()` | **Share-of-total**: percent-of-whole per group → **100%-stacked** bars/areas and pie percentages, computed where the data is. |

### 1.3 Scalar functions (`datafusion-functions` 54) — the group-key transforms

| Family | Functions | What it buys the chart |
|---|---|---|
| Temporal bucketing | `date_bin(stride, x, origin)` | The temporal X mechanism (spec §5). |
| Temporal truncation/parts | `date_trunc`, `date_part`/`extract` | **Seasonality matrices**: `extract(dow)` × `extract(hour)` as the two axes of a heatmap; month-of-year cycles. A calendar heatmap is one GROUP BY over two `date_part`s. |
| Temporal conversion | `to_timestamp*`, `from_unixtime`, `to_date`, `to_char` | Rescuing string/epoch "temporal" columns into real temporal X (the name-regex fallback in spec §3 becomes a *cast offer*, not a guess); `to_char` for engine-side bucket labels. |
| Numeric binning | `floor`, `ceil`, `round`, `trunc` | Numeric X bins: `floor(x / w) * w` (spec §5). No `width_bucket` in 54 — the arithmetic form is the mechanism, not a workaround. |
| Scales | `log`, `log10`, `ln`, `power` | **Log-scale binning** for heavy-tailed histograms (`floor(log10(x))` decade buckets). Axis *display* scaling stays in the renderer; binning math belongs to the engine. |
| Sign/branching | `abs`, `signum`, `CASE` (expr) | Waterfall up/down split; Top-N's `Other` fold; user-defined banding (`CASE WHEN x < 10 THEN 'small' …`). |
| Sampling | `random()` | The only honest sampler: an **explicit, user-visible** `WHERE random() < p` for scatter over huge results — opt-in, labeled, never automatic (no `TABLESAMPLE` exists in 54; spec §1.4 stands). |
| Series generation | `generate_series(start, stop, step)` + `unnest` (`datafusion-functions-nested` 54, `range.rs`) | **Honest gap-filling**: generate the full bucket calendar, LEFT JOIN the aggregate onto it, and an empty bucket becomes an explicit zero/absent row *by user choice*. Upgrades spec §5's "gaps only" from a permanent limitation to the default of a toggle. |
| Null handling | `coalesce`, `nullif` | The zero-vs-gap choice made explicit in the query (`coalesce(sum_y, 0)` only when gap-fill is on). |
| Labels | `concat`, `format` | Composite group keys (two-column X, series labels) built engine-side so the pivot stays dumb. |

---

## 2. The system: presets are column-role mappings over SQL the user owns

The six chart types stop being the design's unit, but not the way the algebra revision thought.
Read §1 as a whole and the real shape is: **every analytical chart is an ordinary SQL result
whose columns play named roles.**

- The user (or a scaffold/snippet template) writes the query — `date_bin` group, percentiles,
  window functions, whatever §1 offers.
- The chart maps result columns onto mark roles: a **candlestick** is open/high/low/close
  columns over a bucketed X; a **box plot** is p25/p50/p75 (+ whisker) columns; **error bands**
  are `y`, `y_lo`, `y_hi`; a **Pareto** is a measure column beside a running-share column the
  user computed with `sum(y) OVER (ORDER BY …)`; an **ECDF** is `cume_dist() OVER (ORDER BY x)`
  charted as a line. The renderer keys marks off the preset; the engine only ever ran the
  user's SQL.
- **Guardrails stay uniform** because every preset flows through the same `Rows` read — result
  order, `cap + 1`, duplicate-refusal. A new preset inherits refusal behaviour by construction.
- **The scaffold stays total**: each preset ships with the SQL template that produces its
  columns, so every chart the system can draw is one the user can open, read and edit — the
  no-shadow-language principle (spec §1.3) scales with the system instead of being outgrown by
  it.

## 3. Tiers — what to build, in what order

**Tier A — better on-ramps, no new marks**: scaffold templates beyond plain `GROUP BY` — Top-N +
Other (rank + CASE fold, the constructive answer to high cardinality), share-of-total
(100%-stacked via `sum(y) OVER …`), `FILTER`-split series. Each is a snippet the user lands on
and owns.

**Tier B — new mark presets** (one role mapping + one renderer each; SQL template alongside):
box plot, error bands, candlestick, ECDF, Pareto, heatmap (two group columns), indexed
comparison, period-over-period delta. The one candidate for engine computation is the scatter
**trendline** (`regr_*` in a single call) — weighed when 05 is picked up, as the exception it
would be.

**Tier C — toggles and rescues**: gap-fill via `generate_series` LEFT JOIN (a template, default
off = gaps); explicit labeled `random() < p` sampling for scatter (never automatic); log-decade
histogram binning; temporal *cast offers* (`to_timestamp` / `from_unixtime`) replacing the
name-regex guess.

**Considered and rejected**: `bool_*`/`bit_*` reducers (nothing to encode); `TABLESAMPLE`
(absent in 54 — its absence is load-bearing for the honesty principle); native gap-fill/`locf`
(absent; the `generate_series` join is the mechanism); `width_bucket` (absent — which is exactly
why the histogram is the one mark that computes, spec §1.2).

Renderer cost note: every Tier B mark is rects, lines, circles and polygons — nothing new from
`freya-plotters-backend`; the pie wedge (`fill_polygon`) is already the most exotic path used.

## 4. Where this lands in the workstream

Tasks 01–04 build the renderer-first chassis (`Rows` read + pivot, marks, strip, guardrails +
scaffold); nothing in them pre-builds for a tier that hasn't been picked up (AGENTS.md §5). Task
05 is the delivery vehicle: Tier A templates, Tier B presets picked by value, Tier C toggles
behind them. The survey above is the menu those choices order from.
