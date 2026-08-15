# DB-10 · Remote DML: INSERT and CTAS into a database connection

**Workstream:** Database connections · **Status:** ⬜ · **Depends on:** DB-02 (DB-03's policy is
what this task relaxes)

## Goal

`INSERT INTO pg.public.events SELECT … FROM local_parquet` loads a local result into Postgres, and
`CREATE TABLE pg.public.report AS SELECT …` materializes any result — local, remote, or a
cross-source join — as a real server table. Strata becomes a load path *into* a database, not only
a reader of one. Writes are a **per-connection opt-in** (a new `read_only` toggle on `PgStore`,
default read-only), so every existing connection keeps DB-03's behavior until someone flips it.

This is the first half of write-back; DB-11 (statements the server runs — the DDL, plus
`UPDATE`/`DELETE`) is the second and depends on this task's gate and listing refresh. Statements
DataFusion can *plan* go here; statements only the server can run go there.

## Current state (verified 2026-08-15)

- **DB-03's refusal is the thing being relaxed.** Every DDL arm resolves its target through
  `ddl::bare_name` (`ddl/mod.rs`), which answers a bare workspace name or refuses; a
  remote-qualified name gets `in_database`'s one sentence ("… Strata reads remote tables; it does
  not create, drop or write them"). `INSERT` (`ddl/tables.rs::insert`) plans first, then reaches
  `bare_name` **before** `Engine::is_internal` — ownership is not a question to ask about a remote
  relation. That ordering survives this task; what changes is that a remote target can now be an
  arm's own branch instead of a refusal.
- **The crate already has the write mechanism** (verified in
  `datafusion-table-providers-postgres` 0.13.0 source):
  - `PostgresTableFactory::read_write_table_provider` wraps the federated read provider in a
    `PostgresTableWriter` whose `insert_into` supports `InsertOp::Append` and `Overwrite`, an
    optional `OnConflict`, and runs the whole insert in **one transaction** on one pooled
    connection — an interrupted insert rolls back, nothing half-lands.
  - `CreateTableBuilder` (`arrow_sql_gen::statement`, plus the postgres ext) renders an Arrow
    schema as a server `CREATE TABLE` — the same builder the crate's own
    `PostgresTableProviderFactory::create` uses. Remote CTAS needs no SQL unparse: derive the
    input's schema, create the table from it, insert the batches.
- **The trap, so nobody re-learns it:** `PostgresTableWriter` *wraps* the federated read provider
  — its `scan` delegates, but the provider node in a plan is the writer, not the
  `FederatedTableProviderAdaptor` the federation rule's downcast walk looks for. Serving writers
  from the catalog's schema provider would silently forfeit pushdown on **every read** — exactly
  the failure the workstream's own-provider decision exists to prevent. The catalog keeps serving
  read providers; a write provider is resolved by the *arm*, after policy, and lives nowhere.
- `PgStore` (`strata-model/src/connection.rs`) has `catalog`, `user`, `sslmode`, `sslrootcert`,
  `password`, `schemas` — no write flag. Its `Deserialize` is hand-written for `schemas`' default;
  the new field joins that impl.
- `engine::db` holds the seams: `Live { _pool, catalog, listing }`, `build_pool`, `enumerate`,
  `PostgresTableFactory` per connection, providers cached per relation. The listing is the
  **connect-time enumeration**, shared as `Arc<Listing>` with the catalog provider — today nothing
  ever replaces it short of a re-connect.
- **DB-09 left a placeholder refusal for this task to turn into a rewrite** (decided by Alex,
  2026-08-15). A *bare* `INSERT` target that only a database connection has is refused today by
  `sql::qualify` (`sql/qualify.rs`, `Pass::write_target`) with `ddl::in_database`'s own sentence,
  before the statement plans — because with no remote write path at all, "table
  'strata.public.orders' not found" reads as a contradiction after `SELECT * FROM orders` works.
  **That refusal is not the rule; it is what the rule looks like while writing is impossible.**
  The rule is that **a write target resolves exactly as a read does**: when this task makes a
  connection writable, `Pass::write_target` stops refusing and simply rewrites, so
  `INSERT INTO orders` dispatches to `pg.public.orders` the same way `SELECT * FROM orders` reads
  it. Asked for directly: *"I want the write to dispatch just like read does."*
  Three things make that safe without a second gate of ours, and they are the argument to keep in
  view rather than re-derive: a connection is **read-only by default** and the user opted this one
  in; **ambiguity still refuses by name**, so a write never picks between two servers; and the arm
  is reached with a qualified name, so `ddl::bare_name` and this task's read-only refusal answer
  identically whether or not the qualifier was typed — one funnel, no "did the user mean it"
  branch. The pass is then simpler, not more complex: `write_target` and `Refusal::remote_target`
  both disappear into `resolve`, and the position list becomes "everything but a create target".
- **Creation is the one asymmetry, and it stays.** `CREATE TABLE orders` names a relation that
  does not exist yet, so there is nothing to resolve *to* — and if the connection happens to have
  an `orders`, resolving would turn a plainly local intent into "create it on the server", which
  then fails as already existing. A create target is the workspace's unless qualified; a remote
  `CREATE TABLE pg.public.x` is this task's own branch, reached by typing the qualifier. Same for
  DB-11's `CREATE VIEW`.

## Build

1. **The gate: `PgStore.read_only`, default `true`.** Serde default keeps every stored def
   read-only, so shipping this changes nothing until a connection opts in. One checkbox row in
   the connection editor's Postgres form (DB-04's window — this task owns the row). The refusal
   for a write against a read-only connection is minted **once**, beside `in_database`, and names
   the toggle: the user is one setting away, so the sentence says which one.
2. **The choke point grows a second answer, not a second copy.** Beside `bare_name`, a
   `remote_target(ctx, name) -> Option<RemoteTarget>` (connection catalog in its registered
   spelling, schema, table) for the arms that gain a remote branch. An arm that stays
   workspace-only keeps calling `bare_name` untouched; `in_database`'s wording shrinks to the
   statements that still hold ("Strata does not …" must stop claiming writes are impossible —
   rewrite it to name what *is* refused). DB-11 reuses both.
3. **`INSERT INTO pg.schema.t`** — in `tables::insert`: after planning, if the target resolves
   remote and the connection is writable, verify the plan under the same `SQLOptions`, build the
   write provider through a new `db.rs` seam (`PostgresTableWriter` over the cached read
   provider), physical-plan the DML's input, and drive `insert_into(Append)`. The input plan is
   an ordinary query — local scans, federated remote scans, and cross-source joins all already
   work — so `INSERT INTO pg.t SELECT … FROM parquet_table` is the same machinery as any query
   feeding a sink. `INSERT OVERWRITE` keeps `Blocked::InsertOverwrite` (the writer could do it —
   transactional delete-all-then-insert — but a statement that silently empties a server table is
   not v1). Report: the sink's row count, in the local arm's own wording.
4. **`CREATE TABLE pg.schema.t AS SELECT …`** — in `tables::create`'s CTAS half: plan the input,
   derive its Arrow schema, create the server table via `CreateTableBuilder`, then the same
   writer insert. On insert failure, drop the just-created table best-effort and report the
   insert's error — never leave a schema-only husk under a name the user thinks holds data.
   A plain `CREATE TABLE pg.t (…)` (column list, no body) is **DB-11's** — its types are the
   server's vocabulary (`jsonb`, `serial`), which only the server should judge.
5. **A settled remote write updates what Strata says the server holds.** An `INSERT` changes no
   listing. A CTAS does: re-run `enumerate` over the existing pool (one round trip, same read as
   connect) and swap the connection's listing in place — the `Arc<Listing>` is shared with the
   catalog provider, so the slot it is read from must become swappable (small `db.rs` surgery;
   never a disconnect/re-connect, which drops the pool mid-session). The statement's
   `StoreEffect` bumps the catalog epoch — remote relations have no store rows, so this is the
   `FunctionsChanged` shape (a new variant; the fold's only job is the epoch) — and the tree,
   completion and `describe_remote` see the new table without a manual ↻.
6. **The agent stays read-only, verified again** — its refusals are at `classify` under
   `Capability::Agent`, which none of this touches; assert it the way DB-03 did.
7. **No new confirm.** A typed statement is the intent — the same terms as a typed local
   `DROP TABLE`, where the confirm belongs to the pane's gesture, not the router. The read-only
   default is the belt.

## Acceptance

- With a writable connection: `INSERT INTO pg.public.t VALUES …`, `INSERT … SELECT` from a local
  table, and from a remote one, all land and report the row count; the rows are on the server
  (read back through a fresh query).
- `CREATE TABLE pg.public.t AS SELECT` from a cross-source join lands; the new table shows in the
  tree and completion **without** a ↻; a CTAS whose insert fails leaves no table behind.
- With the toggle off (the default): every write is refused by the new sentence naming the
  toggle; existing project files deserialize read-only.
- `INSERT OVERWRITE` is still refused; the workspace arms (`bare_name` callers) are untouched —
  the DB-03 fourteen-kind table's other rows still hold, and `docs/STATEMENTS_SPEC.md` §4 is
  updated to the new answers.
- Reads lose nothing: the federation integration phases still pass unchanged (the
  writer-wrapping trap, asserted by the existing pushdown tests staying green).
- `Capability::Agent` verified unchanged.
- Driven against the real container in `tests/postgres_federation.rs` — a fake catalog cannot
  take an insert.

## Files

`crates/strata-engine/src/ddl/{mod,tables}.rs` · `crates/strata-engine/src/db.rs` ·
`crates/strata-model/src/connection.rs` · the connection editor's Postgres form (DB-04's window) ·
`crates/strata-engine/tests/postgres_federation.rs` · `docs/STATEMENTS_SPEC.md`,
`docs/CONNECTIONS_SPEC.md`, `docs/reference/INVARIANTS.md` + AGENTS.md §2 (the "read-only in v1"
sentences — rewrite to lead with what now works, the toggle and the still-refused kinds as the
subordinate clause).
