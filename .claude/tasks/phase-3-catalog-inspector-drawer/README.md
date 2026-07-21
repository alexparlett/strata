# Phase 3 — Catalog · inspector · drawer

The surfaces around the workbench: left **sidebar/catalog**, right **column inspector + profiling**,
bottom **drawer** (Problems / Events / History).

## State of play
Greenfield in Freya — the project root mounts only **header + workbench** today; there is no sidebar,
inspector, or drawer. All engine/domain logic exists in `strata-core` (`[core ✓]`): catalog,
`RefreshCatalog`, `Profile`, view-deps, validity, diagnostics, event log, history. This phase is
**UI + wiring**. Do **P3-01** (the layout shell) first.

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P3-01 | Project layout shell (rail·sidebar·workbench·inspector·drawer) | ⬜ | — | — |
| P3-02 | Catalog sidebar (sections, nested columns, filter) | ⬜ | U3 | P3-01 |
| P3-03 | Catalog re-scan | ⬜ | D5 | P3-02 |
| P3-04 | Catalog validity indicators | ⬜ | D11 | P3-02 |
| P3-05 | View dependencies (UI consumer) | ⬜ | D10 | P3-02/04 |
| P3-06 | Catalog context menus | ⬜ | — | P3-02 |
| P3-07 | PART badges · nested JSON · shape detection | ⬜ | D9 | P3-02 |
| P3-08 | Column inspector (facts box) | ⬜ | U9 | P3-01 |
| P3-09 | Column/table profiling (PROFILE zone) | ⬜ | D4 | P3-08 |
| P3-10 | Profile-cost confirm | ⬜ | U15 | P3-09 |
| P3-11 | Drawer scaffold (tabbed bottom panel) | ⬜ | U10 | P3-01 |
| P3-12 | Drawer — Problems tab | ⬜ | U10 | P3-11 |
| P3-13 | Drawer — Events tab | ⬜ | U10 | P3-11 |
| P3-14 | Drawer — History tab | ⬜ | U10 | P3-11 |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.

> The **Connections pane** in the sidebar belongs to `workstream-connections/` (W7), not here.
