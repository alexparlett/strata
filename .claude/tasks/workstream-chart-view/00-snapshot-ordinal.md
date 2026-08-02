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
- **Write** (`engine/query.rs::materialize`): append `__strata_ord` (Int64, 0-based, arrival
  order — the spool is a single ordered stream) to each batch before it is written, **after**
  `QueryOutput::columns` is captured so the user-visible schema never carries it. On a name
  collision with a result column, escalate the prefix (`___strata_ord`, …) until free — the same
  move as `chart.rs`'s `measure_alias` — and record the chosen name in `SnapshotStats`, which
  already has exactly a snapshot's lifetime.
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
`materialize` appends the escalated ordinal to each **written** batch only (page 1, the null
counts and `QueryOutput::columns` all capture the original), and records the name in
`SnapshotStats.ord`. `read_page` sorts by `(user sort?, ordinal)` and `drop_columns` the
ordinal after the window; export's `select_sql` names the result's columns explicitly
(`quote_col` — verbatim double-quote escaping, replacing the old local escape) and orders by
the ordinal, user sort first when set. One subtlety the tests pin: for a query with **no**
`ORDER BY`, "result order" is the order the spool received — the engine's own output order,
frozen. The guarantee is agreement (page 1 = the spooled page, re-reads identical, pages
disjoint), not row `i` at position `i`; a query that wants that writes `ORDER BY`, and the
snapshot then preserves it exactly. Tests: `tests/snapshot_order.rs`, five cases over >10 MB
snapshots.

## Out of scope
Any chart code (01). Any UI change — the grid gets correct pages through the same calls.

## Acceptance
- [ ] Page reads over a 3M-row snapshot are stable and in result order; re-reads agree; page 2
      continues page 1 exactly.
- [ ] User sorts are stable across page windows on tied keys.
- [ ] No export, page batch, or schema ever contains the ordinal; a colliding user column name
      is escalated around, not broken.

## References
`docs/SNAPSHOT_SPEC.md` §9 (design + measurements), §5–§6 (read model).
`engine/query.rs` (`materialize`, `read_page`, `SnapshotStats`), `engine/export.rs`
(`select_sql`).
