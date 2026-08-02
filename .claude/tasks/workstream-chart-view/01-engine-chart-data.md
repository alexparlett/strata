# Chart 01 · `Engine::chart` + chart vocabulary `[core]`

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** nothing open (P2-01 snapshots ✅)

## Goal
The data half of the chart, engine-side: a `ChartQuery` → `ChartData` vocabulary in `strata-model`
and an `Engine::chart(snapshot, query)` that runs the grouped / binned / raw read **in DataFusion
over the snapshot table** and returns a chart-ready model. Spec: `docs/CHART_SPEC.md` §5.

## Current state
Not built. The mechanism it rides is: every snapshot is registered as `__snap_{id}`
(`engine/query.rs` — `snapshot_name`, `register_arrow`); `read_page` (`engine/query.rs`,
`ctx.table` → DataFrame ops) and export's `select_sql` (`engine/export.rs`, SQL over the snapshot)
are the two precedents for reading it. Prefer the DataFrame shape.

## Build
- **strata-model** (`chart.rs`): `ChartQuery` —
  `Aggregate { x, series, measures: Vec<Measure>, bucket: Option<Bucket>, group_cap }` where
  `Measure { y, agg_fn }` and `Bucket` is a time stride for temporal X or a uniform width for
  numeric X; `Raw { x, y, cap }`, `Histogram { col, bins }`. The measure slot is **plural** even
  though 02–04 only ever send one: `ChartData`'s pivot returns N series regardless, and 05's
  presets (box plot, candlestick) are *additional measures on the same group*, not new shapes
  (`CHART_FUNCTIONS.md` §2) — a single-`y` field is the hardcoded subset the bar forbids. The
  window / derived-stat slots are **not** scaffolded here; they extend the struct additively when
  05 picks them up (AGENTS.md §5). `ChartData` — categories + per-series `Vec<Option<f64>>`
  (pivoted wide in the engine; a series comes from the series column, the measure list, or both),
  or points, or bins, plus `group_count` / `capped`. Hash/Eq throughout — `ChartQuery` is
  freya-query cache identity (02), so no floats in the request. `AggFn`:
  `sum | avg | min | max | count | median | count-distinct` (all DF 54 built-ins; the live
  registry is the truth, check nothing by name-list).
- **Engine::chart** in `strata-core`:
  - Temporal X → `date_bin(stride, x, epoch)` as the group expr; numeric X on bar/line/area is
    first-class too — grouped by value, or binned `floor(x / w) * w` when a width is set
    (spec §3, §5). Bucket resolution (auto from the column's span per spec §5, or the query's
    override) happens **before** the query is built, so the request stays concrete and cacheable.
  - Categorical X order = snapshot order via `min(row_number() OVER ())` per group; temporal and
    numeric X order by value ascending.
    **Verify with a test** that row_number over the registered IPC table follows file order; if it
    does not, fall back to measure-descending and record that in the spec.
  - NULL X / series is its own `(null)` group; missing (category, series) cells are `None`.
  - Caps detected with `LIMIT cap + 1` — over-cap is a reported fact, never a truncated chart.
  - Histogram: min/max pass then uniform bins (`min(24, max(6, ceil(sqrt(n))))`), engine-side.
- Unit tests in `strata-core` over fixture batches (the sanctioned arrow dev-dep): each agg fn,
  date_bin bucketing + gap (missing bucket absent, not zero), numeric X grouped and binned,
  snapshot-order preservation, null grouping, cap detection, series pivot.

## Out of scope
Trendline/overlays (05). Any UI. Any confirm gating — this is `fetch_page`-tier, not profile-tier.

## Acceptance
- [ ] `Engine::chart` answers all three query shapes over a real snapshot with correct values,
      order, gaps, `(null)` groups, and honest cap reporting; core tests cover the list above.

## References
`docs/CHART_SPEC.md` §3–§5, §7 (cap defaults). `docs/CHART_FUNCTIONS.md` §2 (the query algebra
this vocabulary is the chassis for). `docs/reference/ENGINE.md`.
`engine/query.rs` (`read_page`, `snapshot_name`), `engine/export.rs` (`select_sql`).
