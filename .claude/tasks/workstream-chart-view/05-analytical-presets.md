# Chart 05 · Analytical presets — the remaining menu (follow-on)

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 07, 10 · **Optional-shaped:**
the chart is complete without it; nothing earlier may pre-build for it (AGENTS.md §5).

**Re-scoped 2026-08-07.** The 2026-08 chart redesign planning split this task's original
contents into owned tasks: Tier A templates are **07**, the first Tier B slice (heatmap,
error bands, box plot) is **10**, the trendline decision is **11** (verdict: build,
engine-side), and the "where does a template land" question is **settled** — fixed
templates live in the **command palette** (07); the interactive composer is the **Shape
panel** (09). This file holds what remains of `docs/CHART_FUNCTIONS.md`'s menu.

## Goal
Deliver the remaining tiers on the renderer-first chassis: an analytical chart is an
**ordinary SQL result whose columns play named roles**, plus the template that writes that
SQL. Never an engine-side computation — spec §1.2 and §10 (the trendline, 11, is the one
recorded exception).

## Build (pick by value; each item independently shippable)
- **Tier B remainder** (the 10 pattern — a role mapping + renderer + template each):
  - **Candlestick**: open/high/low/close fields on `ChartConfig` over a `date_bin`ned X —
    `first_value`/`last_value ORDER BY` + `min`/`max` in the template; the box-plot
    vocabulary pattern (10) again.
  - **ECDF** (`cume_dist() OVER (ORDER BY x)` as a line) and **Pareto** (measure +
    running-share column): ship as **templates only** over the existing Line mark until
    proven to need marks of their own — revisit if the template form disappoints.
  - **Indexed comparison** (`first_value` window normalize-to-100) and **period delta**
    (`lag`): template-only candidates likewise.
- **Tier C — toggles and rescues**:
  - Gap-fill template: `unnest(generate_series(…))` LEFT JOIN calendar; default off = gaps.
  - Labeled scatter sampling: an explicit `WHERE random() < p` template — never automatic.
  - Log-decade histogram binning (`floor(log10(x))` buckets) — the one item that may touch
    `ChartQuery::Histogram`; weigh against 06's renderer-side log axis first.
  - **Temporal cast offers** (`to_timestamp` / `from_unixtime`) for a column that holds a
    timestamp in a type that isn't one. This **adds** an offer; a role from a column *name*
    stays ruled out by the invariant (`chart_role` matches the Arrow `DataType` and nothing
    else). The cast is the user's, in their own SQL.

## Acceptance
- [ ] Each shipped preset renders from columns the user's SQL produced and ships the
      template that produces them (spec §1.3). Nothing new computes engine-side.

## References
`docs/CHART_FUNCTIONS.md` (the survey + tiers). `docs/CHART_SPEC.md` §4–§5, §10.
`07-tier-a-templates.md`, `09-shape-panel.md`, `10-tier-b-marks.md`, `11-trendline.md`.
