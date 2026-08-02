# Chart 05 · Analytical presets — role mappings + templates (follow-on)

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01–04 · **Optional-shaped:** the
chart is complete without it; nothing earlier may pre-build for it (AGENTS.md §5).

## Goal
Deliver `docs/CHART_FUNCTIONS.md`'s tiers on the renderer-first chassis: an analytical chart is
an **ordinary SQL result whose columns play named roles**, plus the template that writes that SQL.
Never an engine-side computation — spec §1.2 and §10.

## Build (pick by value, in tier order; each item independently shippable)
- **Tier A — scaffold/snippet templates** beyond plain `GROUP BY`: Top-N + Other (rank + CASE
  fold — the constructive answer to high cardinality, with the cap refusal as the fallback);
  share-of-total (`sum(y) OVER …` → 100%-stacked bar/area, honest pie percentages);
  `FILTER`-split series. Each lands the user in an editable tab they own.
- **Tier B — mark presets** (a role mapping + a renderer + its SQL template, one preset each):
  **box plot** (p25/p50/p75 + whisker columns — the user's `percentile_cont … WITHIN GROUP`);
  **error bands** (`y`, `y_lo`, `y_hi`); **candlestick** (open/high/low/close over a
  `date_bin`ned X — `first_value`/`last_value ORDER BY` + `min`/`max`); **ECDF** (`cume_dist()
  OVER (ORDER BY x)` as a line); **Pareto** (measure + running-share columns); **heatmap** (two
  group columns); indexed comparison (`first_value` window); period delta (`lag`). Each preset's
  roles join `ChartConfig` (03's channel + persistence carry them for free); every mark is
  rects/lines/circles/polygons — no new backend surface.
- **The one candidate for engine computation**: the scatter **trendline**
  (`regr_slope`/`regr_intercept`/`regr_r2` in a single call, drawn dashed with an R² label).
  Weigh it here as the exception it would be — a computed overlay the user cannot reasonably
  re-write per keystroke — and record the verdict either way.
- **Tier C — toggles and rescues**: gap-fill template (`unnest(generate_series(…))` LEFT JOIN;
  default off = gaps); labeled `random() < p` scatter sampling (never automatic); log-decade
  histogram binning; temporal cast offers (`to_timestamp` / `from_unixtime`) replacing the
  name-regex guess.

## Acceptance
- [ ] Each shipped preset renders from columns the user's SQL produced, ships the template that
      produces them, and keeps scaffold parity (spec §1.3). Nothing computes engine-side except
      a recorded, weighed trendline decision.

## References
`docs/CHART_FUNCTIONS.md` (the survey + tiers). `docs/CHART_SPEC.md` §4–§5, §10.
