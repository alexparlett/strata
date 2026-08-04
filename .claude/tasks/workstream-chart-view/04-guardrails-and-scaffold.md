# Chart 04 · Guardrails + the SQL scaffold

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 02, 03

## Goal
The refusal surfaces and the way out of them: guardrail overlays, the high-cardinality banner,
and the **Aggregate in SQL** scaffold into a new tab — promoted from escape hatch to the normal
raw-data workflow. Spec: `docs/CHART_SPEC.md` §7–§8.

**The chart still aggregates nothing.** The scaffold writes SQL *text* into a new editable tab
and never runs it; what comes back is an ordinary result the chart renders like any other. Read
"the aggregation path" as "where aggregation went when it left the engine" — the user's own
query — not as anything the chart does. Without it the refusals 02 already states are dead ends:
spec §1.4 promises a message **and a CTA into SQL**, and today only the message exists.

## Current state
01 answers the refusals as data (`OverCap`, `Duplicates`); 02/03 render and configure.

02 **states** every refusal already, as a centred title + body `Notice` in place of the canvas
(`results/chart/mod.rs`: `notice` for the engine's two and for every shape that would
otherwise paint nothing, `encode`'s `Err` for the encodings a schema cannot satisfy). What it does not do is offer a way out: this task adds the icon tile and
the **Aggregate in SQL** CTA beneath that same copy, and the high-cardinality banner. The
messages are the deliverable of that surface, so re-word them here if the CTA changes what they
should say — do not leave two copies.

03 moved `encode` (and its `Err` messages) into `results/chart/config.rs`, where the encoding is
resolved from the tab's `ChartConfig`. Two things follow for this task: the **scaffold reads the
resolved `Encoding`** (`config::resolve`'s output — real column names, already checked against
the result), not the stored config; and the encodings a *choice* can reach are now narrower than
the schema's — the option sets make an invalid encoding unreachable, so what is left for an
overlay is genuinely "nothing valid to offer" (no measure at all, fewer than two for a scatter,
no category for a pie) plus the deliberately-emptied Y.

## Build
- **Overlays** (in place of the canvas; icon + title + body + optional CTA, IDE-terse copy):
  `OverCap` (1 000 rows; pie 24; scatter 6 000 points) → scaffold CTA; `Duplicates { x, series }`
  ("more than one row per category — aggregate in SQL") → scaffold CTA; no valid Y / histogram
  without a numeric column → instructional, no CTA. All driven by `ChartData`'s refusal variants
  / the config — never by re-deriving in the UI. There is **no materialize cap, no sampling, and
  no aggregation fallback** (settled — spec §1.2, §7).
- **Banner**: non-blocking, across the canvas top, past 60 categories (`axis.labels.len()` — no
  extra query). Chart still renders beneath. **Wear the dress that exists**: the Export window
  already ships this banner (`apps/export/mod.rs` — `warning_background` / `warning_border_fill`,
  taken from the sheet's semantic `warning`). Reuse it rather than minting a second warning tone.
- **Scaffold**: build the SQL from the current encoding — `SELECT <x>[, <series>],
  SUM(<y>) AS sum_<y>` (or `COUNT(*) AS n`) `FROM ( <the tab's SQL, verbatim> ) GROUP BY …
  ORDER BY …`; temporal X uses `date_bin(interval '1 day', <x>)` as a starting stride the user
  edits (the engine no longer guesses spans) and orders by the bucket ascending, otherwise by the
  measure descending. Open through the existing funnel (`session.open_named`), **never auto-run**.
  Offered from the healthy chart too — it is the normal path from raw data to a chartable shape,
  not only the refusal CTA. Quote identifiers the way export's `select_sql` does.
- **Which SQL it wraps, and where to get it.** The **run's** SQL — `QuerySpec::sql`, the query
  that produced the snapshot being charted — never the tab's editor buffer, which may have moved
  on since that run and would scaffold a query over data nobody is looking at. `ChartView` is not
  handed it today (`ChartView::new(ws, find, export, snapshot, columns)`) and `ExportLaunch` does
  not carry it either, so this task adds the prop; `self.spec` is in scope at the call site
  (`results/mod.rs`, the Chart arm), so it is one line.

## Acceptance
- [ ] Each guardrail condition shows its overlay with working CTA; the banner appears past 60
      categories and blocks nothing; no silent truncation or aggregation anywhere.
- [ ] The scaffold opens a new editable tab with correct, runnable SQL (categorical and temporal
      forms), and does not run it.

## References
`docs/CHART_SPEC.md` §7–§8. `state/session.rs` (`open_named`), `engine/export.rs` (identifier
quoting precedent). Guardrail copy per AGENTS.md §3 (IDE register, single-quoted identifiers).
