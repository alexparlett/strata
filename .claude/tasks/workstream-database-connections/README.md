# Workstream: Database connections (DB)

Federated SQL over remote databases: a **Postgres** connection joins the project's catalog as a
queryable catalog of its own, so the editor can `SELECT … FROM pg.public.orders JOIN events …` —
cross-joining file-based tables onto live Postgres data — with filters, projections and whole
same-source subplans pushed down to the server. Built on
[`datafusion-table-providers`](https://github.com/datafusion-contrib/datafusion-table-providers)
(the `datafusion-table-providers-postgres` leaf crate) and
[`datafusion-federation`](https://github.com/datafusion-contrib/datafusion-federation), per the
former's own README.

Eleven tasks. DB-01 is the low-risk groundwork; DB-02 is the mechanism and carries the
integration test; DB-03, DB-04 and DB-08 sit on DB-02 independently; DB-05 — the catalog
redesign — sits on DB-02 + DB-04; DB-06 (gestures + completion) and DB-07 (inspector +
profiling) sit on the tree; DB-10 and DB-11 are write-back — DB-10 (INSERT/CTAS through the
crate's write provider) relaxes DB-03's read-only policy behind a per-connection opt-in, and
DB-11 (statements dispatched to the server: the DDL plus UPDATE/DELETE) sits on it. **01–09 are
in**; DB-10 and DB-11 are open.

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
  at connect, lists tables lazily, and builds providers cached per table so diagnostics'
  validation costs one remote introspection per table per connect, not one per keystroke.
  **The construction sits one level below `PostgresTableFactory`** (DB-08,
  `engine/db/federate.rs`): that factory's own three steps written out — `SqlTable`, the Postgres
  unparser dialect, the federation wrapper — plus an executor of ours wrapping the crate's,
  because `datafusion-table-providers` leaves every `datafusion-federation` rewrite hook at its
  `None` default and those hooks are the only seam between the unparser and the wire.
  `DbSchemaProvider` is still the one construction site; the dialect, the wrapper and the
  per-table cache did not change with the move.
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
- **A remote relation is a place work starts, and both gestures compose into an unrun tab**
  (built, DB-06 ✅). *Query table / Query view* is `view_row`'s own funnel over a three-part
  name, and *Pin as view…* is the workstream's "make it a bare-named def" gesture — composed,
  never executed, because the view's name is a guess and running the statement lands the def
  through the view funnel that already exists. The menu carries those two and nothing else:
  everything else a workspace row offers is about a **def**, and a remote relation has none.
  Two renderers, picked by whose identity a name is: `sql::qualified`/`quote_verbatim`
  (case-preserving, for the server's spelling) and `engine::quote_ident` (fold-preserving, for
  the workspace def the view will become).
- **Completion offers a database's names segment by segment, and the listing needs no warming**
  (built, DB-06 ✅ — this **overturns** the interior-swappable handle DB-06's plan called for).
  The catalog name comes from the def (so a connection that has never answered still offers the
  name a query has to say), the schemas and relations from `db_listing`. There is no warming
  step because DB-02 enumerates a whole database in one round trip at connect: a listing moves
  only at connect and disconnect, both catalog-epoch events, so the ordinary snapshot rebuild
  already sees it. A third segment offers nothing — a remote relation's column list is an
  introspection, and the completion path does no I/O. The *workspace* catalog is deliberately
  not offered as a qualifier: bare names are how every surface addresses it.
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
  and the store still grows nothing** (built, DB-07 ✅). Columns are the cached provider's Arrow
  schema, read through the one capability the tree and the inspector share; profiling keeps
  P3-09's whole shape — opt-in, one entry point (`ProfileActions::ask`, generalized over a
  `ProfileTarget`), the confirm in front, nonce-keyed freya-query result — with a
  **remote-specific expression set** federating to the server (the local set's median is
  `approx_percentile_cont`, which no Postgres speaks and DF 54's dialect cannot override), and the
  confirm's wording saying the scan runs on the database. The one generalization: a remote table
  has no `ProjectState` row, so the profile request lives in a window-side satellite instead of on
  a row — the "store holds the request" rule generalized to "whoever owns the surface holds the
  request", never a remote row minted into the store. Two corrections came out of building it,
  both in DB-07's own file: the **tree's relation opens onto its columns** and the *pane* holds
  that one subscription, because the walk is synchronous and a virtualized row's scope is a slot;
  and **`pg_class.reltuples` is refused rather than shown**, because a row estimate's only home is
  the ROWS row and the completeness bar divides by it.
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
  The friction this left — Postgres-heavy sessions qualifying every name — is closed by DB-09,
  and **not** by the `USE`-shaped session default this bullet expected (see below).
- **A bare name is resolved across the connections on the statement; there is no current
  database** (built, DB-09 ✅ — Alex, 2026-08-15, and this **overturns** the `USE`/session-default
  design DB-09 was written around). Asked as "instead of a default for a session, could the
  planner auto-inject the qualifier when only one database has that name?" — and it is the better
  answer, for a reason the task file had already identified as its whole risk.
  `providers::in_workspace` answers `true` for every bare name, and four rules turn on that: the
  `__snap_` fence, what a write may target, and the two halves of a view's recorded dependencies.
  **Moving the default breaks all four at once**, most sharply the last — a view whose body says
  `orders` would be recorded as reading a workspace table it never read. Resolving on the
  *statement* instead (`sql::qualify`, inside `sql::parse_one`, in front of both the router and
  the planner) leaves all four untouched, because the plan then carries the name the read reached
  and `PlanDeps` records it remote for free. Workspace first, exactly one remote match rewritten
  whole, several refused **by name** with every candidate printed, none left bare. Two carve-outs,
  and only one is permanent: a **create** target is never resolved (it names something that does
  not exist yet, so there is nothing to resolve to, and resolving would read a plainly local
  intent as "make it on the server"), while a **write** target is merely *refused* in
  `ddl::in_database`'s existing sentence for as long as writing to a database is impossible at
  all — a write addresses a relation that already exists, so **it resolves like a read once
  DB-10/DB-11 land** (Alex, 2026-08-15: "I want the write to dispatch just like read does"), and
  those two files carry the seam. The implicit search runs in the schemas a connection **shows**
  (a correction made while using it: a hidden schema was refusing a query about a visible one, by
  naming a relation the tree does not list — `PgStore::schemas` bounds where an *unqualified* name
  is looked for, and never what a name written in full resolves to), and the two surfaces that
  answer about names were taught the same question: the keyword-typo lint stopped squiggling a
  resolvable `orders`, and completion offers a connection's relations where a relation goes, each
  row named by the spelling that reaches it. No mode, no status bar, no `RESET`, nothing for a
  restart to clear. The cost is
  that a bare name can change meaning when a workspace table takes it; completion's **qualified**
  offer (DB-06) is the answer, so what is in the buffer stays explicit.
- **Read-only against the database in v1.** ✅ **built (DB-03, 2026-08-13).** Every DDL arm that
  resolves a target refuses a name inside a database connection's catalog **by name** — one
  sentence, minted once in `ddl::bare_name`, which every such arm already went through;
  `INSERT` reaches it before `Engine::is_internal`, since ownership is not a question to ask
  about a remote relation. The agent's capability is unchanged (verified: the new refusals are
  all at dispatch, which the agent never reaches). Write-back is now **DB-10 + DB-11** (asked
  for 2026-08-15), split by mechanism: what DataFusion can plan (INSERT, CTAS — the crate's
  `PostgresTableWriter`, which DB-03 named) versus what only the server can run (CREATE VIEW,
  CREATE MATERIALIZED VIEW, DROPs, UPDATE, DELETE — span-spliced dispatch over the pool; the
  DML pair joined v1 because once DROP TABLE dispatches, refusing DELETE is a hole, not a
  line). Both sit behind a per-connection `read_only` opt-in defaulting to read-only, so
  DB-03's answer stays the shipped behavior for every existing connection, and the agent stays
  read-only throughout.
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
- **A JSON accessor is mapped only where the operator means the same thing, and the rest refuse
  by name.** ✅ **built (DB-08, 2026-08-15).** The mapped pair is `json_as_text` → `->>` (the
  arrow chain for a path) and `json_contains` → `(… IS NOT NULL)` over the chain. Everything else
  in the family is unmapped **on purpose**, each for a stated semantic difference — `json_get`
  returns Arrow's JSON union, `json_get_str` is NULL where `->>` stringifies, `json_get_json`
  hands back the source slice where `->` hands back normalised `jsonb`, `json_length` counts
  objects as well as arrays. Two corrections to this plan's first draft, both from reading the
  crates: **`json_contains` is not `?`** (Postgres's `?` is true for a string array element and
  takes no integer index, where the local function is false for both — the arrow chain is the
  faithful spelling, at the cost of a GIN index this query never had), and **`json_length` has no
  faithful spelling at all** (`jsonb_array_length` raises on a non-array and the object half is
  set-returning), so it is unmapped rather than approximated. Do not "complete" the table: a
  mapping that is close enough makes a query's answer depend on where it ran.
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
  the federation rule runs — test, don't assume). The JSON-accessor case is **closed** (DB-08,
  2026-08-15): `->>` and `?` are rewritten into Postgres's own operators at the federation seam,
  every other family member refuses by name with the workaround, and a DF-only name that only the
  server can catch keeps Postgres's words with ours after them. `IN (subquery)` reaching the
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
| DB-06 | Gestures + completion over the tree | ✅ | DB-05 |
| DB-07 | Column inspector + profiling for remote tables | ✅ | DB-05 |
| DB-08 | JSON accessors over remote columns: the pushdown rewrite | ✅ | DB-02 |
| DB-09 | Unqualified names resolve across the connections | ✅ | DB-02 |
| DB-10 | Remote DML: INSERT and CTAS into a database connection | ⬜ | DB-02 |
| DB-11 | Remote statements the server runs: DDL + UPDATE/DELETE | ⬜ | DB-10 |

Sources for the research this plan rests on: both repos read at 2026-08-13 HEAD
(`datafusion-table-providers` 0.13.0, `datafusion-federation` 0.5.5), and the codebase map in
each task file's Current state. Docs to keep true as tasks land, **each owned by a task**:
DB-02 — `docs/CONNECTIONS_SPEC.md` (database section), `docs/reference/ENGINE.md`,
`docs/reference/INVARIANTS.md` + AGENTS.md §2 (no-secrets and the one-catalog scoping),
`docs/ARCHITECTURE.md`, `docs/README.md`'s CONNECTIONS_SPEC index row; DB-03 —
`docs/STATEMENTS_SPEC.md`; DB-05 — CONNECTIONS_SPEC's pane section,
`docs/reference/{MODULE_MAP, FREYA_UI, INVARIANTS}.md`; DB-06 — `docs/COMPLETION_SPEC.md`
plus CONNECTIONS_SPEC's gestures and completion sections; DB-07 — INVARIANTS' profiling entry;
DB-09 — CONNECTIONS_SPEC's *Unqualified names* section, `docs/STATEMENTS_SPEC.md` §1 + §4, and
INVARIANTS + AGENTS.md §2;
DB-08 — CONNECTIONS_SPEC's database section (what pushes down) plus INVARIANTS + AGENTS.md §2;
DB-10 — STATEMENTS_SPEC §4, CONNECTIONS_SPEC's read-only toggle, and the "read-only in v1"
sentences in INVARIANTS.md + AGENTS.md §2 (rewritten to lead with what now works); DB-11 —
STATEMENTS_SPEC §4's remote answers.
