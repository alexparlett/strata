# P3-02 · Catalog sidebar

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U3 · **Depends on:** P3-01

## Goal
The catalog: collapsible sections, nested columns, and a filter that spans tables/views/queries.

## Current state

**The data already exists — this task is render-only.** The Project store
(`apps/project/state/project.rs`) *is* the catalog, and it was built for exactly this:

```rust
ProjectState { name, root, tables: Vec<TableRow>, views: Vec<ViewRow>, saved_queries: Vec<SavedQuery> }
TableRow { def: TableDef, reg: Reg<TableMeta> }     // meta.columns, meta.rows
ViewRow  { def: ViewDef,  reg: Reg<ViewInfo> }      // info.columns, deps, view_deps
enum Reg<T> { Loading, Ready(T), Failed(String) }
enum ProjChan { Meta, Tables, Views, Queries }      // one channel per section
```

`use_init_project` (`state/hooks.rs`) already loads `.strata/project.json` and spawns
`register_defs`, landing each row `Loading → Ready(meta) | Failed(err)` by name on
`ProjChan::Tables` / `ProjChan::Views`. The sidebar shell (`views/sidebar/mod.rs`) renders the
header + collapse and leaves an empty `rect().expanded()` body for this task.

Several store members are parked `#[allow(dead_code)]` **"Feature reservoir: … (Phase 3)"** and
are this task's consumers: `TableRow::meta_label()`, `Reg::error()`, `ProjChan::Queries`.

### The catalog is *definitions*, not a mirror of DataFusion

There is **no `FetchCatalog` capability**, and there must not be one. (It appears in
`FREYA_STATE_ARCHITECTURE.md` §6/§9/§11 and in P3-03/P3-06 as prose only; nothing implements it.)
The catalog is the project file's defs plus what registration learned — never an introspection
query. `strata-dioxus/src/ui/sidebar.rs:429` states the rule and the Freya store encodes it:

> *"The catalog is definitions, not a mirror of DataFusion — a row can exist yet be broken."*

Asking the engine would be wrong four ways, each verified:

1. **Result snapshots would leak in.** `engine/query.rs:78` registers every materialized result
   as a real table `__snap_{id}` in the same `SessionContext` (`retire_snapshot` deregisters it).
   Introspection lists them; the catalog must not.
2. **Broken rows would vanish.** A `Reg::Failed` row is a def with nothing behind it (missing
   file, bad path, SQL that wouldn't plan) — DataFusion has never heard of it. Those rows are
   precisely what the validity indicator (P3-04) renders.
3. **`information_schema` defaults to `false`** and is a user-facing Settings key
   (`engine/config.rs:56`) — an introspection catalog would be empty out of the box and would
   change shape when the user toggles an unrelated engine option.
4. **Saved queries aren't a DataFusion concept at all.**

There is also no second source of truth to reconcile: `ProjectState` is already the ⌘S / save-as-view
**save-target** store (`workbench/editor/actions.rs`), so a freya-query-cached copy of the same
catalog would be two stores disagreeing about one thing.

## Build

Fill the sidebar body. Read the store per section — `use_radio::<ProjectState, ProjChan>(ProjChan::Tables)`
etc. — so a table registration landing doesn't wake the views or queries sections.

1. **Collapsible chevron section headers**: `TABLES · {n}` · `VIEWS · {n}` · `QUERIES · {n}`
   (Freya `Accordion`). Section open/closed is sidebar-local UI state, not store state.
2. **Row per def**, expandable to its columns (`SideBarItem`):
   - **Table** — table icon, name, `meta_label()` ("6 cols · 2 partitions").
   - **View** — eye icon in the accent colour, name.
   - **Saved query** — brackets icon; addressed by `id`, not name. Empty section renders
     "No saved queries yet".
3. **`Reg` tri-state on every row.** `Ready` → columns expand. `Loading` → the row renders with a
   quiet placeholder (rows genuinely start `Loading` and flip async on window open — this is the
   first paint, not an edge case). `Failed` → the row still renders; its badge is P3-04, so leave
   `Reg::error()` unconsumed here rather than half-building it.
4. **Column rows** indent by depth, with an **expand chevron on struct/list/map columns**
   (recursive `flatten_cols` over `ColumnInfo::children`, expansion keyed `"{owner}::{path.join(\".\")}"` —
   display-only; identity is `ColRef { kind, owner, path: Vec<String> }`, because a column name may
   contain dots). Type-coloured square dot per `Kind` — the theme tokens exist
   (`type_str_color` … `type_map_color`); follow the `kind_color` precedent in
   `results/record_view.rs:62`. Partition columns (top level only) get a `PART` chip.
5. **Filter row** + refresh button (refresh is inert here → P3-03). The filter matches **def names
   across all three sections** (case-insensitive `contains`), each section filtered independently —
   it does not search column names.
6. **Selecting a column** (including a nested one) sets the inspector's `ColRef` and opens the
   inspector (P3-08). Compare the whole `path` for the selected highlight — by name alone, selecting
   `city` lights up every `city` at any depth.

## Acceptance
- [ ] Sections collapse; nested columns expand; filter matches across all three groups; selection updates the inspector.
- [ ] Rows appear immediately on open and flip from `Loading` to their column tree as registration lands, without a full-sidebar re-render per row.

## Freya / references
- Freya `Accordion`, `SideBarItem`, `Chip` (all present in the fork), `ScrollView`.
- Reference implementation: `strata-dioxus/src/ui/sidebar.rs` (`flatten_cols`, `col_rows`, `ColRow`).
- Design: `Sidebar.dc.html`.
