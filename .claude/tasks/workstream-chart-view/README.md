# Workstream — Chart view (Rz2)

The results **Chart** surface: engine-side chart data, a plotters/Skia renderer, an encoder strip,
guardrails, and the GROUP BY scaffold. A whole feature surface switched into by the results
Table/Chart segment (P2-07, done). Spec: **`docs/CHART_SPEC.md`** — the committed, grounded spec;
the design-handoff bundle's CHART_SPEC + `screenshots/chart-*.png` are the *visual* reference only.

## What this rebuild settled (do not re-litigate)

- **Aggregation is the engine's, not the client's.** Every snapshot is already a DataFusion table
  (`__snap_{id}`); `Engine::chart` groups/bins there and returns a small `ChartData`. The old
  "client aggregate over the snapshot" task and the handoff's 200k materialize cap are gone —
  an aggregated chart over millions of rows is a normal hash aggregation.
- **Temporal X buckets with `date_bin` in the engine query itself** (stride auto from span,
  user-overridable), not only in the SQL scaffold. Empty buckets are gaps, never interpolated.
- **Rendering is `freya-plotters-backend`** (fork, `plot` feature) on a `canvas` — not a
  hand-rolled axis/tick/mark stack. `CanvasContext` is logical-units and pre-scaled; redraw needs
  an explicit request (`RenderCallback` never diffs unequal).
- **Chart data is a freya-query capability shaped like `PageSpec`** — keyed by
  `(SnapshotId, ChartQuery)`, no confirm dialog, store holds the config, never results.
- **Refuse, never sample** — DataFusion has no `TABLESAMPLE`; over-cap is an overlay + scaffold CTA.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 01 | `Engine::chart` + chart vocabulary `[core]` | ⬜ | Rz2 | — (P2-01 ✅) |
| 02 | Chart body + plotters renderer | ⬜ | Rz2 | 01 |
| 03 | Encoder strip + `ChartConfig` state | ⬜ | Rz2 | 02 |
| 04 | Guardrails + GROUP BY scaffold | ⬜ | Rz2 | 02, 03 |
| 05 | Trendline + overlays (follow-on) | ⬜ | Rz2 | 01–04 |

## Why the order

01 is pure `strata-core` + `strata-model` and unit-testable without any UI — it settles the data
contract everything else consumes. 02 makes the surface real (canvas, renderers, theme) over
schema-derived default encodings, so it is demonstrable before any controls exist. 03 adds the
controls and the persisted config; 04 adds the refusal surfaces and the scaffold, which need both
the data facts (01) and the config (03). 05 is deliberately last and optional-shaped: every piece
of it (regr_*, window fns) extends `Engine::chart` without touching the earlier contracts.

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core]` logic in `strata-core`.
