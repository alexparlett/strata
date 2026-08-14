# Workstream: Database connections (DB)

Federated SQL over remote databases: a **Postgres** connection joins the project's catalog as a
queryable catalog of its own, so the editor can `SELECT … FROM pg.public.orders JOIN events …` —
cross-joining file-based tables onto live Postgres data — with filters, projections and whole
same-source subplans pushed down to the server. Built on
[`datafusion-table-providers`](https://github.com/datafusion-contrib/datafusion-table-providers)
(the `datafusion-table-providers-postgres` leaf crate) and
[`datafusion-federation`](https://github.com/datafusion-contrib/datafusion-federation), per the
former's own README.

Eight tasks. DB-01 is the low-risk groundwork; DB-02 is the mechanism and carries the
integration test; DB-03, DB-04 and DB-08 sit on DB-02 independently; DB-05 — the catalog
redesign — sits on DB-02 + DB-04; DB-06 (gestures + completion) and DB-07 (inspector +
profiling) sit on the tree.

## Decisions already made (do not re-litigate; the reasoning is recorded here)

- **Versions move in lockstep with the `datafusion` pin.** DataFusion 54 ↔ arrow 58 ↔
  `datafusion-table-providers` 0.13.0 ↔ `datafusion-federation` 0.5.5 (verified against both
  repos' Cargo.tomls and crates.io, 2026-08-13). `TableProvider`, the unparser and the federation
  types all cross the crate boundary, so app and providers must share one DataFusion — a future
  DF bump bumps all four together, and the comment on the dependency says so.
- **Postgres only in v1, through the leaf crate.** The providers crate's uniform seams
  (`DbConnectionPool`, per-source `XxxTableFactory` → `table_provider(TableReference)`) mean
  MySQL/SQLite/DuckDB later are further `Provider` arms over the same mechanism, not a redesign.
  We do not build a generic RDBMS abstraction ahead of a second database — the per-arm `match`
  *is* the mechanism.
- **A database connection is a fourth `Provider` arm, not a second kind of thing.** Same
  `ConnectionDef`, same `ConnRow`/`Reg<()>`, same editor window, same `register_pass`
  phase 1, same Forget confirm — the `TableOrigin` lesson. (Not "same pane": the Connections
  pane retired with DB-05's tree, which gives a database node its own subtree.) The
  compile errors a fourth arm forces through `url()`, `check_address` and every `match` are
  the intended checklist — with two deliberate exceptions to keep in view:
  `ProviderId::ALL` is pinned by **test**, not the compiler, and the loops over it are where
  a new arm ships silently (the Configure TYPE pill needs the object-store predicate,
  DB-02).
- **DataFusion cannot remove a catalog, so the engine owns the catalog list** (built at DB-02,
  and the one structural surprise in it). `CatalogProviderList` is `register_catalog` /
  `catalog_names` / `catalog` and nothing more; `MemoryCatalogProviderList` is insert-only. Without
  removal a forgotten database connection answers `pg.public.orders` for the life of the window —
  the inverse of the catalog-is-the-store rule. `providers::StrataCatalogList` is DataFusion's list
  plus `deregister`, installed via `SessionStateBuilder::with_catalog_list` so the workspace
  catalog lands in it. It refuses nothing, so `CREATE DATABASE`'s gate stays the router's.
- **Connecting registers a *catalog*, not an object store.** `store::connect`'s
  `register_object_store` path does not apply; a Postgres connection builds a connection pool
  (whose construction *is* the probe: DNS + TCP + auth + `SELECT 1`, all-or-nothing exactly like
  `store::connect`) and registers a DataFusion `CatalogProvider` on the `CatalogProviderList`
  under a **user-chosen catalog name** — a new def field, the first that is an SQL identifier,
  because SQL cannot address `postgres://host/db` and tables must be reachable as
  `pg.public.orders`. `strata` (and a fold-collision with another connection's name) is refused.
  `StrataCatalogProvider` (one catalog, one schema, `register_schema` refuses) is untouched:
  ED-03's refusal fences *user-typed* `CREATE SCHEMA`; `CREATE DATABASE`'s only gate was always
  the router, and Strata registering a catalog programmatically is Strata's own act.
- **The whole database comes through automatically; there are no per-table defs and no manual
  adds** (confirmed with Alex, 2026-08-13). Connect registers every schema the role can see and
  every table in them — three-part names, remote schemas preserved (`pg.analytics.sessions`) —
  with nothing typed per table. A manual def-per-remote-table model was considered and
  rejected: it restates configuration the server owns, goes stale silently, costs an
  introspection per def per pass, and mints failure states for things whose only real failure
  is the connection's. **Discovery gets catalogs; declaration gets defs**: a bucket cannot say
  what its tables are (someone must declare globs + format + options, and a declaration can
  fail — that is what the `Reg` rows exist to show), while a database answers for itself — so
  object stores stay connection → def and self-describing sources become catalogs. This is the
  "multiple databases" shape, scoped to where it is true. **Pinning is a view**: `CREATE VIEW
  orders AS SELECT * FROM pg.public.orders` puts a bare-named, store-rowed, honestly-failing
  def in the workspace with zero new machinery.
- **We write our own catalog/schema provider over the pool; the crate's
  `DatabaseCatalogProvider` is not used.** Three reasons, all verified in its source: it
  snapshots the schema/table list at construction (a ↻ could not refresh it), it builds plain
  `SqlTable`s with the default dialect, and it skips the federation wrapper — so the generic
  path would silently forfeit exactly the pushdown this workstream exists for. Ours enumerates
  at connect, lists tables lazily, and builds providers through `PostgresTableFactory` (dialect
  + federation included), cached per table so diagnostics' validation costs one remote
  introspection per table per connect, not one per keystroke.
- **Federation is installed unconditionally, in `build_context`.** ✅ **built (DB-01,
  2026-08-13).** `datafusion_federation::default_optimizer_rules()` (inserts
  `FederationOptimizerRule` after `scalar_subquery_to_join` — the ordering is load-bearing,
  decorrelation must run first) plus `FederatedQueryPlanner` as the query planner, both on a
  `SessionStateBuilder` that inlines what `SessionContext::new_with_config_rt` did. With no
  `FederatedTableProviderAdaptor` in a plan the rule rewrites nothing, so local-only projects
  pay a walk that finds nothing. The query-planner slot is
  single-occupancy: we use the default planner today, and `FederatedQueryPlanner` is the default
  planner plus one extension planner, so this is a no-op swap — but a future custom planner must
  *include* `FederatedPlanner` rather than replace it. Statement routing is in front of
  DataFusion and is unaffected. **One correction from building it:** "no-op" holds for every plan
  DataFusion can execute, but not *structurally* — the rule's expression walk refuses
  `Expr::InSubquery` before it looks at providers, so a surviving one now errors naming the
  federation rule. Measured both ways: the only shape that reaches it is one DataFusion's
  physical planner already refused, so nothing that worked stopped working and only the wording
  changed. DB-01's file has the table.
- **The no-secrets rule is rewritten, not routed around.** Alex (2026-08-13): the rule was a
  consequence of the keystore not existing when W7 was built, not a standing prohibition. The
  password is captured in the editor exactly as `ai/keys.rs` captures a provider key (typed →
  put; cleared → delete) and read **per use** inside the pool's `PasswordProvider` — never
  cached, never serialized. The prose invariants in `connection.rs`, `engine/store.rs`,
  `CONNECTIONS_SPEC.md`, AGENTS.md §2 and INVARIANTS.md are rewritten deliberately in the same
  change: *no arm takes a secret value; a secret lives in the keystore, read per use*.
- **A connection's secret is indexed by a *derived* ref, so the committed def never carries a
  machine-local fact** (Alex, 2026-08-13). Not a minted-random `SecretRef`: with one of those
  in the committed `project.json`, every colleague's "enter my password" would mint a fresh
  UUID and rewrite the def — two machines ping-ponging the ref through git forever. Instead
  `SecretRef::derived(kind, name)` is `Uuid::new_v5(STRATA_SECRET_NS, "{kind}:{name}")` —
  e.g. `derived("pg-password", def.url())` — deterministic, so the same def addresses the same
  keystore slot on every machine while each machine's keystore holds its own entry. The def
  therefore stores only the **expectation** (`PgPassword::{None, Keystore}`), never the ref:
  storing a derivable value beside the fields it derives from is two statements of one fact
  that can disagree. Consequences carried by the tasks: an identity edit (address or user)
  moves the derived ref, so the editor's Save **migrates** the entry (get old → put new →
  delete old, best-effort — the local keystore may have no entry) exactly as it already
  deregisters a moved URL; Forget deletes the entry without needing a stored ref; and on a
  machine with no entry the row settles `Reg::Failed` naming the fix ("no password stored on
  this machine…"), the same honest shape as an expired SSO session — and re-entering it
  touches nothing in git.
- **The catalog pane is redesigned into one data-sources tree; the catalog *invariant* stands
  untouched underneath it** (built, DB-05 ✅; Alex, 2026-08-13 — this overturns this plan's first draft, which
  tucked remote browsing into the Connections pane; that made remote tables second-class and
  wedged a second hierarchy under a flat pane). The reference UX is **DataGrip**: one tree
  whose top level is data sources — the **project workspace** (the def-driven tables / views /
  saved queries, `Reg` failure rows intact), each **database connection** (enabled schemas →
  Tables and Views groups → columns), and each **object-store connection** (status node whose
  children are navigation links to the workspace defs reading through it — a jump, never a
  second row). The separate Connections pane **retires**; status glyphs, Edit / Forget and the
  `+` move onto the tree's nodes and header. What does *not* move is where truth lives: defs
  and their failure states stay `ProjectState` rows, remote listings stay discovery reads off
  the engine's caches — the redesign changes what the pane paints, not what holds the data,
  so "never a `FetchCatalog`" survives intact.
- **Schema visibility is a per-connection choice on the def, and it scopes display, never
  resolution.** `PgStore.schemas` (enabled schemas, default `public`) is committed
  configuration — DataGrip's "N of M schemas" model. The tree and completion show enabled
  schemas only; a query naming a non-enabled schema still resolves and runs (the engine
  registers the whole catalog regardless — lazy providers make that free). The picker is a
  gesture on the connection's tree node reading the connect-time enumeration — it cannot live
  in the editor, because a new connection has no schema list until its first green settle.
  And the listing our provider serves reads **`pg_class`** (`relkind IN r,p,v,m,f`), not the
  crate's `pg_tables` — remote *views*, matviews, partitioned and foreign tables must show
  and resolve, or the tree lies about what is queryable.
- **The inspector and profiling treat a remote table as first-class, on the existing terms —
  and the store still grows nothing.** Columns are the cached provider's Arrow schema (free);
  profiling keeps P3-09's whole shape — opt-in, one entry point (`ProfileActions::ask`,
  generalized over a `ProfileTarget`), the confirm in front, nonce-keyed freya-query result —
  with a **remote-specific expression set** federating to the server (the local set's median
  is `approx_percentile_cont`, which no Postgres speaks and DF 54's dialect cannot override —
  DB-07 has the proof), and the confirm's wording saying the scan runs on the database. The one generalization: a remote table has no `ProjectState`
  row, so the profile request lives in a window-side slot instead of on a row — the
  "store holds the request" rule generalized to "whoever owns the surface holds the request",
  never a remote row minted into the store (DB-07).
- **The multi-database data model lands at the engine level; the def model stays flat — by
  argument, not inertia** (raised twice by Alex, settled 2026-08-13). After DB-02 the engine's
  model *is* DataFusion's native one: the `CatalogProviderList` holds N databases, schemas
  inside them, three-part addressing throughout, and the workspace is merely the default
  catalog bare names sugar into. What stays flat is `ProjectDefs`: workspace tables/views keep
  bare-name identity in one namespace, because (a) a parquet workspace has no server-side fact
  for a schema level to reflect — it would be taxonomy, and shallow sources beside deep ones
  is DataGrip's own shape; and (b) bare-name identity is the deepest assumption in the app
  (stores, renames, tab `Origin`s, view deps, history dedupe, `fold_ident`, the `__snap_`
  fence, the funnels, completion, the agent vocabulary) — re-keying all of it buys
  organization most projects never use. ED-03's `register_schema` refusal stays sound *scoped
  to the workspace catalog* (sync, no caller identity — unchanged), and the router's
  `CREATE DATABASE`/`SCHEMA` refusals stay (a statement can mint neither credentials nor a
  persistence story); neither fences the mechanism's programmatic catalog registration. The
  door to workspace schemas stays **additively** open: an optional `schema` field defaulting
  to `public`, workspace-catalog schema registration, a tree that already paints the level.
  Known friction accepted for v1: Postgres-heavy sessions qualify every name; the fix if it
  bites is a `USE`-shaped session default-catalog gesture (a session feature over config keys
  we currently fence as owned — its own small follow-up, not a def-model change).
- **Read-only against the database in v1.** ✅ **built (DB-03, 2026-08-13).** Every DDL arm that
  resolves a target refuses a name inside a database connection's catalog **by name** — one
  sentence, minted once in `ddl::bare_name`, which every such arm already went through;
  `INSERT` reaches it before `Engine::is_internal`, since ownership is not a question to ask
  about a remote relation. The agent's capability is unchanged (verified: the new refusals are
  all at dispatch, which the agent never reaches). Write-back (`read_write_table_provider` exists
  in the crate) is a possible follow-up workstream, not a seam to pre-build.
  Three corrections came out of building it, each recorded in DB-03's own file: the `__snap_`
  namespace is the **workspace catalog's** and the predicate says so (`is_snapshot_ref`); a
  view's dependencies are **two lists**, workspace scans bare and remote scans qualified, or a
  cross-source view is indistinguishable from a workspace table of the same bare name; and a
  relation that vanishes server-side is a **reconciliation** with its staleness bound stated
  where the message is built (`catalog::view_error`).
- **`jsonb` (and unknown exotic types) map to text, not to a refusal.**
  `UnsupportedTypeAction::String`: a `jsonb` column arrives as `Utf8` JSON text, which the app's
  own Postgres-style accessors (`json_get`/`->`/`->>` over Utf8) already handle — the default
  (`Error`) would instead make any table with one exotic column entirely unreadable. This is
  representation honesty, not silent corruption: the value is intact, only the type is wider.
- **Pushdown expectations, so nobody re-measures them.** Single-table filter/projection/LIMIT
  push down even without federation (the `SqlTable` scan unparses them; unsupported exprs fall
  back to `Unsupported` and re-apply locally). Same-connection joins/aggregates/TopK federate
  into one remote statement (`JoinPushDown::AllowedFor(host+port+db+user)`). A pg × parquet
  join is `Ambiguous` at the join node: the largest single-provider subtree under the pg side
  still federates; the join itself runs locally. A federated subplan that unparses to SQL the
  server rejects (a DF-only function reaching remote SQL) fails **loudly at execute time** —
  there is no silent local fallback, and the results pane's error path is the surface, per the
  existing "a run failure is the results pane's" rule.

## Known gaps (measured at DB-02, not theoretical)

- **DataFusion 54's unparser drops the qualifier rebase on a derived table.** When a federated
  subplan puts a projection under another projection (or under a window), the unparser emits
  `… FROM (SELECT …) AS "derived_projection"` and leaves the *outer* column references qualified
  by the original relation — so the statement names a relation its own `FROM` has aliased away
  and Postgres answers `42P01`. Postgres would run the intended statement perfectly well; the
  defect is entirely in the SQL we generate, and there is no newer `datafusion-federation` or
  `datafusion-table-providers` to bump to (0.5.5 / 0.13.0 are latest).

  **This used to break every federated read**, because the snapshot ordinal was a plan-level
  `row_number() OVER ()` and so rode into the remote statement on top of the user's projection.
  DB-02 moved the ordinal into the writer (`docs/SNAPSHOT_SPEC.md` §9) — which it should have
  been anyway, since the ordinal is defined as numbering the stream the writer consumes — and
  the gap shrank to genuine user shapes: a window or an expression over an already-projected
  federated subquery (`SELECT id, row_number() OVER () FROM (SELECT id FROM pg.public.orders)`).
  Pinned in `tests/postgres_federation.rs` the way DB-01 pinned `IN (subquery)`; closing it
  needs an upstream unparser fix, not anything this workstream owns.

## Known risks (watch during DB-02, verify in its test)

- **Unparser gaps**: DF-specific functions (created macros should be `simplify`-expanded before
  the federation rule runs — test, don't assume). The JSON-accessor case — `json_get` over a
  *remote* column reaching Postgres as unknown SQL — is not accepted as a gap: **DB-08 closes
  it** by rewriting the `functions-json` family into Postgres's own operator syntax at the
  federation seam, with a named refusal for anything unmapped. `IN (subquery)` reaching the
  federation scanner is `not_impl_err`, and `datafusion.optimizer.skip_failed_rules` defaults
  to `false`, so such a query errors rather than degrading — DF's decorrelation usually
  removes these first; the integration test pins one. **Measured at DB-01**: the refusal is
  raised by the rule's *expression* walk, before any provider is consulted, so it is not
  specific to a remote query — but the one shape that survives decorrelation to reach it
  (`SELECT a IN (SELECT …) FROM t`) is one DataFusion's physical planner already refuses, so
  the local blast radius is a changed error message and nothing more. DB-01's file has the
  before/after table.
- **TLS is native-tls** (Security.framework on macOS — fine, and no bundle-self-containment
  impact); `verify-ca`/`verify-full` are the crate's emulation over tokio-postgres.
- **Pool lifetime**: bb8 spawns a driver task per connection; the pool must live on the
  `Engine` (the `InternalTables` shape) and every call spawn onto the engine runtime, so
  teardown is the engine's `Drop`.

## Tasks

| # | Task | Status | Depends on |
|---|---|---|---|
| DB-01 | Federation groundwork in `build_context` | ✅ | — |
| DB-02 | The Postgres arm: model, secrets, pool, catalog provider, registration | ✅ | DB-01 |
| DB-03 | Statement policy over remote catalogs | ✅ | DB-02 |
| DB-04 | The connection editor's Postgres form | ✅ | DB-02 |
| DB-05 | The data-sources tree: the catalog pane redesigned | ✅ | DB-02, DB-04 |
| DB-06 | Gestures + completion over the tree | ⬜ | DB-05 |
| DB-07 | Column inspector + profiling for remote tables | ⬜ | DB-05 |
| DB-08 | JSON accessors over remote columns: the pushdown rewrite | ⬜ | DB-02 |
| DB-09 | A current database, so unqualified names resolve | ⬜ | DB-02 |

Sources for the research this plan rests on: both repos read at 2026-08-13 HEAD
(`datafusion-table-providers` 0.13.0, `datafusion-federation` 0.5.5), and the codebase map in
each task file's Current state. Docs to keep true as tasks land, **each owned by a task**:
DB-02 — `docs/CONNECTIONS_SPEC.md` (database section), `docs/reference/ENGINE.md`,
`docs/reference/INVARIANTS.md` + AGENTS.md §2 (no-secrets and the one-catalog scoping),
`docs/ARCHITECTURE.md`, `docs/README.md`'s CONNECTIONS_SPEC index row; DB-03 —
`docs/STATEMENTS_SPEC.md`; DB-05 — CONNECTIONS_SPEC's pane section,
`docs/reference/{MODULE_MAP, FREYA_UI, INVARIANTS}.md`; DB-06 — `docs/COMPLETION_SPEC.md`;
DB-07 — INVARIANTS' profiling entry.
