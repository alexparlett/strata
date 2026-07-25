# Phase 3 — Catalog · inspector · drawer

The surfaces around the workbench: left **sidebar/catalog**, right **column inspector + profiling**,
bottom **drawer** (Problems / Events / History).

## State of play
The layout shell is built (**P3-01** ✅): the project root mounts the full rail · sidebar ·
workbench · inspector · drawer frame with resizable, collapsible panels (open/collapse state + sizes
persist across restart). The **catalog sidebar** now fills the left pane (**P3-02** ✅) — sections,
nested columns, filter, and the column selection that drives the inspector. The inspector and
drawer are still **shells** waiting on their content tasks.

> **The catalog is the `ProjectState` store**, not a query: the project file's defs plus what
> registration learned (`Reg<T>`). There is no `FetchCatalog` capability and there must not be —
> introspecting DataFusion would surface `__snap_*` result snapshots and hide failed rows. See
> `docs/FREYA_STATE_ARCHITECTURE.md` §6 and P3-02's own notes; P3-03 and P3-06 were written against
> the old, wrong premise and have been corrected.

Most engine/domain logic exists in `strata-core` (`[core ✓]`): catalog registration, `Profile`,
view-deps, validity, diagnostics, event log, history. **Not** `Engine::refresh_catalog` — P3-03
writes it (the old `Command::RefreshCatalog` died with P2-01). This phase is **UI + wiring**.

**Shared components that landed with P3-02** and are ready for the rest of the phase (and W7):
`components/sidebar_row.rs` (row shell over Freya's `SideBarItem` — hover/selected + a11y),
`components/badge.rs` (PART / HOTSPOT / dtype pills), `components/type_palette.rs` (the seven
per-`Kind` hues, one shared group).

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P3-01 | Project layout shell (rail·sidebar·workbench·inspector·drawer) | ✅ | — | — |
| P3-02 | Catalog sidebar (sections, nested columns, filter) | ✅ | U3 | P3-01 |
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
