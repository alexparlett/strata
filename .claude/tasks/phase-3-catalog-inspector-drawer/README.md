# Phase 3 — Catalog · inspector · drawer

The surfaces around the workbench: left **sidebar/catalog**, right **column inspector + profiling**,
bottom **drawer** (Problems / Events / History).

## State of play
The layout shell is built (**P3-01** ✅): the project root mounts the full rail · sidebar ·
workbench · inspector · drawer frame with resizable, collapsible panels (open/collapse state + sizes
persist across restart). The **header** is built too (**P3-15** ✅ — the app-shell surface that had
no task of its own): it is the window's title bar (drag · double-press to fill · traffic-light
gutter) and carries the brand, the project switcher, and the ⌘K / ⌘, cluster; the palette, settings
and open-project actions are placeholders waiting on P6-01 / P4-03 / P4-13. The **catalog sidebar**
now fills the left pane (**P3-02** ✅) — sections, nested columns, filter, and the column selection
that drives the inspector — its ↻ re-scans the catalog (**P3-03** ✅), and broken rows carry a
warning triangle with the reason (**P3-04** ✅). The **drop confirm** is built (**P3-05** ✅),
naming the views a drop leaves invalid and performing the drop, and **P3-06** ✅ gave every row its
menu (right-click *and* the canvas's ⋮): open in a tab, edit a view's SQL, rename a saved query,
refresh one table, and the Drop items that open that confirm. The inspector and drawer are still
**shells** waiting on their content tasks.

> **P3-07 was rescoped** (and is now ✅) to *registration failure messages*. Its original three items were PART
> chips and the nested column tree — both delivered by P3-02, PART exercised end to end by the
> `sample/` project's Hive-partitioned `events` — plus a "parseable-JSON echo" that had no referent
> anywhere, and a pre-flight JSON-shape / schema-consistency report that is **dropped**: the
> register already fails on any shape DataFusion can't read, and P3-04's triangle already carries
> the reason. What survives is that those reasons are badly worded (and one interpolates the parsed
> file into the error string). See its file.

> **A row's Refresh re-creates the views over it** (P3-06), and re-creates them in **dependency
> order** — a view inlines the plan of any view it reads at `CREATE OR REPLACE` time, so an outer
> view rebuilt before its inner one inlines the stale plan. `ProjectState::refresh_order` is shared
> with the whole-catalog ↻, which had the same latent ordering bug (it worked only where the def
> names happened to sort right).

> **Validity is derived, never stored** (P3-04): `ProjectState::{table_problem, view_problem}`
> answer off the live rows on every read — a table's from the answer already on it, a view's from
> whether the base tables its `deps` name are still there and still working. So a re-scan, a drop,
> or a fixed path needs no invalidation pass; the flag simply follows the catalog. **P3-05** ✅ runs
> the same `deps` the other way (`dependent_views`: "which views would this drop leave invalid"),
> and hangs the **drop confirm** off it — the dialog, its consequence line, and the drop itself.
> P3-06 supplies its trigger and must not write a second drop path.

> **The catalog is the `ProjectState` store**, not a query: the project file's defs plus what
> registration learned (`Reg<T>`). There is no `FetchCatalog` capability and there must not be —
> introspecting DataFusion would surface `__snap_*` result snapshots and hide failed rows. See
> `docs/FREYA_STATE_ARCHITECTURE.md` §6 and P3-02's own notes; P3-03 and P3-06 were written against
> the old, wrong premise and have been corrected.

Most engine/domain logic exists in `strata-core` (`[core ✓]`): catalog registration, `Profile`,
view-deps, validity, diagnostics, event log, history. `Engine::refresh_catalog` was never written
and isn't needed: P3-03 settled that **a re-scan is a re-registration from the defs**
(`Engine::register` already re-infers, and only the def-driven path can retry a table whose
registration failed) — see its task file. This phase is **UI + wiring**.

**Shared components that landed with P3-02** and are ready for the rest of the phase (and W7):
`components/sidebar_row.rs` (row shell over Freya's `SideBarItem` — hover/selected + a11y),
`components/badge.rs` (PART / HOTSPOT / dtype pills), `components/type_palette.rs` (the seven
per-`Kind` hues, one shared group).

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| P3-01 | Project layout shell (rail·sidebar·workbench·inspector·drawer) | ✅ | — | — |
| P3-15 | Header bar (title bar · brand · project switcher · cluster) | ✅ | U2 | — |
| P3-02 | Catalog sidebar (sections, nested columns, filter) | ✅ | U3 | P3-01 |
| P3-03 | Catalog re-scan | ✅ | D5 | P3-02 |
| P3-04 | Catalog validity indicators | ✅ | D11 | P3-02 |
| P3-05 | View dependencies (UI consumer) + drop confirm | ✅ | D10 | P3-02/04 |
| P3-06 | Catalog context menus | ✅ | — | P3-02/05 |
| P3-07 | Registration failure messages | ✅ | D9 | P3-04 |
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
