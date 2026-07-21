# P2-03 · `QueryPage` → grid model (kill the fixture)

**Phase:** 2 — Workbench · **Status:** ⬜ `[core ✓]` · **Depends on:** P2-01/02 · **Unblocks:** P2-08..13

## Goal
The datagrid renders the `QueryPage` returned by `use_query` (typed columns + a page of rows) instead
of the static `fixture()`.

## Current state
`datagrid/model.rs::fixture()` builds static `GridData`; `datagrid/mod.rs` does
`use_hook(|| Rc::new(fixture()))`.

## Build
1. Feed `GridData` (or its replacement) from the settled `QueryPage`: real column names, Arrow types
   (→ the type-colour `Kind`), the page's formatted cells.
2. Remove `fixture()` from the render path (keep only behind a dev flag, or delete).
3. **Paging / sort / filter** read the run's **snapshot** (P2-01): the freya-query key is
   `(snapshot_id, page, sort, filter)` — bump it to fetch (or cache-serve) a page. Not a separate store.
4. Re-check autofit clamp bounds against real column widths.

## Acceptance
- [ ] Grid shows the real result set; type colours from the real Arrow schema.
- [ ] Prev/next bumps the snapshot read's page; a revisited page is cache-served (immutable snapshot).
- [ ] `fixture()` is not on the normal render path.

## Freya / references
- `datagrid/{mod,model,cell,header}.rs` — keep render/selection/resize, swap the data source only.
- `QueryPage` from `apps/project/query/run_query.rs` (P2-01). Core `serialize` formats cell text.
