# ED-04 · Internal tables, engine half: def shape, CTAS spool, replay

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** E5 · **Depends on:** ED-02 (ED-03 should land before or with this)

## Goal

Internal tables exist: `CREATE TABLE` / CTAS spools to `.strata/tables/<slug>/` as Arrow IPC,
registers through the existing funnel, folds into the store as an ordinary def, and replays on
open — headless host included — with zero new replay code. `docs/STATEMENTS_SPEC.md` §6.1 + §7.

## Current state

- Table creation is hard-wired to one function: `register_external`
  (`crates/strata-core/src/engine/catalog.rs:68`) — the `SourceFormat::Arrow` arm exists.
- The def→spec projection is `table_spec` (`strata-core/src/register.rs:54`); replay is
  `register_pass`/`register_project` (both hosts).
- Verified (spec §2): DF's native CTAS is RAM-whole `MemTable` — unusable; the Arrow sink writes
  LZ4-frame IPC; `ArrowFormat::infer_stats` returns unknown.

## What to build

**Defs (`strata-model/src/catalog.rs`):** `TableDef` gains
`#[serde(default)] origin: TableOrigin { External, Internal }` — a flag, not a new type (single
namespace kept; old `project.json` loads unchanged; a def is one list entry either way).

**Engine (`strata-core`):**
- `TableSpec` gains `internal: bool`; `table_spec` maps it from `origin` (one line);
  `register_external` records internal folded names in an engine-side set — derived state rebuilt
  by every pass, answering only "may a write statement target this" (never a second catalog).
- `Engine::set_data_dir(root)`: the absolute `.strata/tables` root, set at project open by the
  app and the headless host; CTAS refuses politely when unset.
- `engine/ddl.rs::ctas`: refuse constraints/defaults/`TEMPORARY`/duplicate result columns from
  the parsed statement; resolve `IF NOT EXISTS`/`OR REPLACE`/plain-exists against the namespace;
  spool via an internally rendered
  `COPY (<inner query text, sliced verbatim>) TO '<data_dir>/.tmp-<nonce>/' STORED AS ARROW`
  (streaming; the sink's count column is the report's row count); rename tmp → final (atomic);
  zero-row and column-list-only CREATE write one empty IPC file carrying the schema; then
  `register_external` → `TableMeta` → `StoreEffect::TableUpserted`.
- `StrataArrowFormat` wrapping `ArrowFormat`, overriding `infer_stats` to read exact row counts
  from IPC footers (metadata-only), used by the `SourceFormat::Arrow` arm — real
  `TableMeta.rows` for internal (and external Arrow) tables. Null counts deliberately not
  attempted.
- `tidy_strata_dir` sweeps `.strata/tables/.tmp-*`; `ensure_gitignore` adds `tables/`.

**App:** the Configure window shows an `origin: Internal` def read-only ("Internal table. Data is
managed by Strata; drop and re-create to change it") — sources/format/hive controls disabled, no
Save.

## Acceptance

- CTAS over a large result completes without proportional RAM growth (streamed), lands the row
  in the sidebar, persists the def, and the table is queryable in another tab after the epoch
  bump.
- `CREATE TABLE t (a INT)` yields an empty queryable table with the declared schema; a following
  restart replays it (schema from the IPC file, not the def).
- `IF NOT EXISTS` no-ops with a report; plain create over an existing name errors; `OR REPLACE`
  replaces; constraints/`TEMPORARY` refuse tersely.
- Close and reopen the project: the internal table returns through the ordinary pass
  (`register_project` test in `strata-core` covers the headless half). A copy of the project
  without `.strata/tables/` shows an honest `Reg::Failed` row.
- `TableMeta.rows` is exact for Arrow tables (footer-read test with a multi-batch file).

## Verification

`cargo test -p strata-core`; run the app end to end (CTAS → sidebar → restart → still there);
`git status` confirms data files are ignored.
