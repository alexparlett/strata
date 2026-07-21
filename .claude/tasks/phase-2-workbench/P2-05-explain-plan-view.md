# P2-05 · Explain-plan view

**Phase:** 2 — Workbench · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** Rz-plan / U8 · **Depends on:** P2-02/03

## Goal
Replace the placeholder plan body with the real three-tier EXPLAIN plan card.

## Current state
`results/explain_plan.rs` is a one-line placeholder ("Plan explanation…"). The engine already emits a
typed, pre-labelled `Vec<Metric>` (classified by `MetricValue` variant) + derived per-node self-time.

## Build
Build the card per `EXPLAIN_PLAN_SPEC.md` v3:
1. **Headline row** per node: rows · self-time · bytes · a time-share bar.
2. **Insight callouts** — the non-zero `plan::insights()` results.
3. **Collapsed grouped grid** — `plan::metric_group()`, hide-zeros, expandable.
4. Depth **guide-rails**, a 2-line detail clamp, an amber **ANALYZE** badge, and an active-tab summary.

## Acceptance
- [ ] Explain plan renders nodes with self-time + time-share; ANALYZE variants show the badge + real metrics.
- [ ] Insights show only when non-zero; grouped grid collapses/expands.

## Freya / references
- `docs/EXPLAIN_PLAN_SPEC.md` v3. Core: `strata-core::plan::{Metric, MetricKind, self_time_ms, insights, metric_group}`.
- Design canvas: the plan surface in `Results.dc.html`.
