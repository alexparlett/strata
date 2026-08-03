# Chart 03 · Encoder strip + `ChartConfig` state

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 02

## Goal
The left control strip (X / Ys / Series / Sort) and the persisted per-tab `ChartConfig` that
drives the chart. Spec: `docs/CHART_SPEC.md` §3, §4, §6.

## Current state
02 charts off schema-derived defaults computed inline (`results/chart/mod.rs`: `Roles::of` over
each column's `ChartRole`, then `encode`). No config state exists — the **mark** is a
`use_state` on the body, written by the strip's tile grid, and it resets on a re-run because
this task is what persists it.

What is already built and should be extended rather than replaced: the strip itself
(`results/chart/strip.rs` — 232px, its own scroll, `CHART TYPE` at the top and room under it),
the `chart` component theme (its `label_color` is a section eyebrow's), and the **one**
`ChartQuery` construction site (`encode`), which is where the config replaces the derivation.

## Build
- **State**: `ChartConfig { mark, x, ys, series, sort }` (serde, `strata-model`) on `QueryTab`
  under a new **`Chan::Chart(tab)`** channel — encoder edits must never wake the editor or grid
  (`derive_channel`: goes to `Persist`, like `View`). Persist via `TabSnapshot` alongside `view`.
  Defaults merge **under** user-set keys per spec §6; when a new result's columns no longer match
  the stored config, re-derive — a stale column name must never reach `ChartQuery`. The
  `ChartQuery` construction from config + schema stays in the one site 02 built (`encode`).
  Moving the mark there means the tile grid writes `config.mark` instead of its `use_state`. **`sort` is a
  view transform** (`ResultOrder` | `ByX` | `ByYDesc`): applied client-side to the settled
  `ChartData::Table`, never part of `ChartQuery` — flipping it repaints without a re-query. Any
  float comparison in that reorder is total (`total_cmp`, NaN last): the withdrawn pipeline's
  `sort_by` panic on a NaN weight is the standing lesson.
- **Strip** (~232 logical px, own scroll, per the design visuals): standard components only —
  `Select`s for X and Series filtered by column role (measure / temporal / dimension, nested
  excluded; numeric columns are valid on X — spec §3), a **multi-select for Ys** (each selected
  measure is its own series, named by column), and the **Sort toggle**. No aggregate toggle, no
  fn menu, no bucket control — aggregation is SQL's (spec §1.2), reached through 04's scaffold.
  Options constrained per mark (spec §4 table): one Y and no Series on pie, no Series on
  scatter/histogram. Typography roles for labels; no hardcoded fonts or colours.
- Invalid encodings are **prevented by construction** (a control never offers a column the type
  can't take); the residual cases (nothing valid to offer) are 04's guardrail overlays, not
  inline errors here.

## Acceptance
- [ ] Every channel is assignable through the strip; changes re-chart via the subscription;
      config survives restart with the tab; a schema change re-derives defaults cleanly.
- [ ] Editing an encoder redraws the chart without waking any other results/editor channel.

## References
`docs/CHART_SPEC.md` §6. `state/session.rs` (`QueryTab`, `set_view` shape),
`state/channel.rs` (`derive_channel`), `strata-model/session.rs` (`TabSnapshot`).
Design visuals: handoff `Results.dc.html` control strip.
