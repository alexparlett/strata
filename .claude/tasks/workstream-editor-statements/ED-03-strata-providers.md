# ED-03 · Strata providers: catalog/schema identity + snapshot hiding

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** — (land before or with ED-04)

## Goal

Strata owns the catalog/schema pair — identity and visibility, never lifecycle
(`docs/STATEMENTS_SPEC.md` §3 + §5). `__snap_*` disappears from every enumeration surface while
staying resolvable by name; `CREATE SCHEMA` becomes impossible by construction;
`SHOW TABLES` starts working on a fresh project (`DESCRIBE` already did — see the corrections).

## As built

`crates/strata-core/src/engine/providers.rs`:

- `StrataCatalogProvider` — `schema_names() == ["public"]`; `schema("public")` only;
  `register_schema` / `deregister_schema` return an exec error.
- `StrataSchemaProvider` — one `RwLock<BTreeMap>` keyed by `fold_ident`; `table_names()` filters
  `__snap_`-prefixed entries; `table`, `table_exist`, `register_table`, `deregister_table` are
  `MemorySchemaProvider`'s behaviour verbatim (duplicate-name error included) over the folded key.
  `BTreeMap` rather than DataFusion's `DashMap`: the only contention here is registration, and
  sorted keys make `SHOW TABLES` deterministic for free.
- Installed in `build_context` via `ctx.register_catalog(CATALOG, …)`, before anything registers —
  it replaces the `MemoryCatalogProvider` the session builder puts under the same name.
- The prefix rule became a **predicate**, `engine::query::is_snapshot_name`, beside `snapshot_name`
  — three consumers now (the router's refusal, the provider's filter, the naming itself), so
  `SNAPSHOT_PREFIX` went back to private and `validate::is_reserved` calls the shared function.
- `datafusion.catalog.information_schema` defaults **on** in two places that have to agree:
  `SessionConfig::with_information_schema(true)` **before** the override loop (so a user's `false`
  still wins, and it is not an owned key), and `default: "true"` in `ENGINE_KEYS` (so a *removed*
  override lands back on what the engine was built with rather than on DataFusion's `false`).

## Corrections to the task as written

Three things the plan asserted that the DataFusion 54 source does not support. All are recorded in
`docs/STATEMENTS_SPEC.md` §4/§5 and `docs/reference/INVARIANTS.md`; none changes a conclusion.

- **`CREATE DATABASE` cannot fail at a provider.** DF's `create_catalog` registers into the
  `CatalogProviderList`, not into a `CatalogProvider`, and `CatalogProviderList::register_catalog`
  returns an `Option` with no way to fail (`datafusion-54.0.0/src/execution/context/mod.rs:1030`).
  A refusing list could only lie ("already exists") or silently no-op — both worse end-states than
  a refusal. So it was **not** built: the router's `Blocked::CreateDatabase` is its only gate, and
  the acceptance line that asked for a provider-level refusal is dropped rather than faked.
  `CREATE SCHEMA` *is* structural, and is tested through `ctx.sql` directly.
- **`register_table` is not last-write-wins.** `MemorySchemaProvider` returns "The table … already
  exists"; the provider keeps that. So a `__snap_` collision costs a *Run* (failing on a name the
  user cannot see) rather than silently displacing the user's table — a worse failure than §4
  described, and the same reason to refuse reserved names.
- **The D5 re-scan does not walk our schema.** It walks the `ProjectState` store
  (`state::hooks::plan_scan`), as the catalog invariant requires. Unaffected, trivially.
- **`DESCRIBE` never needed `information_schema`** (caught by the adversarial review, which raised
  it from three independent lenses). Only `SHOW TABLES` did: it rewrites to
  `SELECT * FROM information_schema.tables` and errors outright when the key is off
  (`datafusion-sql-54.0.0/src/statement.rs:1627`), where `describe_table_to_plan` (`:1638`) goes
  straight to `get_table_source` with no gate. The goal line above is corrected, and the four
  places that stated the wrong reason with it. The default flip is still right and still needed —
  for `SHOW TABLES` and the `information_schema` views.

One behaviour genuinely widened, deliberately: folding on **both** sides makes the single namespace
case-insensitive at the map, so `SELECT * FROM "MyView"` now resolves the view named `MyView` where
DataFusion alone treats a quoted spelling as a different table. That is the "defense in depth for
the single case-insensitive namespace" this task asked for; a fold on the write side alone would
have stranded any entry that arrived unfolded, which is the hazard it claims to prevent. It moved
`quoting_keeps_the_identity_the_unquoted_interpolation_gave_a_name`'s sanity assertion from *which
spellings resolve* (every one, now) to *what identity the entry is stored under* — the stricter
question, and the one the fold-preservation contract is actually about.

## Acceptance — met

- With a live snapshot registered: `SHOW TABLES` lists user tables/views only;
  `information_schema.tables` / `.views` / `.columns` contain no `__snap_*` row and no
  `__strata_ord` column; `ctx.table("__snap_N")` still resolves
  (`a_live_snapshot_resolves_by_name_and_appears_in_no_enumeration`, and the same over a snapshot a
  real Run minted through `register_listing_table` —
  `the_snapshot_a_run_mints_is_hidden_and_still_readable`).
- `CREATE SCHEMA x` fails at the provider when driven directly through `ctx.sql`
  (`a_second_schema_cannot_be_created`). `CREATE DATABASE x` does not — see the corrections above.
- Registration keyed case-insensitively (`registration_is_keyed_case_insensitively`).
- `SHOW TABLES` / `DESCRIBE t` succeed on a fresh project with default config
  (`introspection_works_on_a_fresh_context` — the `SHOW TABLES` half is what the flip bought, the
  `DESCRIBE` half pins that it resolves through the new provider); a user's
  `information_schema = false` still wins (`the_information_schema_default_is_overridable`).

## Verification

`cargo test --workspace --locked` green on macOS with a container runtime (429 in `strata-core`
including the six above, plus every other member and the MinIO integration test).
