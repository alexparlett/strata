# P2-03 · `QueryPage` → grid model (kill the fixture)

**Phase:** 2 — Workbench · **Status:** ✅ **built** · **Depends on:** P2-01/02 · **Unblocks:** P2-08..13

## Goal
The datagrid renders the `QueryPage` returned by `use_query` (typed columns + a page of rows) instead
of the static `fixture()`.

## Current state
Built. `fixture()` and the local `Kind` are deleted: `GridData` holds the model's real
`Vec<ColumnInfo>` + `Vec<Vec<Cell>>` (nulls render dimmed), `DataGrid::new(&QueryOutput, page)`
renders page 1 from the Run's own output, and later pages are `use_query(FetchSnapshotPage)` reads
keyed by `PageSpec` with `stale_time(MAX)` (`enable(false)` on page 1). Column widths live at the
grid level so a page flip keeps resizes; the gutter numbers rows absolutely across pages. A minimal
working pager (prev/next + row range) sits in the status bar's right slot — P2-08 dresses it.
`ResultsBody` is keyed by the press's `RunId`, so a new Run resets page + widths.

## Build
1. Feed `GridData` (or its replacement) from the settled `QueryPage`: real column names, Arrow types
   (→ the type-colour `Kind`), the page's formatted cells.
2. Remove `fixture()` from the render path (keep only behind a dev flag, or delete).
3. **Paging / sort / filter** read the run's **snapshot** (P2-01): the freya-query key is
   `(snapshot_id, page, sort, filter)` — bump it to fetch (or cache-serve) a page. Not a separate store.
4. Re-check autofit clamp bounds against real column widths.

## Acceptance
- [x] Grid shows the real result set; type colours from the real Arrow schema.
- [x] Prev/next bumps the snapshot read's page; a revisited page is cache-served (immutable snapshot).
- [x] `fixture()` is not on the normal render path (deleted).

## Freya / references
- `datagrid/{mod,model,cell,header}.rs` — keep render/selection/resize, swap the data source only.
- `QueryPage` from `apps/project/query/run_query.rs` (P2-01). Core `serialize` formats cell text.
