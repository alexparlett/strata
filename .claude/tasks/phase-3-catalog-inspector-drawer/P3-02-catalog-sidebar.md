# P3-02 · Catalog sidebar

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** U3 · **Depends on:** P3-01

## Goal
The catalog: collapsible sections, nested columns, and a filter that spans tables/views/queries.

## Current state
Not built. The catalog is **server data** — a freya-query `FetchCatalog` query, invalidated by the
DDL mutations (state-arch §4), not a local mirror.

## Build
1. **Collapsible chevron section headers**: Tables · Views · Saved queries (Freya `Accordion`).
2. **Column rows** indent by depth with an **expand chevron on struct/nested columns** (recursive
   `flatten_cols`, keyed `"{table}::{path}"`); type-coloured column dots. Rows = Freya `SideBarItem`.
3. A **filter row** searching tables *and* views *and* saved queries, + a refresh button (→ P3-03).
4. Selecting a column (incl. nested) drives the inspector (P3-08).

## Acceptance
- [ ] Sections collapse; nested columns expand; filter matches across all three groups; selection updates the inspector.

## Freya / references
- Freya `Accordion`, `SideBarItem`, `Chip`, `use_query(FetchCatalog)`. Design: `Sidebar.dc.html`.
