# Workstream — Chart view (Rz2)

The results **Chart** surface: a **renderer-first** chart over the result set — snapshot ordinal,
a projected read + long→wide pivot, a plotters/Skia renderer, an encoder strip, guardrails, and
the SQL scaffold as the aggregation path. Switched into by the results Table/Chart segment
(P2-07, done). Spec: **`docs/CHART_SPEC.md`** — the committed renderer-first spec; the
design-handoff bundle's CHART_SPEC + `screenshots/chart-*.png` are the *visual* reference only.

## What this workstream settled (do not re-litigate)

- **The chart computes nothing SQL can say.** The first design's engine-side aggregation pipeline
  (`AggFn`/`Bucket`/`Stride`, auto-stride, imposed category order) was **built, adversarially
  reviewed twice, and withdrawn**: the hard defects clustered in it, and its ordering fought the
  user's own `ORDER BY` — a `GROUP BY` has no output order, so re-aggregating an already-shaped
  result destroys the order the user asked for. Full evidence:
  `docs/reference/INVARIANTS.md` (the chart entry). The histogram is the one computed mark.
- **Result order is real, and it is the snapshot ordinal** (`SNAPSHOT_SPEC.md` §9). Measured: a
  bare snapshot read is nondeterministic above 10 MB — the *grid's paging* is affected today, so
  task 00 is a bug fix that happens to unblock the chart, not chart pre-work.
- **Aggregation's on-ramp is the scaffold**, promoted from escape hatch to the normal workflow:
  raw data → one click → an editable `GROUP BY` tab → chart the shaped result.
- **Refuse, never sample** — over-cap and pivot-duplicates both refuse with a CTA into SQL.
- **Rendering is `freya-plotters-backend`** (fork, `plot` feature) on a `canvas`;
  `CanvasContext` is logical-units and pre-scaled; redraw needs an explicit request.
- **Chart data is a freya-query capability shaped like `PageSpec`** — keyed
  `(SnapshotId, ChartQuery)`, no confirm dialog, store holds the config, never results.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 00 | Snapshot ordinal + ordered reads `[core]` | ✅ | Rz2 (P2-01 fix) | — |
| 01 | `Engine::chart` renderer-first read + vocabulary `[core]` | ✅ | Rz2 | 00 |
| 02 | Chart body + plotters renderer | ✅ | Rz2 | 01 |
| 03 | Encoder strip + `ChartConfig` state | ⬜ | Rz2 | 02 |
| 04 | Guardrails + the SQL scaffold | ⬜ | Rz2 | 02, 03 |
| 05 | Analytical presets — role mappings + templates (follow-on) | ⬜ | Rz2 | 01–04 |

## Why the order

00 is a standalone correctness fix to the snapshot read path (stable paging is `SNAPSHOT_SPEC.md`
§1's own promise) and the chart's order guarantee rides on it. 01 re-cuts the engine read to the
renderer-first shape — the branch holds the withdrawn pipeline's implementation, and its
salvageable parts (caps, `CellFormat` labels, `(null)` handling, pivot collision refusal,
histogram, pin, most tests) carry over. 02 makes the surface real over schema-derived defaults;
03 adds the strip and persisted config; 04 adds the refusal surfaces and the scaffold. 05 is
deliberately last and optional-shaped: presets are role mappings + SQL templates over the same
`Rows` read, so nothing earlier pre-builds for them (AGENTS.md §5).

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core]` logic in `strata-core`.
