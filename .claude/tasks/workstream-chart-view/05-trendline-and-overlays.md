# Chart 05 · Analytical charts — the function-map tiers (follow-on)

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01–04 · **Optional-shaped:** the
chart is complete without it; nothing earlier may pre-build for it (AGENTS.md §5).

## Goal
Deliver `docs/CHART_FUNCTIONS.md`'s tiers on the 01–04 chassis: richer encodings (Tier A), new
chart presets (Tier B), and the explicit system toggles (Tier C) — every one of them a widening of
`ChartQuery`'s data, an extra measure/window expr in `Engine::chart`, and a renderer preset. Never
a client-side computation, and every new capability keeps scaffold parity (a SQL form the user can
own — spec §1.3).

## Build (pick by value, in tier order; each item is independently shippable)
- **Tier A — richer encodings**: aggregate menu grows `median` / `percentile_cont(p)` / `stddev` /
  count-distinct; **Top-N + Other** (rank + CASE fold) as the constructive answer to high
  cardinality — the group-cap refusal becomes the fallback, not the first response;
  **share-of-total** Y mode (`sum(y) OVER (PARTITION BY x)` → 100%-stacked bar/area, honest pie
  percentages); `FILTER`-split series.
- **Tier B — new presets** (one engine query + one renderer each): scatter **trendline**
  (`regr_slope/intercept/r2`, drawn dashed with an R² label); **overlays** — moving average
  (window `clamp(round(n/8), 2, 12)`) and running total as dashed series folded into the y-range;
  **box plot** (`percentile_cont` ×3 + whiskers); **error bands** (`avg` ± `stddev`);
  **candlestick** (`first_value`/`last_value ORDER BY` + `min`/`max` over `date_bin`); **ECDF**
  (`cume_dist`); **Pareto** (rank + running share); **heatmap** (two group exprs; calendar variant
  via `date_part`); indexed comparison (`first_value` window); period delta (`lag`).
- **Tier C — explicit toggles**: gap-fill (`unnest(generate_series(…))` LEFT JOIN; default off =
  gaps); labeled `random() < p` scatter sampling (never automatic); log-decade histogram bins;
  temporal cast offers (`to_timestamp` / `from_unixtime`) replacing the name-regex guess.
- All choices join `ChartConfig` (03's channel + persistence carry them for free); all new marks
  are rects/lines/circles/polygons — no new backend surface. Verify each function against the
  live registry at build time, and extend the scaffold's expr → SQL mapping in the same change as
  each capability.

## Acceptance
- [ ] Each shipped capability computes in `Engine::chart` (core test), renders themed, and emits
      its SQL form through the scaffold when active. No capability computes client-side.

## References
`docs/CHART_FUNCTIONS.md` (the survey + tiers). `docs/CHART_SPEC.md` §5, §10.
