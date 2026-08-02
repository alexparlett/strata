# Chart 01 · `Engine::chart` renderer-first read + vocabulary `[core]`

**Workstream:** Chart (Rz2) · **Status:** ✅ · **Depends on:** 00 (the ordinal)

## Goal
The data half of the chart, renderer-first: a `ChartQuery` → `ChartData` vocabulary in
`strata-model` and an `Engine::chart(snapshot, query)` that **reads** — projected columns, ordinal
order, `LIMIT cap + 1` — and pivots long→wide in Rust. No aggregation, no bucketing, no imposed
order. Spec: `docs/CHART_SPEC.md` §4–§5.

## Current state
Done — the re-cut landed. The withdrawn first design (an engine-side aggregation pipeline:
`AggFn`/`Measure`/`Bucket`/`Stride`/`Width`, auto-stride, axis builders, measure-descending
order) was built, adversarially reviewed twice, and replaced wholesale by the renderer-first
read; the why lives in `docs/reference/INVARIANTS.md` (the chart entry) and
`docs/CHART_SPEC.md` §1.2. `engine/chart.rs` went from ~2 200 lines to ~600 plus tests: a
projection (`sort(ordinal)` + `limit(cap+1)` plans as a TopK, so memory is O(cap) however
large the snapshot), the pivot keyed on `ScalarValue` pairs with an occupancy check answering
`Duplicates`, `Axis.positions` for numeric/temporal/clock X, and the salvage list below.
Tests: 21 unit cases in `engine::chart`, 6 facade cases in `tests/engine_chart.rs` (the lead
one: a result the user `ORDER BY`ed draws in exactly that order, and the grid agrees).

## Build
- **strata-model** (`chart.rs`): per spec §5 —
  `ChartQuery::{ Rows { x, ys, series, cap }, Raw { x, y, cap }, Histogram { col, bins } }`;
  `ChartData::{ Table { axis, series }, Points, Bins, OverCap { unit, cap }, Duplicates { x, series } }`;
  `Axis { labels, positions: Option<Vec<Option<f64>>> }`. Hash/Eq throughout (`ChartQuery` is
  cache identity). `AggFn`, `Measure`, `Bucket`, `Stride`, `Width` are **deleted** — nothing else
  consumes them.
- **Engine::chart** (`engine/chart.rs`): `Rows` = select referenced columns + ordinal,
  `ORDER BY <ord>`, `LIMIT cap + 1`, then pivot. `cap + 1` rows → `OverCap`. Pivot cell identity
  is the (X, series) **value pair** (`ScalarValue` — never renderings; NULL and a literal
  `"(null)"` stay distinct); a second row in one cell → `Duplicates`, carrying the encoding
  names. NULL Y → `None` (a gap). `positions` from a numeric/temporal X (epoch ms). No series
  column → no pivot: one category per row, duplicates draw.
- **Salvage from the branch** (keep, with their tests): the in-call snapshot pin; `CellFormat`
  label rendering + `DISPLAY_CHARS` clip; `(null)` as label-never-key; the `plottable` type
  refusal for `Raw`/`Histogram`; the finite-values filter; the whole histogram implementation
  (min/max pass, `√n` clamped 6..=24, ≤ 200 bins); the empty-result → empty-axis rule; the
  `measure_alias`-style name-escalation idea (now 00's, for the ordinal).
- **Delete from the branch**: everything the pipeline needed — aggregation exprs, axis builders,
  stride ladder + widening loop, `by_measure` ordering, the bucket-kind refusals, and their tests.
- Unit tests over in-memory fixtures per shape: pivot correctness (values, gaps, series naming —
  multi-Y, series column, both), duplicate refusal, `(null)` vs `"(null)"`, caps at exactly
  `cap` / `cap + 1`, positions for numeric/temporal/categorical X, empty result, scatter finite
  filter, histogram matrix. Facade tests over a real spooled snapshot: order agrees with the
  grid's pages (via 00), a retired snapshot fails cleanly.

## Out of scope
Sorting (the `sort` view transform is client-side, 03 — any float comparator there must be total,
`total_cmp` with NaN last; the first build's `sort_by` panic is the standing lesson). Presets
(05). Any UI.

## Acceptance
- [ ] `Engine::chart` answers all three shapes over a real snapshot: result order, correct pivot,
      gaps, honest `OverCap`/`Duplicates` refusals, positions where X is orderable; core tests
      cover the matrix above; none of the withdrawn vocabulary remains exported.

## References
`docs/CHART_SPEC.md` §4–§5. `docs/SNAPSHOT_SPEC.md` §9 (the ordinal this reads through).
`docs/reference/INVARIANTS.md` (the chart + ordinal entries — the full history).
`engine/query.rs` (`read_page` precedent), `engine/export.rs` (`select_sql`).
