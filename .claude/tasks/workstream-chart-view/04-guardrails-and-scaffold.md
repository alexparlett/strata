# Chart 04 · Guardrails + the SQL scaffold

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 02, 03

## Goal
The refusal surfaces and the aggregation path: guardrail overlays, the high-cardinality banner,
and the **Aggregate in SQL** scaffold into a new tab — promoted from escape hatch to the normal
raw-data workflow. Spec: `docs/CHART_SPEC.md` §7–§8.

## Current state
01 answers the refusals as data (`OverCap`, `Duplicates`); 02/03 render and configure. Nothing
consumes the refusals yet.

## Build
- **Overlays** (in place of the canvas; icon + title + body + optional CTA, IDE-terse copy):
  `OverCap` (1 000 rows; pie 24; scatter 6 000 points) → scaffold CTA; `Duplicates { x, series }`
  ("more than one row per category — aggregate in SQL") → scaffold CTA; no valid Y / histogram
  without a numeric column → instructional, no CTA. All driven by `ChartData`'s refusal variants
  / the config — never by re-deriving in the UI. There is **no materialize cap, no sampling, and
  no aggregation fallback** (settled — spec §1.2, §7).
- **Banner**: non-blocking, across the canvas top, past 60 categories (`axis.labels.len()` — no
  extra query). Chart still renders beneath.
- **Scaffold**: build the SQL from the current encoding — `SELECT <x>[, <series>],
  SUM(<y>) AS sum_<y>` (or `COUNT(*) AS n`) `FROM ( <the tab's SQL, verbatim> ) GROUP BY …
  ORDER BY …`; temporal X uses `date_bin(interval '1 day', <x>)` as a starting stride the user
  edits (the engine no longer guesses spans) and orders by the bucket ascending, otherwise by the
  measure descending. Open through the existing funnel (`session.open_named`), **never auto-run**.
  Offered from the healthy chart too — it is the normal path from raw data to a chartable shape,
  not only the refusal CTA. Quote identifiers the way export's `select_sql` does.

## Acceptance
- [ ] Each guardrail condition shows its overlay with working CTA; the banner appears past 60
      categories and blocks nothing; no silent truncation or aggregation anywhere.
- [ ] The scaffold opens a new editable tab with correct, runnable SQL (categorical and temporal
      forms), and does not run it.

## References
`docs/CHART_SPEC.md` §7–§8. `state/session.rs` (`open_named`), `engine/export.rs` (identifier
quoting precedent). Guardrail copy per AGENTS.md §3 (IDE register, single-quoted identifiers).
