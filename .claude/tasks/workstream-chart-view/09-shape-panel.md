# Chart 09 · Shape panel — the aggregation composer, in a surface of its own

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01–04 · Independent of 06–08,
10–11. **Amends the INVARIANTS chart entry + CHART_SPEC §8** — the docs change ships in this
task.

## Goal
The chart-side aggregation UX the cut press was standing in for (04), in the placement the
invariant invites: a **surface of its own** (DBeaver's Grouping panel; Metabase/Superset/
Looker eject from a menu — never the encoder strip). A dialog that composes **visible SQL**
— group-by columns with `date_bin` strides, per-measure aggregates, an explicit `ORDER BY` —
and opens it **unrun** in a new tab. Settled in planning (2026-08-07, Alex): approved as the
re-litigation of the placement; renderer-first stands untouched; the refusal overlays keep
**no control behind them** in this pass (a link to the panel is a separate future decision).

## Hard boundaries
- No engine aggregation: the aggregate vocabulary is a **UI-local enum rendering to SQL
  text** in this module. It must not enter strata-model, `ChartQuery`, or any engine type —
  that would resurrect the withdrawn `AggFn` pipeline.
- Generated SQL always carries its own `ORDER BY` (a `GROUP BY` has no output order).
- No live preview (the preview is the tab it opens); no persisted panel state (the SQL in
  the new tab is the only artifact).
- Top-N + Other fold is **not** built here — 07 owns rank+CASE (AGENTS.md §5). Leave the
  form able to take another section; build nothing for it.

## Current state / verified mechanisms
- Dialog chassis: `views/dialogs/` — a `State<Option<Target>>` provided at ProjectRoot
  (`apps/project/project.rs`, beside `drop_target` ~line 679), the dialog mounted at the
  root, the trigger elsewhere fills the slot; `drop_confirm.rs` (~line 930) already opens
  tabs from a dialog via `SessionState::open_named`. **Not** the Export child window — it
  deliberately carries only engine/app/log handles and cannot reach the opener's
  SessionState.
- The run's SQL: `QuerySpec.sql` via `session.request(tab)` — the panel wraps the SQL that
  produced the settled result, never the live buffer (04 settled this for the cut press).
- Column quoting: `quote_col` in `engine/export.rs` — private again since the press revert
  (04 "Reverted with it"); re-promote it as the press did. It is deliberately **not**
  `quote_ident` (which folds a bare word per `TableReference` semantics); `quote_col`
  always quotes with doubled quotes — the right form for column idents in composed SQL.
- Roles: the results pane already builds `Roles` from `ColumnInfo`; `Instant` vs `Clock`
  is the stride distinction this panel finally reads (04 kept the split for exactly this).

## Build
1. **`views/workbench/results/shape/`** (sibling of `chart/`):
   - `compose.rs` — `ShapeForm { groups: Vec<GroupPick>, measures: Vec<MeasurePick>,
     order: ShapeOrder }`, with UI-local `SqlAgg` (Sum/Avg/Min/Max/Count/Median) and
     `Stride` (minute/hour/day/week/month/year) enums, and
     `fn compose(form, sql) -> String`. Subquery form — `FROM (<run's SQL, per the Run
     press's statement extraction>) AS q` — not a CTE, to dodge WITH-inside-WITH when the
     user's SQL already has CTEs. Ordinal `GROUP BY 1, 2` (avoids repeating the `date_bin`
     expression); measure aliases `"{col}_{agg}"`, group columns keep their names; idents
     through the re-promoted `quote_col`. Pure, no Freya types.
   - `mod.rs` — `ShapeDialog` on the drop_confirm chassis; `ShapeTarget { tab, sql, roles }`
     (cheap clones; `PartialEq` on tab + sql, `ExportLaunch`'s reasoning).
2. **Contents** (all `components::form` — Form > Row > control, never bespoke rows):
   - **Group by**: a checkbox row per `Dimension` + time column. An `Instant` column gets a
     stride `Select` (None + minute…year → `date_bin(INTERVAL '…', "col")`); a `Clock`
     column only sub-day strides — DataFusion refuses a day-wide `date_bin` over a `Time`
     (measured, 04). `date_bin` takes a `Timestamp`: `Date32` coerces, `Date64` does not
     (04) — verify the `Date64` path and cast in the composed SQL if needed.
   - **Measures**: one row per `Measure` column, a `Select` of Skip + `SqlAgg`, plus a
     standalone "Row count (count(*))" toggle.
   - **ORDER BY**: a two-way choice — by group (ascending) / by first measure (descending).
     Always emitted.
3. **Trigger**: a results-toolbar "Shape…" action (`results/toolbar.rs`, folds per the one
   fold policy), visible in Grid **and** Chart views, enabled only on a settled data run.
   When the Chart view is active, seed group-by from the resolved encoding's X + series and
   measures from its Ys — the cut press's "composed from the encoding" value, without its
   placement.
4. **Output**: `open_named("{tab name} · shaped", sql, scratch-origin)`, unrun; close the
   dialog. Never replaces the current buffer.
5. **Docs, same change**: `docs/reference/INVARIANTS.md` (chart entry, the 04 guardrails
   paragraph) + `docs/CHART_SPEC.md` §8 gain the recording that the placement was
   re-litigated with a surface of its own, as the entry invites — cite this task. Update
   `05-analytical-presets.md`'s "note on where a template lands" (palette = fixed
   templates, 07; panel = the interactive composer, this task). AGENTS.md §2's one-liner
   stays true as written ("re-litigate the placement only with a surface that isn't the
   strip" — this is that surface); extend the line only if review finds it misleading.
6. **Tests**: golden SQL in `compose.rs`, plus an integration test running composed SQL
   through `EngineCtx::default()` over a fixture covering dates, `Date64`, clocks, NULL
   groups, reserved-word and uppercase column names.

## Acceptance
- [ ] Shape… over a settled run opens the dialog; the composed SQL is readable, quoted,
      ordered, and runs; the new tab is unrun and owned by the user.
- [ ] A `Clock` column is never offered a day-or-wider stride; a `Date64` group survives.
- [ ] Nothing new enters strata-model or the engine except `quote_col`'s visibility;
      refusal overlays are unchanged.
- [ ] The INVARIANTS/CHART_SPEC amendments land in the same change.

## References
`docs/CHART_SPEC.md` §8. `docs/reference/INVARIANTS.md` (chart entry).
`04-guardrails.md` ("What the scaffold cut settled" — the mechanism this revives, and the
revert list naming `quote_col`). Dialog chassis: `views/dialogs/drop_confirm.rs`.
