# Chart view — the DataFusion function survey

What the engine can actually compute, and the charting system that falls out of it. Companion to
`docs/CHART_SPEC.md` (the buildable spec): this file is the **capability map** — every aggregate,
window and scalar family in the engine considered for what it buys a chart, including the ones
considered and rejected. The design method is deliberately inverted from the handoff: not "what
did the designer draw, can we render it" but "what can one DataFusion query over the snapshot
answer, and which of those answers are charts".

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
| Numbering | `row_number` | Snapshot-order preservation for categorical axes (spec §5) — already load-bearing. |
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

## 2. The system: chart types are presets over a query algebra

The six handoff chart types stop being the design's unit. Read §1 as a whole and the real shape
appears — every capability above is the same four-slot query:

```
ChartQuery ≈ {
  group:    [bucket-transform exprs]         // date_bin / floor / date_part / CASE-band / raw
  measures: [agg(measure) [FILTER (pred)]]   // one per series-from-measures, or count(*)
  window:   [post-ops over the grouped rows] // running / moving / share / rank / lag / index
  derived:  [whole-set stats]                // regr_* family, corr — drawn as annotations
}
```

- **`Engine::chart` composes these four slots into one DataFrame plan** over `__snap_{id}` and
  pivots the result. It does not know what a "candlestick" is.
- **A chart type is a preset**: bar = 1 group + 1 measure; candlestick = `date_bin` group + 4
  order-sensitive measures; ECDF = no group + one window op; Pareto = 1 group + 1 measure + rank +
  running-share windows; heatmap = 2 groups + 1 measure. The renderer keys marks off the preset,
  the engine only ever sees the algebra.
- **Guardrails stay uniform** because every preset flows through the same `group_cap` /
  `raw_cap` / `LIMIT cap+1` detection — a new chart type inherits refusal behavior instead of
  reimplementing it.
- **The scaffold stays total**: each algebra slot has a canonical SQL rendering (that is what the
  window/aggregate columns above are), so **every** chart the system can draw can be handed to the
  user as an editable query — the no-shadow-language principle (spec §1.3) scales with the system
  instead of being outgrown by it.

This is why the workstream's task 01 builds `ChartQuery` as data, not as six code paths: widening
an enum of presets is additive; widening six pipelines is a rewrite.

## 3. Tiers — what to build, in what order

**Tier A — richer encodings, no new chart types** (extends tasks 01–04 directly):
aggregate menu grows `median` / `percentile_cont(p)` / `stddev` / count-distinct; **Top-N + Other**
replaces refusal as the first response to high cardinality (rank + CASE fold, cap becomes a
preference); **share-of-total** as a Y mode (100%-stacked bar/area, honest pie percentages);
`FILTER`-split series (one measure, several predicates) as an alternative to a series column.

**Tier B — new presets, each one engine query + one renderer** (successor of task 05's scope):
box plot (`percentile_cont` ×3 + whiskers), error bands (`avg` ± `stddev`), **candlestick**
(`first_value`/`last_value` ORDER BY + `min`/`max` over `date_bin`), **ECDF** (`cume_dist`),
Pareto (rank + running share + `ROLLUP` total), **heatmap** (two group exprs; calendar variant via
`date_part`), indexed comparison lines (`first_value` window), period-over-period delta (`lag`),
scatter trendline (`regr_*` — already specced in task 05).

**Tier C — system toggles**: gap-fill via `generate_series` + LEFT JOIN (explicit toggle, default
off = gaps); explicit `random() < p` scatter sampling (labeled on the chart, never automatic);
log-decade histogram binning; temporal *cast offers* for string/epoch columns (`to_timestamp` /
`from_unixtime` instead of the name-regex guess).

**Considered and rejected**: `bool_*`/`bit_*` reducers (nothing to encode); `TABLESAMPLE` (absent
in 54 — and its absence is load-bearing for the honesty principle); native gap-fill/`locf`
(absent; `generate_series` join is the mechanism); `width_bucket` (absent; `floor` arithmetic is
the mechanism, not a stopgap).

Renderer cost note: every Tier B mark is rects, lines, circles and polygons — nothing new from
`freya-plotters-backend`; the pie wedge (`fill_polygon`) is already the most exotic path used.

## 4. Where this lands in the workstream

Tasks 01–04 already build the right chassis (`ChartQuery` as data, uniform caps, one scaffold
funnel) — nothing in them changes shape because of this survey; Tier A widens their enums. Task
05 is the survey's delivery vehicle: its scope is Tier A + the Tier B presets, picked by value,
with Tier C as explicit toggles behind them. Per AGENTS.md §5, nothing in 01–04 pre-builds for a
tier that hasn't been picked up — the algebra already accommodates it, which is the point.
