# ED-10 · Typed CREATE EXTERNAL TABLE onto the Table Config funnel

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** ED-02

## Goal

`CREATE EXTERNAL TABLE` typed in the editor registers an ordinary external table through the
funnel Table Config already uses — the parsed statement becomes a `TableDef`, and Table Config
and typed DDL are two gestures into one registration path, exactly as ⌘S and typed `CREATE VIEW`
are for views. `docs/STATEMENTS_SPEC.md` §6.7.

## Current state

- Table registration is one function: `register_external`
  (`crates/strata-core/src/engine/catalog.rs:68`), driven from defs via `table_spec`
  (`register.rs:54`). The Configure window builds defs (`apps/configure/model.rs::def(root)` —
  source relativization, format options, hive partitions) and never registers directly.
- The statement is refused today (`Blocked::CreateExternalTable` → "Register tables in Table
  Config"); that variant and message stay as the **agent** path's refusal.
- DF's native `CREATE EXTERNAL TABLE` path (`TableProviderFactory`) must stay unused — it would
  register behind the store's back, and the def, not the engine registration, is the durable
  artifact.

## What to build

`engine/ddl.rs::create_external` — map the parsed `CreateExternalTable` statement onto a
`TableDef { origin: External }`, then register and fold like CTAS:

- `STORED AS` → `SourceFormat`: `PARQUET`/`CSV`/`JSON`/`ARROW`; anything else refused **by name**
  (the Avro-fallthrough rule — a format with no reader must fail, never fall through, P4-11).
- `LOCATION` → one source; relativize when under the project root (the same rule Configure's
  `def(root)` applies — share it, don't restate it).
- `OPTIONS(…)` → the matching `CsvRead`/`JsonRead` fields (`format.has_header`,
  `format.delimiter`, quote, escape, comment, compression, newlines-in-values, infer rows). Any
  key with no def field is refused **by name** — a silently dropped option is a def that lies
  about how the table reads.
- `PARTITIONED BY` → `partition_cols`. A column list is accepted only where every listed column
  is a partition column (declared types checked against the supported partition types —
  `Utf8`/`Int32`/`Int64`/`Date32`); data columns refuse: "Schemas are inferred. Remove the
  column list."
- Also refused: constraints, `ORDER BY` clauses, `UNBOUNDED`, `TEMPORARY`, a reserved `__snap_`
  name (router + `register_external` backstop). `IF NOT EXISTS` honored against the store's
  namespace; a plain create over an existing name errors with the store's wording.
- Outcome: `register_external` from the built def → `TableMeta` →
  `StoreEffect::TableUpserted { def, meta }` — the identical settle (store fold on
  `ProjChan::Tables`, `persisted_defs`, `catalog_settled`, history + event log) as CTAS's.

## Acceptance

- A CSV `CREATE EXTERNAL TABLE` with header/delimiter/compression options lands a def equal to
  what Configure would build for the same choices (def-equality asserted), the row `Reg::Ready`
  in the sidebar, persisted to `project.json`, queryable after the epoch bump; restart replays it
  through the ordinary pass.
- `STORED AS AVRO` refused by name; an unknown `OPTIONS` key refused by name; a data-column list
  refused; a partition-only column list carries its declared types into the def.
- `LOCATION` under the project root stores relative, outside stores absolute (matches Configure).
- `IF NOT EXISTS` over an existing name no-ops with a report; plain create over an existing name
  errors.
- The agent surface still refuses the statement with today's message.

## Verification

`cargo test -p strata-core`; run the app: type one against a fixture CSV, see it land in TABLES,
open Configure on it (ordinary editable external def), restart, still registered.
