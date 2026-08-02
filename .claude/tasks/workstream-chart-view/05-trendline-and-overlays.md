# Chart 05 · Trendline + overlays (follow-on)

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01–04 · **Optional-shaped:** the
chart is complete without it; nothing earlier may pre-build for it (AGENTS.md §5).

## Goal
The analytical extras, all engine-side: a scatter trendline, line/area overlays (moving average,
running total), and a richer aggregate menu. Spec: `docs/CHART_SPEC.md` §10.

## Build
- **Trendline** (scatter, opt-in control): `regr_slope(y,x)`, `regr_intercept(y,x)`,
  `regr_r2(y,x)` computed in the same `Engine::chart` call (extend `ChartQuery::Raw` with a flag,
  `ChartData` with the fit) — never a client-side least-squares. Drawn dashed with an R² label.
- **Overlays** (line/area, opt-in control): moving average
  (`avg(y) OVER (ORDER BY x ROWS BETWEEN k PRECEDING AND CURRENT ROW)`, window
  `clamp(round(n/8), 2, 12)`) and running total (`sum … UNBOUNDED PRECEDING …`) as extra engine
  window exprs, returned as dashed overlay series and folded into the y-range. Overlay choice
  joins `ChartConfig` (03's channel + persistence carry it for free).
- **Aggregate menu**: add `approx_percentile_cont` / `stddev` to `AggFn` if wanted — verify
  against the live registry, extend the scaffold's fn → SQL mapping in the same change.

## Acceptance
- [ ] Trendline and each overlay render from engine-computed values, follow theming, and appear
      in the scaffold SQL when active (window fn / regr forms).

## References
`docs/CHART_SPEC.md` §10. DataFusion 54 window fns + `regr_*` (verify via the registry, not docs).
