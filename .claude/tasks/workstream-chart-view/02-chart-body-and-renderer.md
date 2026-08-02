# Chart 02 · Chart body + plotters renderer

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 01

## Goal
The Chart results body: a `canvas` drawing the six chart types through `PlotSkiaBackend`, fed by a
freya-query subscription over `Engine::chart`, themed, with a chart-type picker. Replaces the
placeholder tile in `results/chart.rs`. Spec: `docs/CHART_SPEC.md` §2, §4, §9.

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
  This task derives the `ChartQuery` from schema defaults (spec §6 rules); 03 replaces that with
  the real `ChartConfig`.
- **Renderer**: one module per concern under `results/chart/` — a render fn per type over
  `ChartData` via plotters `ChartBuilder` (bar/line/area/scatter/histogram cartesian; pie via
  plotters' `Pie` element — verified `fill_polygon`-only). `None` series cells: gap on lines,
  zero-height bar. Mesh per spec §9: ~5 gridlines, nice max, abbreviated ticks (`1.2k`/`3.4M`),
  thinned X labels, zero baseline when data spans negatives.
- **Canvas contract**: draw in logical units only (`CanvasContext.size` is logical, pre-scaled).
  Request a repaint when the settled data / config / theme changes — `RenderCallback` never diffs
  unequal, so the `feature_plot_3d` idiom (platform redraw request) applies; if that misbehaves,
  the fix is a fork-level revision key on `CanvasElement`, not an app workaround.
- **Theme**: a `chart` component theme in `theme.rs` (axis / grid / label colours) + the
  **categorical palette as 10 palette slots**; fonts from the theme family through the backend's
  text style — no `"sans"`. Then `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.
- **Type picker**: the control strip's top control (SegmentedToggle or Select per the design
  visuals), writing chart type; the rest of the strip is 03's.

## Acceptance
- [ ] Selecting Chart renders the current result with schema-default encoding; all six types
      render correctly and re-render on type change, data settle, theme change, and resize.
- [ ] Empty temporal buckets show as gaps; the chart reads every colour/font from the theme.

## References
`docs/CHART_SPEC.md` §9. Design visuals: handoff `Results.dc.html` + `screenshots/chart-*.png`.
Fork: `crates/freya/crates/freya-plotters-backend/`, `examples/feature_plot_3d.rs`,
`freya-components/src/canvas.rs`. `apps/project/query/run_query.rs` (`PageSpec` shape).
