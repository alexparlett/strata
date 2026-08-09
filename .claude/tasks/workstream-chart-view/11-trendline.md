# Chart 11 · Scatter trendline — the one weighed engine computation

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01–04 · After 10 only for
strip layout. Records the CHART_SPEC §10 verdict.

## Goal
A dashed least-squares trendline over the scatter with an R² label, computed engine-side —
the one computed overlay spec §10 sanctioned for weighing. **Verdict (settled in planning,
2026-08-07, Alex): build it, engine-side.** Rationale to record in `docs/CHART_SPEC.md` §10
and here: the overlay is a function of the **encoding** (which two columns the scatter
currently plots), not of the query — templating it would rewrite the user's SQL on every
encoder gesture (which "config is intent" forbids) and smuggle two scalars through a rows
read by duplicating them onto every row. That is exactly the exception §10 pre-sanctioned.

## Current state
- `Engine::chart` (`strata-core/src/engine/chart.rs`) answers `ChartQuery::Raw` for the
  scatter; `ChartQuery` is the point-read's **cache identity** — the trendline must not
  touch it (toggling the overlay must never re-read the points).
- `ChartSpec` (`apps/project/query/chart.rs`) is the freya-query capability precedent:
  keyed `(snapshot, query, display config)`, `stale_time(MAX)`, `enable(readable)`.
- DataFusion 54 has the `regr_*` aggregate family (`regr_slope`, `regr_intercept`,
  `regr_r2` — verified against the pinned sources, `docs/CHART_FUNCTIONS.md` §5); one
  aggregation call computes all three.
- The strip's per-mark conditional sections (`chart/strip.rs`) and `config.rs` gate
  predicates are the pattern for a scatter-only toggle.

## Build
1. **Engine**: `Engine::trend(snapshot, x, y) -> Result<Trend>` beside `Engine::chart` —
   one aggregation over the snapshot (`regr_slope`/`regr_intercept`/`regr_r2` + count),
   filtered to finite pairs like the scatter read (`finite()`), holding the snapshot pin
   for the call. `Trend { slope, intercept, r2, n }` in `strata-model/src/chart.rs`.
   Degenerate answers (n < 2, zero x-variance → NULL slope) come back as `None`/absent —
   the overlay simply doesn't draw; never an error the user must dismiss.
2. **Capability**: `TrendSpec { snapshot, x, y }` beside `ChartSpec` in
   `apps/project/query/` — no display config in the key (numbers only), `stale_time(MAX)`,
   enabled iff scatter + toggle on + settled points. Do **not** extend `ChartQuery`.
3. **Config**: `ChartConfig` gains `#[serde(default)] trend: bool`; a strip toggle shown
   only for Scatter (gate predicate beside `sortable`); persists per tab like everything
   else.
4. **Renderer** (`marks.rs` scatter): dashed line clipped to the plotted x-range, drawn
   from `slope`/`intercept`; a small "R² = 0.87" label near an end, formatted through
   `axis::readout`'s style. Honors log_y if 06 has landed (draw in value space through the
   same Y coordinate — verify the composition; if it fights, scope the toggle to linear
   and banner it, recorded here).
5. **Docs**: amend `docs/CHART_SPEC.md` §10 (verdict + rationale) and tick the "recorded,
   weighed trendline decision" line in `05-analytical-presets.md`.
6. **Tests**: `crates/strata-core/tests/engine_chart.rs` — `Engine::trend` over a real
   spooled snapshot (known slope fixture, NaN/NULL rows excluded, degenerate n < 2);
   round-trip test on `TrendSpec` beside `ChartSpec`'s.

## Acceptance
- [ ] Toggling the trendline never re-reads the points (watch the chart capability's key);
      the overlay redraws with theme and resize; degenerate data draws no line and no
      error.
- [ ] The verdict and rationale are recorded in CHART_SPEC §10; the engine computes
      nothing else new.

## References
`docs/CHART_SPEC.md` §10. `docs/CHART_FUNCTIONS.md` §5 (regression family).
`apps/project/query/chart.rs` (`ChartSpec` — the capability pattern to copy).
