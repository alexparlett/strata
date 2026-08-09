# Workstream — Chart view (Rz2)

The results **Chart** surface: a **renderer-first** chart over the result set — snapshot ordinal,
a projected read + long→wide pivot, a plotters/Skia renderer, an encoder strip and guardrails.
Aggregation is the user's own SQL and V1 writes none of it. Switched into by the Table/Chart segment
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
- **Aggregation is the user's own SQL, and V1 does not write it for them** (04). The
  *Aggregate in SQL* press — a `GROUP BY` composed from the encoding, opened unrun in a new tab
  — was built and **cut**: sound mechanism, wrong surface (no comparable tool puts it among the
  encoders), and it was standing in for the chart-side aggregation actually worth building.
  `docs/CHART_SPEC.md` §8 has the full reasoning. Do not re-add it to the strip.
- **Refuse, never sample** — over-cap and pivot-duplicates both refuse, and the message names
  the fix in prose.
- **Rendering is `freya-plotters-backend`** (fork, `plot` feature) on a `canvas`;
  `CanvasContext` is logical-units and pre-scaled; redraw needs an explicit request.
- **Chart data is a freya-query capability shaped like `PageSpec`** — keyed
  `(SnapshotId, ChartQuery)`, no confirm dialog, store holds the config, never results.
- **The config is intent, and the option sets are the constraint** (03). Unset channels take the
  schema's defaults; a reference the result cannot answer falls back at *read* time and is never
  written out of the config. What a control offers is the mark's own option set, so an invalid
  encoding is unreachable rather than reported. The sort is a view transform over the settled
  data, never part of the read.
- **A chart image is the chart** (08). Copy Image renders the canvas's own `Rc<Frame>` through
  the same `marks::draw`, which is why `draw` takes a canvas + a `FontCollection` (never a
  `CanvasContext`) and **returns** its hit regions. No paint pass: the font collection is a root
  context, so the plan's capture-during-paint slot was never needed. The fork's clipboard grew
  images rather than the app growing a save-to-PNG stopgap, by **replacing** copypasta with
  arboard — one backend, because text and images are one clipboard — **inside** the existing
  `Box<dyn ClipboardProvider>` seam, which the integrations still fill. Deleting that seam was
  tried and rejected.
- **A time column is two roles** (04): `Instant` (date/timestamp) and `Clock` (time of day) are
  identical on an axis and differ wherever a stride does — DataFusion refuses a day-wide
  `date_bin` over a `Time`. Nothing in V1 reads the distinction; it is kept because recovering
  it later means reading a type's spelling, which the role invariant rules out.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 00 | Snapshot ordinal + ordered reads `[core]` | ✅ | Rz2 (P2-01 fix) | — |
| 01 | `Engine::chart` renderer-first read + vocabulary `[core]` | ✅ | Rz2 | 00 |
| 02 | Chart body + plotters renderer | ✅ | Rz2 | 01 |
| 03 | Encoder strip + `ChartConfig` state | ✅ | Rz2 | 02 |
| 04 | Guardrails (overlays + banner) | ✅ | Rz2 | 02, 03 |
| 05 | Analytical presets — the remaining menu (follow-on) | ⬜ | Rz2 | 07, 10 |
| 06 | Interactivity — bins, legend toggle, log axis, crosshair | ⬜ | Rz2 | 01–04 |
| 07 | Tier A templates — palette commands that write the SQL | ⬜ | Rz2 | 01–04 |
| 08 | Copy chart as image — fork clipboard + offscreen capture | ✅ | Rz2 | 02 |
| 09 | Shape panel — the aggregation composer, its own surface | ⬜ | Rz2 | 01–04 |
| 10 | Tier B marks — heatmap, error bands, box plot | ⬜ | Rz2 | 01–04 |
| 11 | Scatter trendline `[core]` | ⬜ | Rz2 | 01–04 (10 for strip layout) |

## Why the order

00 is a standalone correctness fix to the snapshot read path (stable paging is `SNAPSHOT_SPEC.md`
§1's own promise) and the chart's order guarantee rides on it. 01 re-cuts the engine read to the
renderer-first shape — the branch holds the withdrawn pipeline's implementation, and its
salvageable parts (caps, `CellFormat` labels, `(null)` handling, pivot collision refusal,
histogram, pin, most tests) carry over. 02 makes the surface real over schema-derived defaults;
03 adds the strip and persisted config; 04 adds the refusal surfaces.

06–11 are the 2026-08 redesign (planned with Alex; the decisions each file marks "settled in
planning" are his — do not re-ask them). 06/07/08 were mutually independent quick wins — pick
up in any order, one session each (08 is done). 09 is independent of all of them (it re-promotes
`quote_col`, nothing else shared) and ships the INVARIANTS/CHART_SPEC §8 amendment. 10 comes
before 11 only because both touch the strip's layout. 07 and 10 both grow
`chart/templates.rs` — whichever lands second merges. 05 is what remains of the presets menu
after the split (candlestick, ECDF/Pareto-as-templates, Tier C) and stays last and
optional-shaped: nothing earlier pre-builds for it (AGENTS.md §5).

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core]` logic in `strata-core`.
