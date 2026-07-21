# Chart 01 · Chart body + type selection

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** P2-07 (switcher), P2-03 (results model)

## Goal
The Chart results body with the chart-type picker, switched into by the Table/Chart segment.

## Current state
Not built. P2-07 provides the segment; this is the body it swaps to.

## Build (to `CHART_SPEC.md`)
- A chart canvas rendering the **6 chart types** (per the spec) over the current result.
- A type picker; the chart re-renders on type/encoding change.
- Rendering: Freya `plot()` (Plotters) or a canvas — pick per the spec and note it.

## Acceptance
- [ ] Selecting Chart shows a chart of the current result; switching type re-renders.

## Freya / references
- `docs/CHART_SPEC.md`. Design: `Results.dc.html` chart view. Freya `plot()` (skill Plotting) or canvas.
  Depends on P2-07 + the results model (P2-03).
