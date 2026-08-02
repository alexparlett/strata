# Chart 04 · Guardrails + GROUP BY scaffold

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 02, 03

## Goal
The refusal surfaces and the user-owned escape hatch: guardrail overlays, the high-cardinality
banner, and the **Add GROUP BY in SQL** scaffold into a new tab. Spec: `docs/CHART_SPEC.md` §7–§8.

## Current state
01 already *reports* the facts (`group_count`, `capped`); 02/03 render and configure. Nothing
consumes the facts yet.

## Build
- **Overlays** (in place of the canvas; icon + title + body + optional CTA, IDE-terse copy):
  over `group_cap` groups (1 000; pie 24) → scaffold CTA; raw/scatter over `raw_cap` (6 000
  points) → scaffold CTA; histogram without a numeric column and scatter without numeric X+Y →
  instructional, no CTA. All driven by `ChartData`'s reported facts / the config — never by
  re-deriving in the UI. There is **no materialize cap and no sampling** (settled).
- **Banner**: non-blocking, across the canvas top, when an aggregated chart exceeds 60 groups
  (`group_count` — no extra query). Chart still renders beneath.
- **Scaffold**: build the SQL from the current encoding — `SELECT <x>[, <series>],
  <fn>(<y>) AS <fn>_<y>` (or `COUNT(*) AS n`) `FROM ( <the tab's SQL, verbatim> ) GROUP BY …
  ORDER BY …`; temporal X uses `date_bin` with the currently-selected stride and orders by the
  bucket ascending, otherwise order by the measure descending. Open through the existing funnel
  (`session.open_named`), **never auto-run**. Offered from the healthy chart too (promotion), not
  only from refusals. Quote identifiers the way export's `select_sql` does.

## Acceptance
- [ ] Each guardrail condition shows its overlay with working CTA; the banner appears over 60
      groups and blocks nothing; no silent truncation anywhere.
- [ ] The scaffold opens a new editable tab with correct, runnable SQL (categorical and temporal
      forms), and does not run it.

## References
`docs/CHART_SPEC.md` §7–§8. `state/session.rs` (`open_named`), `engine/export.rs` (identifier
quoting precedent). Guardrail copy per AGENTS.md §3 (IDE register, single-quoted identifiers).
