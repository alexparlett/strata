# DB-02 · The Postgres arm: model, secrets, pool, catalog provider, registration

**Workstream:** Database connections · **Status:** ⬜ · **Depends on:** DB-01

## Goal

The whole mechanism, window-free: a `Provider::Postgres` arm on `ConnectionDef` (the first
whose credentials live in the OS keystore, under a ref *derived* from the connection's
identity), an `engine::db` module that builds the connection pool (the probe),
wraps it in our own lazily-listing catalog/schema provider built through
`PostgresTableFactory`, and registers/deregisters it as a DataFusion catalog — dispatched from
`Engine::connect`/`disconnect` so `register_pass` phase 1 and the whole-catalog ↻ need no
change. Proven headless against a real Postgres in testcontainers. DB-04 puts the editor on
this; nothing here paints.

## Current state (verified 2026-08-13)

- `Provider` is a closed 3-arm enum (`strata-model/src/connection.rs:122-131`), tagged serde,
  "the provider *is* its own settings". `url()` (82), `check_address` (156), `id()` (135),
  `ProviderId::ALL` (196, **pinned at three by test**), `label()` (199). The module doc
  (lines 1-14) and `engine/store.rs`'s (1-14) both declare no arm takes a secret — **rewrite
  both deliberately** (see README decision), plus `CONNECTIONS_SPEC.md`'s two opening rules,
  AGENTS.md §2's connections lines and their INVARIANTS.md entries.
- `engine::store::connect` = `prepare` → `reachable` → `settle` (`store.rs:68,90,120,207`) —
  object-store-shaped end to end (`build → Arc<dyn ObjectStore>`,
  `ctx.register_object_store`). The Postgres arm must **not** thread through it; it gets a
  sibling module. `Engine::connect` (engine/mod.rs:1417) notes the URL on `Connections`
  *whatever the outcome* (membership, not connectivity — keep this) and spawns
  `store::connect` on the engine runtime; `Engine::disconnect` (1437) is sync. Both grow a
  `match` on the provider.
- The engine runtime is 2-worker multi-thread with `enable_all()` (mod.rs:437-442) — the IO
  driver bb8/tokio-postgres need is on. Pool lifetime must be the engine's: the
  `InternalTables`/`Connections` shape (a field on `Engine`, mod.rs:296-343), never a task that
  holds the engine (its `Drop` is what ends things, doc at 352-357).
- The keystore: `SecretRef::{mint, put, get, delete}` + `Secret` (`strata-core/src/secret.rs`),
  all **blocking**; the per-use read pattern is `state/listings.rs:305-310` (ref crosses the
  thread boundary, read happens on the worker).
- The crate (verified from source at 0.13.0): `PostgresConnectionPool::new(HashMap<String,
  SecretString>)` — keys `host/user/db/pass/port/sslmode/sslrootcert/connection_pool_size/
  application_name`; construction eagerly validates (DNS + TCP connect, auth classified via
  SQLSTATE 28P01, pooled `SELECT 1`) — **pool construction is the probe**, all-or-nothing like
  `store::connect`, no separate `reachable` needed.
  `new_with_password_provider(params, Arc<dyn PasswordProvider>)` keeps the password out of the
  param map entirely — implement `PasswordProvider` over the `SecretRef` (keystore read per new
  pool connection, via `spawn_blocking`; a failed read becomes the connection's own error naming
  the fix). `PostgresTableFactory::new(pool)` → `table_provider(TableReference)` applies
  `PostgreSqlDialect` and the federation wrapper (feature `federation`, default-on in the leaf
  crate). The one crate introspection primitive we keep is `get_schemas(conn)`
  (`crates/common/…/dbconnection.rs`); its sibling `get_tables` reads `pg_tables` — tables
  only — which is one of the three reasons the relation listing is **ours** (Build 3).
  `with_unsupported_type_action(UnsupportedTypeAction::String)` per the README's jsonb
  decision. Do **not** use the crate's
  `DatabaseCatalogProvider` (snapshot list, default dialect, no federation — README).
- `StrataCatalogProvider` stays untouched: a database connection registers a *sibling* catalog
  via `ctx.register_catalog` (`CatalogProviderList`), exactly as `build_context` registers
  `strata` (mod.rs:1828). `fold_ident` is mod.rs:1723.
- The MinIO test (`strata-core/tests/object_store_minio.rs`) is the integration-test template:
  one un-`#[ignore]`d `#[tokio::test]`, sequential phases, container held for the duration,
  capacity-refusal retry only, seeded by a different layer than the one under test. CI's split
  (AGENTS §7): the `minio` job runs the container test *binaries* entire
  (`--test object_store_minio`) and carries the container queue; the `test` job is the same
  `--workspace` run with the container tests' own **test-function names** `--skip`ped
  (ci.yml:125-126 — libtest's `--skip` filters names, never `--test` targets).

## Build

1. **Model** (`strata-model/src/connection.rs`) — `Provider::Postgres(PgStore)`:
   ```rust
   pub struct PgStore {
       pub catalog: String,          // SQL identifier; how queries address it
       pub user: String,
       pub sslmode: PgSslMode,       // Disable|Prefer|Require|VerifyCa|VerifyFull
       pub sslrootcert: String,      // path; meaningful for VerifyCa/VerifyFull only
       pub password: PgPassword,     // None | Keystore — the expectation, never a ref
       pub schemas: Vec<String>,     // enabled schemas; serde-default ["public"]
   }
   ```
   `schemas` is the DataGrip-style visibility choice (README): read by the tree and
   completion (DB-05/06), **ignored by the engine** — registration exposes every schema, so a
   query naming a non-enabled one still resolves; nothing in this task filters on it beyond
   carrying the field.
   `address` holds `host:port/database` (one typed string, the HTTP precedent — the server's
   own spelling of what you dial). `url()` → `postgres://{user}@{address}`: the user is part of
   identity because two roles over one database are two connections with two visibility sets
   (and the crate's `JoinPushDown` context agrees — host+port+db+**user**). `check_address`
   grows a Postgres arm (host non-empty, port numeric in range, database non-empty; refused by
   name at the field, per the existing rule). `ProviderId::ALL` 3→4 — **test-pinned, not
   compile-pinned** (`ALL` is a fixed-length const; a loop over it ships the new arm
   silently), and one such loop is load-bearing: the Configure window's LOCATION **TYPE**
   pill iterates `ProviderId::ALL` (`apps/configure/views/location.rs:162`) and would offer
   "read these files through my Postgres connection". Add an object-store predicate
   (`ProviderId::is_object_store()`, or an `ProviderId::OBJECT_STORES` slice beside `ALL`)
   and point the Configure pill and `connections_for` at it — the one place a
   database-connection arm must be *excluded*, named here because no other task owns
   `apps/configure/`. `label()` → `"PG"`. Pin the persisted shape in
   `the_persisted_shape_is_the_tagged_provider`
   — **no UUID appears in it**: the keystore slot is *derived* from identity (README decision),
   so the def stores only `PgPassword::Keystore`, and the ref is
   `SecretRef::derived("pg-password", &def.url())`. `client_config` stays empty for this arm —
   it is `object_store`'s HTTP-client vocabulary and means nothing here (the editor won't offer
   it; DB-04).
   - **`SecretRef::derived(kind, name)`** (`strata-core/src/secret.rs`): `Uuid::new_v5` over a
     new fixed `STRATA_SECRET_NS` namespace const and `"{kind}:{name}"` — beside `mint()`,
     same `SecretRef` type, same `entry()` path, `ai.rs`'s minted refs untouched. Needs the
     `uuid` crate's `v5` feature; doc-comment the contract: *derived refs are for secrets whose
     def is shared — the def must never store the ref, and whoever moves the identity migrates
     the entry*. The lifecycle helpers live **beside it, not in any surface**:
     `secret::migrate_derived(old_ref, new_ref)` (get → put → delete, best-effort) and
     `secret::forget_derived(ref)`, so an identity move or a Forget from *any* caller — the
     editor, a future palette gesture, a project merge — cleans up through one funnel
     (`SettingsCtx::forget_provider`'s precedent). DB-04 and DB-05 call these; neither
     re-implements them.
2. **Catalog-name rules — one copy, named**: `check_catalog_name(existing: &[ConnectionDef],
   candidate: &ConnectionDef) -> Result<(), String>` in `strata-model` beside
   `check_address` — a valid SQL identifier, `fold_ident`-distinct from `strata` and from
   every other connection's catalog name. Called by the engine's `connect` dispatch **and**
   the editor's blocker, the `check_address` shape exactly — never two implementations held
   together by wording discipline.
3. **`engine::db`** (new module beside `engine::store`) —
   `pub async fn connect(ctx, def, pg: &PgStore, passwords: Arc<dyn PasswordProvider>)`:
   build the param map (`to_secret_map`; no `pass` key), construct
   `PostgresConnectionPool::new_with_password_provider` (the probe; classify its errors into
   user-facing prose — bad host/port, refused auth naming the user, missing keystore entry
   naming the machine), set `UnsupportedTypeAction::String`, build the factory, run **one
   introspection query for the whole catalog shape** — `pg_class` joined to `pg_namespace`,
   `relkind IN ('r','p','v','m','f')`, yielding (schema, relation, relkind) in one round
   trip — and register a `DbCatalogProvider` under `pg.catalog` built over that listing.
   The **password seam is the argument**: `Engine::connect` builds the keystore-backed
   provider (derived ref, read per new pool connection on `spawn_blocking`); the integration
   test hands a `StaticPasswordProvider` through the **same** `db::connect` entry point — no
   keystore in a `strata-core` test process (`open_keystore` is called only by the app's
   `main` and by `tests/secret_keystore.rs`, which owns the real-keystore round trip).
   **All-or-nothing shares `settle`'s one body rather than restating it** — the register-or-
   take-back is one function over a registration enum (object store | catalog), because the
   restated version of this contract is the documented burn scar at store.rs:82-89. And
   `db::connect` on a URL the pools map already holds **replaces**: it deregisters the
   catalog the map records for that URL first, so a catalog-name change on an unchanged URL
   (DB-04's rename case) is handled here by construction, never by an editor remembering.
   - **`DbCatalogProvider`/`DbSchemaProvider`** (in `engine::db`): built from the connect-time
     listing above. `SchemaProvider::table_names` is **sync** (DF 54 trait) — it serves the
     cached listing, never a query; `table_type` is **overridden** to answer from the cached
     relkind, because its default is `self.table(name).await` and `information_schema`
     enumerates every catalog through it — without the override, the first `SHOW TABLES`
     builds a provider (one remote introspection) per remote relation. With it,
     `information_schema.tables` / `SHOW TABLES` cost **zero** remote calls; only
     `information_schema.columns` still builds providers per table (bounded by the cache;
     accepted and documented). `table(name)` (async) builds through
     `PostgresTableFactory::table_provider` and **caches the provider per table** — one
     remote introspection per table per connect, so diagnostics' validation never dials per
     keystroke. `Engine::db_listing(url)` over these caches is the one read DB-05's tree,
     the schemas picker and DB-06's completion consume — it answers **scoped and tagged**
     (`Live | EnabledButMissing | NotEnabled` against `PgStore.schemas`), so no consumer
     re-derives visibility. A ↻ re-connects (register_pass phase 1), which rebuilds the whole
     thing — that *is* the refresh. `register_table`/`deregister_table` **refuse with the
     read-only wording** (spelled here — `StrataSchemaProvider::register_table` *succeeds*,
     so there is no precedent to copy; the refusal precedent is
     `StrataCatalogProvider::register_schema`, providers.rs:61-67, which the catalog-provider
     half mirrors).
   - `pub fn disconnect(ctx, catalog: &str, pools: …)`: deregister the catalog, drop the pool.
4. **Engine dispatch** — a `pools` field on `Engine` (keyed by connection URL, the
   `Connections` shape) holding each live pool + its catalog name, so `disconnect(url)` can find
   what to tear down without the def. `Engine::connect` matches the provider: object-store arms
   → `store::connect` (byte-for-byte as today); `Postgres` → `db::connect` on the engine
   runtime. `Engine::disconnect` likewise (it stays sync where it can — catalog deregistration
   is sync; pool drop is too). `Connections::note`/`forget` unchanged. `register_pass`,
   `plan_scan`, the ↻ semantics: **no edits** — phase 1 already calls `Engine::connect` per
   def, serially, connections-first, and a database connection needs exactly that.
5. **Prose rewrites** — every standing doc this task falsifies, in the same change:
   - the two module docs (`connection.rs`, `store.rs`): no-secrets → "no arm takes a secret
     *value*";
   - `connection.rs`'s `url()` doc ("scheme + authority and nothing else") and
     `project.rs:615-616`'s `split_remote` correspondence note — the Postgres `url()`
     carries a path (`postgres://user@host:5432/db`), so both need the database-connection
     caveat; a `postgres://` URL typed into a `CREATE EXTERNAL TABLE LOCATION` splits into a
     URL no connection has and refuses through the existing membership wording (DB-03 pins
     it);
   - `register.rs`'s module doc and the phase-1 doc above `register_pass` ("each registers
     one bucket… tables over that bucket fail") — a database connection's failure consequence
     is different in kind (no def rows; cross-source views are what fail);
   - `CONNECTIONS_SPEC.md` (grows a "Database connections" section: the arm, the catalog
     registration, the derived-ref story including the other-machine degradation);
   - AGENTS.md §2 + INVARIANTS.md: the no-secrets lines, **and** the "one catalog, one
     schema" one-liner gains its scoping word (per-workspace-catalog — the full INVARIANTS
     entry is written about `StrataCatalogProvider` and survives verbatim);
   - `docs/reference/ENGINE.md`'s remote-scheme paragraph; `docs/ARCHITECTURE.md`'s
     engine/catalog paragraphs; and `docs/README.md`'s index row for CONNECTIONS_SPEC, which
     still advertises "the no-secrets credential model".
6. **Integration test** — `strata-core/tests/postgres_federation.rs`, MinIO-shaped: one test,
   sequential phases, `testcontainers-modules` feature `postgres`, seeded over raw
   `tokio-postgres` (a lower layer than the pool/factory under test). Phases at minimum:
   - connect refusals: wrong port (named), wrong password (named), then a good connect
     registers catalog `pg` — **all through `db::connect` with a `StaticPasswordProvider`**
     (the password seam in Build 3; no keystore exists in this process, and the
     real-keystore round trip stays `tests/secret_keystore.rs`'s);
   - `SELECT` through `pg.public.t` returns seeded rows; a filtered scan's EXPLAIN carries the
     filter into `base_sql=` (single-table pushdown);
   - a two-table same-connection join's EXPLAIN shows **one** federated node whose `base_sql`
     contains the JOIN (whole-subplan pushdown);
   - a join of `pg.public.t` × a local CSV/parquet table answers correctly (the mixed plan:
     pg subtree federated, join local);
   - an `IN (subquery)` over remote tables (the `skip_failed_rules=false` cliff — pin what
     actually happens);
   - a `jsonb` column reads as text and `json_get` works over it **locally**; the same
     accessor *federated* fails at this point in the workstream (it unparsers into SQL
     Postgres lacks) — pin that failure here so DB-08, which closes it with the operator
     rewrite, flips this exact assertion;
   - disconnect deregisters: the catalog name no longer resolves.
   **CI edit rides this task**: the container job (`minio:` in ci.yml) gains
   `--test postgres_federation` beside `--test object_store_minio`, and the `test` job's
   `--skip` list gains the new test's **function name(s)** — libtest's `--skip` filters
   names, not targets. The drift property holds (a forgotten skip runs the test
   runtime-less in `test`, which fails loud rather than skipping). The workflow's
   load-bearing prose moves with it: ci.yml's "only file in the workspace that mentions
   testcontainers" comment, the `minio` job's connections-specific step naming, and the
   "whole binary, no edit here" comment all become claims about *both* container tests.

## Acceptance

- Full check green: clippy `-D warnings`, `cargo test --workspace` with a container runtime
  (both container tests), `schema_in_sync` untouched.
- The pinned-at-three `ProviderId` test now pins four; the persisted-shape test carries a
  Postgres literal whose password field is the bare expectation (no UUID anywhere in the
  file), and a unit test pins that `SecretRef::derived` is stable — same inputs, same ref,
  across calls and (by construction) across machines.
- No secret value appears in any def, log line, error string, or `Debug` output (the `Secret`
  type's own guarantees, plus eyeballing the new error prose).
- Grep proof that nothing outside `engine::db` constructs a pool or touches
  `datafusion_table_providers` types.

## Files

`crates/strata-model/src/connection.rs` · `crates/strata-core/src/{secret.rs
(`SecretRef::derived` + lifecycle helpers), register.rs (docs), engine/{db.rs (new), store.rs
(settle sharing + docs), mod.rs}}` · `crates/strata-freya/src/apps/configure/views/location.rs`
(the object-store predicate on the TYPE pill) · `crates/strata-core/Cargo.toml`
(`datafusion-table-providers-postgres = "0.13"`; `uuid` gains the `v5` feature; dev-deps
`tokio-postgres`, `testcontainers-modules` + `postgres` feature) ·
`crates/strata-core/tests/postgres_federation.rs` (new) · `.github/workflows/ci.yml` (targets,
skip names, and the split's prose) · docs per Build 5.
