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
warning triangle with the reason (**P3-04** ✅). The **drawer** header gained its expand / restore
toggle (**P3-11** ✅ — see the note below on how little of that task was left). The **drop
confirm** is built (**P3-05** ✅), naming the views a drop leaves invalid and performing the drop,
and **P3-06** ✅ gave every row its
menu (right-click *and* the canvas's ⋮): open in a tab, edit a view's SQL, rename a saved query,
refresh one table, and the Drop items that open that confirm. The **column inspector** now fills
the right pane (**P3-08** ✅) — the selected column's title + source-format badge, a nested
column's shape, and the STATISTICS zone: a dynamic facts box over what the source actually
reported, plus the completeness bar. **Profiling** fills that zone's scan half (**P3-09** ✅) — an
on-demand full scan per catalog entry, offered by the zone's card and by both row menus, behind a
cost confirm on a first scan (**P3-10** ✅). The bottom drawer has its first body: **P3-12** ✅
fills it with **every** open tab's live diagnostics, grouped by tab and pressable to switch — and
rebuilt the producer behind it (one driver, a per-tab stamp, the catalog as a gate) plus the three
pieces P3-11 deferred to its first consumer. It also gave the drawer its own component theme and
the rail its error badge. Events and History are still empty frames.

> **Only real facts** (P3-08 · DEV_TASKS U9). Every number in the inspector was *read*, never
> derived from the rows on screen — the Dioxus panel once computed them off the current page of the
> current tab's query and presented them as column facts. So the facts box is a **dynamic list** (a
> Parquet column shows four rows, a CSV column shows one, neither shows a blank), inexact footer
> values render `~value`, and the completeness bar needs a real *exact* null count and a real row
> count or it does not appear. The percentage never rounds into a claim it can't make: with nulls
> present it reads `>99.9%`, never `100%`. The null count is the bar and **only** the bar.
>
> P3-09's **scan** lands in that same list, matched on `StatKey`, so a fact still cannot appear
> twice — and the rule extends: the scan's *row count* comes with its null count (the bar divides
> one by the other, so mixing a counted numerator with a reported denominator is one ratio from two
> reads), and a nested field takes neither, because the profile is keyed by top-level column name.
> The canvas's **distribution bars are deliberately not built**: the profile has no distribution
> data, and an honest histogram needs a second full pass. See P3-09.

> **P3-11 was largely already built** (and is now ✅). P3-01 had delivered the drawer container,
> its resizable/collapsible height, the persisted active-tab state — and the **tab switcher**,
> which in the design is the **rail's bottom group**, not a pill row in the drawer header (the
> canvas computes `drawerHistTabStyle` / `drawerProbTabStyle` / `drawerEvtTabStyle` and
> `onDrawerTab` and never renders them). Don't add a second switcher: it would be a second writer
> for `Layout::drawer`. The rest of P3-11's brief — the header's count label, Clear, and the
> "common list frame" — has no consumer until the tab tasks, so it moved to them: **P3-12** owns
> the count, the Problems-hides-Clear rule, and the frame (a scroll container + a centred empty
> state, which is all the three tabs genuinely share); **P3-13** and **P3-14** own Clear's action.
> What shipped is the header's **expand / restore** toggle — see its file, including the fork note
> that `ResizablePanel` reads `initial_size` only at mount, so programmatic sizing goes through a
> `ResizableContext` controller.

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

> **Diagnostics are a reconciliation, and Problems shows every tab** (P3-12). The first build
> scoped the view to the active tab; the real limit was the *producer* — `use_validation` lived in
> `EditorTab`, which mounts only for the tab on screen. Now each tab carries a **stamp** (buffer
> revision + catalog epoch) of what its diagnostics describe, `SessionState::stale_tabs` is the
> whole work list, and **one** driver (`state/diagnostics.rs`, a hook in the window root) drains
> it. No entry point needs enumerating: restored, reopened, opened-from-a-view, duplicated,
> edited, cancelled-mid-pass are all "the stamp does not match". The catalog is a **gate** as well
> as an input (`CatalogState { Scanning, Settled(epoch) }`), so nothing validates mid-scan and
> fixing a source path clears the rows without opening the tab. Run failures are deliberately
> **not** in Problems — they belong to a run, and the results pane renders them in full.
>
> P3-11's three handovers landed with it: the header count (now `error_count()`, shared with the
> new rail badge so they cannot disagree), the Clear show/hide rule (parked on Events/History
> until they have a log to clear), and the shared frame (`drawer/frame.rs`).
>
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
| P3-08 | Column inspector (facts box) | ✅ | U9 | P3-01 |
| P3-09 | Column/table profiling (PROFILE zone) | ✅ | D4 | P3-08 |
| P3-10 | Profile-cost confirm | ✅ | U15 | P3-09 |
| P3-11 | Drawer scaffold (tabbed bottom panel) | ✅ | U10 | P3-01 |
| P3-12 | Drawer — Problems tab + the diagnostics architecture | ✅ | U10 | P3-11/P2-18/P2-01 |
| P3-13 | Drawer — Events tab | ⬜ | U10 | P3-12 |
| P3-14 | Drawer — History tab | ⬜ | U10 | P3-12 |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.

> The **Connections pane** in the sidebar belongs to `workstream-connections/` (W7), not here.
