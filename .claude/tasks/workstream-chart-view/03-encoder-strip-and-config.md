# Chart 03 · Encoder strip + `ChartConfig` state

**Workstream:** Chart (Rz2) · **Status:** ⬜ · **Depends on:** 02

## Goal
The left control strip (X / Y / Series / Aggregate / stride) and the persisted per-tab
`ChartConfig` that drives the chart. Spec: `docs/CHART_SPEC.md` §3, §4, §6.

## Current state
02 charts off schema-derived defaults computed inline. No config state exists.

## Build
- **State**: `ChartConfig` (serde, `strata-model`) on `QueryTab` under a new **`Chan::Chart(tab)`**
  channel — encoder edits must never wake the editor or grid (`derive_channel`: goes to `Persist`,
  like `View`). Persist via `TabSnapshot` alongside `view`. Defaults merge **under** user-set keys
  per spec §6; when a new result's columns no longer match the stored config, re-derive — a stale
  column name must never reach `ChartQuery`. The `ChartQuery` construction from config + schema
  stays in the one site 02 built.
- **Strip** (~232 logical px, own scroll, per the design visuals): standard components only —
  `Select`s for X / Y / Series filtered by column role (measure / temporal / dimension, nested
  excluded; spec §3), the Aggregate toggle + fn `Select`, and the **stride control shown only when
  X is temporal** (auto value visible, overridable). Options constrained per chart type (spec §4
  table): e.g. no Series on pie/scatter/histogram, Y hidden for histogram's X. Typography roles
  for labels; no hardcoded fonts or colours.
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
