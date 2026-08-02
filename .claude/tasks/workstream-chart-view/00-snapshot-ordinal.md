# Chart 00 · Snapshot ordinal + ordered reads `[core]`

**Workstream:** Chart (Rz2) · **Status:** ✅ · **Depends on:** nothing · **Really:** a P2-01
correctness fix that the chart's order guarantee rides on — ship it even if the chart never lands.

## Goal
Make result order a real, queryable property of every snapshot: the `__strata_ord` column, ordered
reads, and the projection discipline. Spec: `docs/SNAPSHOT_SPEC.md` §9 (includes the measurements).

## The bug this fixes (measured, stock config)
A bare `LIMIT/OFFSET` read of a snapshot has no order once the file passes
`repartition_file_min_size` (10 MB): at 3M rows the **same page re-read returns different rows**
(page 1 started at row 1 843 201 on one read, 101 on the next); a 200k-row snapshot with a text
column pages stably but starting at row 57 345, so `fetch_page`'s pages disagree with the spooled
page 1 — duplicated and missing rows as the user pages — and the freya-query page cache freezes
whichever answer a read got. Sorted reads have the same hazard on ties.

## Build
- **Write** (`engine/query.rs::materialize`): the spool query carries `row_number() OVER ()`
  (a UInt64, 1-based — nothing reads its values, only their order) aliased `__strata_ord`,
  added **after** `QueryOutput::columns` is captured so the user-visible schema never carries
  it. On a name collision with a result column, escalate the prefix (`___strata_ord`, …) until
  free and record the chosen name in `SnapshotStats.ord: Option<String>`, which already has
  exactly a snapshot's lifetime. `None` — an `EXPLAIN`, or a duplicate-named result — spools
  without one and reads unordered, as at base.
- **Reads** (`engine/query.rs::read_page`): no user sort → `ORDER BY <ord>`; user sort →
  `ORDER BY <user col> <dir>, <ord>` (the tie-break makes sorts stable across page windows).
  Project the ordinal away before returning — cells, batch, and schema.
- **Export** (`engine/export.rs::select_sql`): select the result's columns explicitly (quoted per
  the existing `quote_ident` path), never `SELECT *` — a `COPY` must not write bookkeeping into
  the user's file. Partitioned exports included.
- **Audit the other readers**: `value_tree` / inspector reads, agent `read_page` (AA), anything
  else that does `ctx.table(__snap_…)` — each either orders by the ordinal or provably doesn't
  care, and none may leak the column.
- **Tests**: the paging probe becomes a pinned regression — a >10 MB snapshot (3M rows with a
  text column) where page reads are asserted stable AND file-ordered, twice each; a sorted read
  with duplicate keys stable across a page boundary; export output asserted free of the ordinal
  (flat and partitioned); a result already containing a `__strata_ord` column round-trips with
  the escalated name.

## As built
The ordinal rides the spool **query**: `materialize` adds `row_number() OVER ()` aliased to the
escalated name after the result schema is captured, and reads everything user-facing (page 1,
null counts) through a projection that drops the window's column. Measured before adopting: the
window numbers the exact stream the writer consumes on the racy over-threshold shape, and a
user's `ORDER BY` survives beneath it — the first implementation stitched the array into each
batch by hand, and was replaced when review asked why the order wasn't simply part of the query
(`SNAPSHOT_SPEC.md` §9, "considered and replaced"). The name rides in `SnapshotStats.ord`. `read_page` sorts by `(user sort?, ordinal)` and `drop_columns` the
ordinal after the window; export's `select_sql` names the result's columns explicitly
(`quote_col` — verbatim double-quote escaping, replacing the old local escape) and orders by
the ordinal, user sort first when set. One subtlety the tests pin: for a query with **no**
`ORDER BY`, "result order" is the order the spool received — the engine's own output order,
frozen. The guarantee is agreement (page 1 = the spooled page, re-reads identical, pages
disjoint), not row `i` at position `i`; a query that wants that writes `ORDER BY`, and the
snapshot then preserves it exactly.

Post-review hardening (found by the branch's own review passes, all pinned in
`tests/snapshot_order.rs` — eleven cases, the ordering/stability ones over >10 MB snapshots):
typed `EXPLAIN`/`EXPLAIN ANALYZE` and duplicate-named results spool **ordinal-less** rather
than failing (the window cannot wrap a root-only plan; a name-keyed read mis-maps a duplicate
onto the ordinal's slot); the registration **declares** the file's sort order, so ordered reads
plan as streams (deep page: 543 ms undeclared TopK vs 97 ms declared, measured at 3M rows) and
exports stream into their `COPY`; a user's own partitioned window survives beneath the global
ordinal; and a partitioned export is asserted ordinal-free, not only a flat one.

## Out of scope
Any chart code (01). Any UI change — the grid gets correct pages through the same calls.

## Acceptance
- [x] Page reads over a 3M-row snapshot are stable and in result order; re-reads agree; page 2
      continues page 1 exactly.
- [x] User sorts are stable across page windows on tied keys.
- [x] No export, page batch, or schema ever contains the ordinal; a colliding user column name
      is escalated around, not broken.

## References
`docs/SNAPSHOT_SPEC.md` §9 (design + measurements), §5–§6 (read model).
`engine/query.rs` (`materialize`, `read_page`, `SnapshotStats`), `engine/export.rs`
(`select_sql`).
