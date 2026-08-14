# Engine model

How the DataFusion boundary is shaped, and the policies it enforces. The invariant form is in
[AGENTS.md](../../AGENTS.md) §2; snapshot lifecycle is [SNAPSHOT_SPEC.md](../SNAPSHOT_SPEC.md).


The engine (`strata_core::engine::Engine`) is a **direct-call async facade**: it owns a private
multi-thread Tokio runtime (DataFusion's operators need a Tokio context; query CPU never touches
the render thread), spawns each call onto it, and the caller awaits the `JoinHandle` — which is
executor-agnostic, so Freya's non-Tokio UI executor awaits engine methods like any async fn. No
UI-side runtime, no channels, no request ids. freya-query capabilities call the facade directly
(`engine.query(…)`, `engine.fetch_page(…)`); snapshot lifecycle (supersede / cancel / retire) is
the facade's own bookkeeping — see **`docs/SNAPSHOT_SPEC.md`**. Snapshots are **Arrow IPC**, not
parquet, so a result's type survives the round trip (parquet cannot write a union or a zero-field
struct at all); compressed they are the same size on disk. The export null-gate's exact counts come
from the write pass (`query::SnapshotStats`), not a footer. In Freya the handle is `EngineCtx`
(an `Arc<Engine>` + Deref) held in context — not stored in any god-object `AppState`. Statement
policy is one router in front of dispatch: `sql::validate::classify(stmt, Capability)` answers
`Query` / `Intercept(StmtKind)` / `Refuse(Blocked)`. `Capability::Editor` runs queries and
introspection and **intercepts** the rest — 14 recognised kinds, **all implemented**:
`CREATE EXTERNAL TABLE`, `CREATE TABLE` / CTAS, `INSERT`, `DROP TABLE`, `CREATE` / `DROP VIEW`,
`COPY`, the session statements (`SET` / `RESET`, `PREPARE` / `DEALLOCATE`) and
`CREATE` / `DROP FUNCTION`. Each lands in an app funnel that already exists; the last of them,
typed `CREATE EXTERNAL TABLE`, lands in Table Config's registration path, and its `OPTIONS` are
split by namespace against connections — the reader's keys onto the def, the store's refused
toward Connections on the key alone (`STATEMENTS_SPEC.md` §6.7).
The refusal list: `CREATE DATABASE`/`SCHEMA`, `UPDATE`/`DELETE`,
`INSERT OVERWRITE`, `PREPARE` of a non-query, `SET`/`RESET` of an owned, `runtime.*`, `format.*` or
dialect key, `DROP` of a non-table/view object, reserved `__snap_` names, multi-statement buffers,
and unknown kinds. A statement whose target is qualified into a **database connection's** catalog
is refused by name at dispatch, in one sentence minted by `ddl::bare_name` (`STATEMENTS_SPEC.md`
§4): v1 reads a database and manages nothing in it. Reading one is never refused.
`Capability::Agent` is read-only and refuses every non-query with its original wording.
`Engine::run` is where that classification is *spent*: `Query` delegates to `query()`'s body
byte-for-byte (the only arm that touches the snapshot lifecycle, so "DDL does not retire
snapshots" holds by construction), carrying only the `ReadPolicy` `EXECUTE` needs;
`Intercept(kind)` goes to `engine/ddl/` under the same
in-flight bracket `explain` uses, and `Refuse` returns the editor's own message before anything
can plan. A statement comes back as a `StatementReport` carrying a `StoreEffect` the app folds —
never something to read back out of DataFusion. One statement per Run.
See `docs/STATEMENTS_SPEC.md` and the invariants in `reference/INVARIANTS.md` for the full rule
(default-deny, reserved `__snap_` names, `Blocked` grows and never shrinks).

**The catalog and schema providers are ours, for identity and visibility — never lifecycle**
(ED-03, `engine::providers`, installed in `build_context` before anything registers). The
**workspace** catalog has one schema, `public`, whose `register_schema` refuses: `CREATE SCHEMA`
is impossible by construction rather than by policy. That is a statement about the workspace and
never about the session, which since the DB workstream holds a catalog per live database
connection — registered on `StrataCatalogList`, DataFusion's list plus the `deregister` it has no
method for. `providers::in_workspace` is the one predicate that tells the two apart, and both the
statement gate and the `__snap_` fence (`is_snapshot_ref`) ask it. One schema map keyed by `fold_ident`, so the single namespace
is genuinely case-insensitive. And `table_names()` filters the `__snap_` result snapshots while
`table()` still resolves them — which matters because `table_names()` is the *only* path every
`information_schema` view and every `SHOW` form enumerates through, so one filter hides the spool
from all of them and keeps `__strata_ord` out of `information_schema.columns`, while paging, chart,
export and retirement (all by name) notice nothing. That is what makes
`datafusion.catalog.information_schema` safe to default **on**, so `SHOW TABLES` works on a fresh
project — it rewrites to `SELECT * FROM information_schema.tables` and errors outright when the key
is off, which `DESCRIBE` never did (`describe_table_to_plan` goes straight to `get_table_source`).
Everything else is `MemorySchemaProvider`'s behaviour verbatim. Lifecycle
is **not** here and cannot be: `register_table` is sync and carries no caller identity, so it can
neither spool a CTAS result nor tell a user's `DROP` from the deregister every re-scan does
(`STATEMENTS_SPEC.md` §3, settled). `CREATE DATABASE` is likewise not stoppable at a provider —
DF registers it into the `CatalogProviderList`, whose `register_catalog` returns an `Option` — so
the router's `Blocked::CreateDatabase` is its only gate.

**A table's data can be Strata's own, and that is a flag on the def rather than a second kind of
table** (ED-04, `engine::ddl::tables`). `CREATE TABLE` / CTAS hands the *parsed* statement to
`SessionState::statement_to_plan` — which executes nothing, refuses every clause DataFusion does
not implement in its own words, and resolves a declared column list against the query — then
spools `CreateMemoryTable.input` through a `LogicalPlan::Copy` node `STORED AS ARROW` into
`.strata/tables/.tmp-…/`, renames it into `<slug>/`, and registers it through `register_external`
with `TableSpec { format: Arrow, internal: true }`. No SQL is re-rendered and no span is sliced,
so the query that runs is the one the user wrote. The def that comes back is an ordinary
`TableDef` with `origin: Internal` and a project-relative source, so replay, the persist funnel
and the headless host need no new code; `Engine::set_data_dir(root)` is what tells an engine which
project it may write into, and a CTAS on an engine with no project refuses politely.
`StrataArrowFormat` wraps `ArrowFormat` to answer `infer_stats` from the IPC footer (a metadata-only
read of each batch header), because otherwise the one table Strata itself wrote could not say how
many rows it holds. Two settings ride with it: `datafusion.runtime.list_files_cache_limit` defaults
to `0`, because DF 54's 1 MiB / infinite-TTL default makes every re-listing answer with the
previous file set; and `register_external` refuses a `__snap_`-prefixed name outright, so a
hand-edited `project.json` cannot do what a typed statement cannot.

The two statements that then write over such a table are ED-05, in the same module. `INSERT` plans
the statement and gates only the **target** — outside `Engine::is_internal` (an external table, or
a view) it is refused, as is any write op that is not `Append`; everything after that is
DataFusion's own INSERT path, appending one LZ4-frame IPC file per statement with no compaction,
and the plan that was gated is the plan that runs. `DROP TABLE` works on **both** origins and is
the one place a table is dropped: `ddl::tables::drop_table` deregisters first, discards
`.strata/tables/<slug>/` only where the def is internal, names the dependent views without
cascading, and answers with `StoreEffect::TableRemoved`. The catalog pane's confirm is a gesture in
front of that same call (`Engine::drop_table`, after its store-first write), not a second copy of
it — before ED-05 the pane's `deregister` orphaned an internal table's data forever — and the
sentence it shows before the fact is the engine's `ddl::drop_intent`, paired with the report's own.
The discard is **by rename** (`ddl::tables::discard`): the directory moves into a `.tmp-…` sibling
and is only then walked, the mirror of the spool's publish-by-rename, so an interrupted delete
leaves what `tidy_strata_dir` sweeps rather than a half-emptied directory under a live table name.
The rename is the operation and the removal is housekeeping — a failure to finish it is logged, not
returned. And because an `INSERT` is one file with no compaction, that delete is not instant, so
`Engine::drop_table` holds a `BackgroundGuard` (`Lifecycle::background`, the count `export`
already used) and the close-while-running confirm asks before a window takes the runtime away.

**A remote scheme is something we register, and a connection is what registers it.** DataFusion
core resolves nothing: there is no built-in "read `s3://…`", so an embedder builds an
`object_store` and calls `register_object_store` **per bucket** or every scan of it fails with *no
suitable object store found*. That call is the whole of what a connection does — which is why a
[`ConnectionDef`](../CONNECTIONS_SPEC.md)'s identity is exactly what the registry keys on (scheme +
authority, no path — so it is `ConnectionDef::url()`, **never the bucket**: `s3://lake` and
`gs://lake` are two connections, and anything addressing one by bucket answers one row twice and
leaves the other unanswered), why the def stores the **authority alone** and derives the scheme
from the provider (two statements of one fact can disagree), and why connections are the **first**
phase of `register::register_pass`: a table registered before its bucket's store fails on a def
that is perfectly correct. `engine::store::connect` is all-or-nothing — it probes the credential
chain *before* registering, and on `Err` deregisters whatever an earlier pass left, so a connection
is never both refused and live and the `Reg` row that folds its outcome means what it says.
`object_store` alone is env-only (it does not read `~/.aws` profiles and does not do SSO), so the S3
arm wraps **`aws-config`**'s resolved credentials in an `object_store::CredentialProvider` —
resolving per request, so short-lived credentials refresh themselves. **Ambient and Named profile
are two different providers**, not one chain with a setting: naming a profile on the default chain
only configures its Profile arm, which sits behind `Environment`, so an exported `AWS_ACCESS_KEY_ID`
silently wins and the chosen profile is never read. **No arm anywhere in that module takes a
secret value**, and that absence is the feature: a connection carries a profile *name* and a key
*file path*, never a key.

**A database connection registers a *catalog*, on the same terms and against a different
registry.** `Provider::Postgres` is a fourth arm of the same `ConnectionDef`, and everything about
it that differs lives in `engine::db`: a connection pool whose construction *is* the probe (DNS,
TCP, auth, `SELECT 1`, all-or-nothing exactly like `store::connect` — there is no separate
reachability step, because a database either let us in or did not), a one-round-trip `pg_class`
enumeration, and a catalog provider registered on `StrataCatalogList` under a **user-chosen SQL
identifier**, because SQL cannot address `postgres://host/db` and relations must be reachable as
`pg.public.orders`. Both arms settle through one `connect::settle`, which takes the take-back as an
argument: the registries differ, the contract does not. `StrataCatalogList` exists because
DataFusion's `CatalogProviderList` can register a catalog and never remove one — without removal a
forgotten connection would answer for the life of the window. The whole database comes through
automatically: **discovery gets catalogs, declaration gets defs**, so there are no per-table defs,
providers are built lazily and cached per relation, and `SchemaProvider::table_type` is overridden
so `SHOW TABLES` costs nothing remote. Federation (DB-01's optimizer rule and query planner) is
what turns a same-connection join into one remote statement; the full picture, pushdown
expectations included, is [`CONNECTIONS_SPEC.md`](../CONNECTIONS_SPEC.md). Its **password** is the
one credential Strata holds: kept in the OS keystore under a ref *derived* from the connection's
identity, read per pool connection, and never in the def — which carries only the expectation that
one exists.

**The SQL function set is the live registry, not a list we keep.** `build_context` registers
`datafusion-functions-json`'s Postgres-style accessors (`json_get` / `->` / `->>`; **not** `?`,
which sqlparser reads as a placeholder before the crate's planner sees it — `json_contains` is the
spelling that works) over Utf8
columns holding JSON text, and that call is the whole integration: `engine::functions::snapshot`
walks `ctx.udfs()`, so anything registered reaches autocomplete and the completion detail with no
per-function table and no way for the completion pool and the engine to disagree. Adding a UDF
family means one `register_*` call in `build_context` and nothing else.
The walk runs at `Engine::new` and again after a `CREATE` / `DROP FUNCTION` (ED-09) and **nowhere
else**: `engine::functions::Functions` holds the result as a swappable `Arc<FunctionCatalog>`
beside the folded names this session created, which is what fences a built-in off from both
statements. `Engine::functions()` hands out the handle, so the language service's per-epoch
snapshot carries the set rather than copying it (`docs/STATEMENTS_SPEC.md` §6.6). The union-tolerant JSON
reader (`engine::json_poly`) is what makes the accessors pay off over mixed-shape files.
Strata has **its own built-ins** too, and they are exactly that one call: `engine::udfs`
(QE-01) holds `struct_keys` / `struct_entries` / `struct_get` / `to_json`, the struct
vocabulary DataFusion has no spelling for — which keys does *this row* have, walk a keyed
object with the values still typed, index one by a key computed per row — over the `Struct`
that `json_poly` infers every JSON object as. Beside them, `regexp_extract_all` (QE-02) —
every non-overlapping match of a pattern, where DataFusion 54's `regexp_match` returns only the
first, so multi-match extraction per row is `unnest(regexp_extract_all(col, pattern))` instead of
a recursive CTE; its compile caching is DataFusion's own two functions rather than a second one.
A new built-in belongs there, and it inherits the `DROP FUNCTION` fence for free
(`Functions::created` stays false for it).

**What DataFusion 54 cannot do around them is a ledger, not folklore.** The limits a user
driving deep JSON hits — a `UNION ALL` branch is an independent scan of its source, a
`json_get_json` result carries `arrow.json` metadata that will not unify against plain text
across a recursive CTE's branches, a FROM-clause `UNNEST` alias has no addressable fields,
`string_agg` orders only by what it aggregates once `DISTINCT` is in play, and `UNNEST` in
FROM cannot reference a nested outer column — are written down once, each with a workaround
re-run against this build, what could only be taken on the field report's word, and the note to
file upstream from, in
`.claude/tasks/workstream-query-ergonomics/README.md`. The pin is structural
(`datafusion-table-providers` + `datafusion-federation` hold the four crates together at 54),
so the list is revisited at the next bump rather than per report. When that workstream closes,
whatever is still true moves into its `SETTLED_TASKS.md` entry — the folder goes, the ledger
must not go with it.

> The Dioxus-era `Command`/`Event` channel protocol + worker loop was deleted along with the
> Dioxus app itself. The engine's only interface is the direct-call facade above.
