# Chart 02 · Chart body + plotters renderer

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01

## Goal
The Chart results body: a `canvas` drawing the five marks (bar/line/area/pie/scatter) plus the
histogram through `PlotSkiaBackend`, fed by a freya-query subscription over `Engine::chart`,
themed, with a mark picker. Replaces the placeholder tile in `results/chart.rs`. Spec:
`docs/CHART_SPEC.md` §2, §4, §9.

## Current state
`ChartView` (`results/chart.rs`) renders the shared toolbar over a "isn't built yet" tile — the
switcher, per-tab mode and body slot are done (P2-07). `strata-freya` does **not** enable freya's
`plot` feature. The fork backend's `draw_pixel` is a reachable `todo!()` panic.

## Build
- **Fork first**: implement `draw_pixel` in `freya-plotters-backend` (1×1 fill; upstream-shaped,
  doc comment), then **push the fork** (AGENTS.md §6 — the gitlink trap). Add `plot` to
  `strata-freya`'s freya features.
- **Subscription**: a `QueryCapability` shaped exactly like `PageSpec`/`FetchSnapshotPage` — keys
  `(SnapshotId, ChartQuery)`, one construction site, `Engine::chart` behind it, no confirm dialog.
  Two wiring facts from 01: `Engine::chart` takes `self: &Arc<Self>` (for its in-call pin), so it
  is **not reachable through `EngineCtx`'s `Deref`** — add a thin wrapper on `EngineCtx` the way
  `pin_snapshot` has one. And axis labels render through the engine's live `datafusion.format.*`
  overrides, so the chart must re-render when those change (as the grid's pages do) — the cache
  key alone does not carry that dependency.
  This task derives the `ChartQuery` from schema defaults (spec §6 rules); 03 replaces that with
  the real `ChartConfig`.
- **Renderer**: one module per concern under `results/chart/` — a render fn per mark over
  `ChartData` via plotters `ChartBuilder` (bar/line/area/scatter/histogram cartesian; pie via
  plotters' `Pie` element — verified `fill_polygon`-only). `None` cells: gap on lines,
  zero-height bar — never interpolated. Line/scatter may place marks by `Axis.positions` when
  present (spec §5); equally-spaced labels are the fallback. Mesh per spec §9: ~5 gridlines,
  nice max, abbreviated ticks (`1.2k`/`3.4M`), thinned X labels, zero baseline when data spans
  negatives.
- **Canvas contract**: draw in logical units only (`CanvasContext.size` is logical, pre-scaled).
  Request a repaint when the settled data / config / theme changes — `RenderCallback` never diffs
  unequal, so the `feature_plot_3d` idiom (platform redraw request) applies; if that misbehaves,
  the fix is a fork-level revision key on `CanvasElement`, not an app workaround.
- **Theme**: a `chart` component theme in `theme.rs` (axis / grid / label colours) + the
  **categorical palette as 10 palette slots**; fonts from the theme family through the backend's
  text style — no `"sans"`. Then `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.
- **Mark picker**: the control strip's top control (SegmentedToggle or Select per the design
  visuals), writing the mark; switching marks is a repaint over the settled `ChartData`, never a
  re-query. The rest of the strip is 03's.

## Acceptance
- [ ] Selecting Chart renders the current result with schema-default encoding; every mark
      renders correctly and re-renders on mark change, data settle, theme change, and resize.
- [ ] NULL Y cells show as gaps, never interpolated; rows draw in result order; the chart reads
      every colour/font from the theme.

## References
`docs/CHART_SPEC.md` §9. Design visuals: handoff `Results.dc.html` + `screenshots/chart-*.png`.
Fork: `crates/freya/crates/freya-plotters-backend/`, `examples/feature_plot_3d.rs`,
`freya-components/src/canvas.rs`. `apps/project/query/run_query.rs` (`PageSpec` shape).
