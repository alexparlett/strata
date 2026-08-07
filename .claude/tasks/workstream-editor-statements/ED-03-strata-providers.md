# ED-03 · Strata providers: catalog/schema identity + snapshot hiding

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** — (land before or with ED-04)

## Goal

Strata owns the catalog/schema pair — identity and visibility, never lifecycle
(`docs/STATEMENTS_SPEC.md` §3 + §5). `__snap_*` disappears from every enumeration surface while
staying resolvable by name; `CREATE SCHEMA`/`DATABASE` become impossible by construction;
`SHOW TABLES` / `DESCRIBE` start working on a fresh project.

## Current state

- `build_context` (`crates/strata-core/src/engine/mod.rs:1295`) uses stock DataFusion memory
  providers with the default catalog/schema renamed `strata`/`public` (`mod.rs:1344`).
- `datafusion.catalog.information_schema` defaults off (`engine/config.rs:57`), so the
  policy-allowed `SHOW TABLES` fails at plan time — and turning the key on today would list every
  `__snap_N` spool table and expose `__strata_ord` through `information_schema.columns`.
- Verified (spec §2): all information_schema views and SHOW enumerate via
  `SchemaProvider::table_names()`; a separate snapshot schema hides nothing; `table_type` has a
  default impl through `table()`.

## What to build

`crates/strata-core/src/engine/providers.rs`:

- `StrataCatalogProvider`: `schema_names() == ["public"]`; `schema("public")` only;
  `register_schema`/`deregister_schema` return an exec error.
- `StrataSchemaProvider`: one map keyed by **folded** name (`fold_ident` — defense in depth for
  the single case-insensitive namespace); `table_names()` filters `__snap_`-prefixed entries;
  `table()`, `table_exist`, `register_table`, `deregister_table` delegate verbatim (so every
  reader, `find_and_deregister`, and snapshot retirement work with zero call-site changes).
- Install in `build_context` via `ctx.register_catalog(CATALOG, …)` before anything registers.
- Move the `__snap_` prefix constant next to `snapshot_name` in `engine/query.rs` and import it
  here — the hiding rule and the naming rule share one definition.
- Flip the `datafusion.catalog.information_schema` default to `true` in `ENGINE_KEYS`
  (still user-overridable; not an owned key).

## Acceptance

- With a live snapshot registered: `SHOW TABLES` lists user tables/views only;
  `information_schema.tables` / `.views` / `.columns` contain no `__snap_*` row and no
  `__strata_ord` column; `ctx.table("__snap_N")` still resolves (paging/chart/export tests
  unchanged).
- `CREATE SCHEMA x` / `CREATE DATABASE x` fail at the provider even when driven directly through
  `ctx.sql` in a test (policy refusal remains the first line).
- Registration keyed case-insensitively: registering `Foo` then resolving `foo` works, matching
  today's `fold_ident` discipline.
- `SHOW TABLES` / `DESCRIBE t` succeed on a fresh project with default config.
- D5's re-scan (walks our schema, skips `__snap_*`) behaves identically.

## Verification

`cargo test -p strata-core`; run the app: `SHOW TABLES` in the editor after a Run shows no
snapshot rows.
