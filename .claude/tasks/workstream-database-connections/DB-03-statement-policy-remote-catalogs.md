# DB-03 · Statement policy over remote catalogs

**Workstream:** Database connections · **Status:** ✅ · **Depends on:** DB-02

## Goal

Every statement the router intercepts answers sensibly when its target is qualified into a
database connection's catalog: refused **by name**, with the refusal saying what the name is (a
remote table in connection *X*) and that Strata does not manage remote objects. Queries —
including `EXPLAIN`, `PREPARE` of a query, views *reading* remote tables — keep working; the
agent's read-only capability is verified unaffected. This is an audit with teeth, not a new
mechanism: the work is wording + tests over arms that already exist.

## What was built (2026-08-13)

The audit held: `bare_name` really is the single choke point, every arm reaches its target
through it, and no arm grew a second copy of the check. What the task added around it:

- **`ddl::bare_name` takes the session and mints the new sentence** (`ddl/mod.rs`). The
  workspace test moved out to `providers::in_workspace` — one predicate, since two rules turn
  on it — and the failure branches on whether the qualifier names a registered catalog:
  `in_database` (*"'pg.public.orders' is in the database connection 'pg'. Strata reads remote
  tables; it does not create, drop or write them"*) or the older `elsewhere` for a qualifier
  that names nothing. Not parameterised by `what`: the sentence is about the catalog, not about
  whether a table or a view was being made in it. The catalog list is what is asked, because it
  is what *resolves* the name, and it answers with the connection's own spelling.
  `INSERT` reaches it **before** `Engine::is_internal`, which is not a question to ask about a
  relation whose data Strata could never own.
- **The `__snap_` fence scoped to the workspace catalog.** `query::is_snapshot_ref` is
  `is_snapshot_name` under `in_workspace`, beside the function that mints the names, and
  `validate::is_reserved` reads the qualifier through DataFusion's **own**
  `object_name_to_table_reference` so the reference judged is the reference the planner would
  resolve. Deliberately syntactic — `classify` stays a pure function of the parsed statement, and
  a qualifier naming no catalog resolves nowhere anyway. A remote `__snap_x` is read like any
  other relation and written to like any other remote one (refused for being remote).
  The database schema provider deliberately grew no hiding filter; `providers`' module docs say
  so beside the workspace one that has it.
- **`PlanDeps` split into `tables` + `remote`** (`catalog.rs`), carried through `ViewMeta` and
  the store's `ViewInfo` (`deps` / `remote_deps`). This is the load-bearing correction: recorded
  by bare component, `pg.public.orders` was indistinguishable from a workspace `orders`, so
  dropping that table named a view that never read it and `view_problem` cried wolf over a
  relation the store has no row for. `dependent_views` / `readers` / `left_invalid` needed no
  change once the split existed. `view_problem` now documents *why* it does not check the remote
  half; the agent's `Described::View.reads` takes both halves, because that question is not about
  rows.
- **`catalog::view_error`** — the view funnel's counterpart to `register_error`: one diagnosis
  (`missing_relation`) in front of `readable`'s unwrapping, turning DataFusion's `table
  'pg.public.orders' not found` into a sentence naming the connection and the refresh. The
  staleness bound is stated where the message is built: a connection's relation list is its
  connect-time enumeration, so "not in the connection" means "not in what it last told us", and
  a ↻ re-runs the pass, which re-connects.
- **The agent vocabulary's two name-answering tools.** `Engine::database_catalogs` and
  `Engine::describe_remote` (+ `db::RemoteRelation`) are the engine reads behind them.
  `list_tables` gains `databases` — outside `total`, outside the window, unfiltered by
  `matching`, because a narrowed listing that dropped them would read as a project with none;
  the entries stay defs-only, since a database has no defs and enumerating one inside a paged
  listing of something else would be an unbounded remote read. `describe_table` asks the store
  first (a def always wins) and falls through to the engine for a qualified name it has no row
  for: `Described::Remote` renders as columns + `connection` + the server's own word for whether
  it is a view. Answering `not found` for a relation the agent can query was the dishonesty.
- **Four review corrections**, each with a regression test: `in_workspace` and
  `database_catalog` fold the catalog before comparing (the catalog list resolves by
  `fold_ident`, so a quoted `"STRATA"` is the workspace — compared raw it escaped the `__snap_`
  fence, which the old any-part test had caught, and it made the refusal call the workspace a
  database connection); `describe_table` falls through to the remote path on `NotFound` **only**,
  so a `WindowGone` is not masked by a successful answer; and `describe_remote` returns
  `Result<Option<_>>`, so a relation the connection lists whose introspection fails is a fault
  with the provider's own sentence rather than a not-found — existence is asked of `table_exist`,
  which reads the connect-time listing and costs no round trip.
- **`Capability::Agent` is unchanged**, verified rather than assumed: it refuses every non-query
  in its shipped wording before any of the above is reached (the new refusals are all at
  *dispatch*, which the agent never gets to), and a plain remote read passes `policy_verdicts`.

### The fourteen kinds

Recorded in `docs/STATEMENTS_SPEC.md` §4 as a table, and pinned by `engine::ddl::tests`:

| Kind | Remote-qualified target |
|---|---|
| `CreateTable`, `Ctas`, `Insert`, `DropTable`, `DropView`, `CreateView`, `CreateExternalTable` | refused, naming the connection |
| `Copy` | runs — its target is a path, and a remote source is an ordinary read |
| `Prepare`, `Deallocate` (and `Execute`, a query verdict) | run — a prepared body over a remote relation is a query |
| `Set`, `Reset` | unaffected; they name no relation |
| `CreateFunction`, `DropFunction` | refused by **DataFusion** while planning (`Qualified functions are not supported`, `datafusion-sql`'s `statement.rs:1390`/`:1484`). No second fence of ours |

A `postgres://…` URL in a `LOCATION` is its own rule: it splits like any remote location and
lands on the membership refusal, naming a connection the project does not have.

### Tests

- `engine::ddl::tests` — the fourteen-kind checklist against a fake catalog
  (`providers::fake_database`, whose doc says what it can and cannot stand in for), plus the
  bare-name collision from both sides and the `LOCATION` refusal.
- `engine::sql::validate` — the reserved namespace is the workspace catalog's, both directions.
- `engine::catalog::cross_source_tests` — qualified recording, the dependent-views split, and
  `view_error`'s one diagnosis and two declines.
- `engine::remote_catalog_tests` — `database_catalogs` and `describe_remote`.
- `strata-agent` — `describe_result` over `Described::Remote`, `tables_result`'s `databases`,
  and the fallback not swallowing the host's not-found.
- `tests/postgres_federation.rs` — two new phases against the real server: `statement_policy`
  (the refusals, and the *same names still reading*, which is the half a fake catalog cannot
  assert) and `cross_source_views` (replay after the connection, the qualified dep, the local
  drop naming the view, and a relation dropped server-side settling `Failed` with the
  connection and the refresh named).

### Left for other tasks

- **DB-05's Forget match** over `ViewInfo::remote_deps` — the data is there and the field is
  named in that task's step 5; the match is the tree's, not the engine's, and §5's rule says the
  capability arrives with the task that owns it.

## Files

`crates/strata-engine/src/ddl/{mod,tables,views,external}.rs` ·
`crates/strata-engine/src/{providers,query,catalog,db,mod}.rs` ·
`crates/strata-engine/src/sql/validate.rs` ·
`crates/strata-engine/tests/postgres_federation.rs` ·
`crates/strata-agent/src/{host,tools,wire,describe}.rs` ·
`crates/strata-freya/src/apps/project/state/{project,agent}.rs` ·
`docs/STATEMENTS_SPEC.md`, `docs/AGENT_ACCESS_SPEC.md`.
