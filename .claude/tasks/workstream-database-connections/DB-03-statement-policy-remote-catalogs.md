# DB-03 · Statement policy over remote catalogs

**Workstream:** Database connections · **Status:** ⬜ · **Depends on:** DB-02

## Goal

Every statement the router intercepts answers sensibly when its target is qualified into a
database connection's catalog: refused **by name**, with the refusal saying what the name is (a
remote table in connection *X*) and that Strata does not manage remote objects. Queries —
including `EXPLAIN`, `PREPARE` of a query, views *reading* remote tables — keep working; the
agent's read-only capability is verified unaffected. This is an audit with teeth, not a new
mechanism: the work is wording + tests over arms that already exist.

## Current state (verified 2026-08-13)

- `classify(stmt, Capability)` operates on the parsed statement and does not resolve names —
  but **the gate below it already exists** (corrected in review): every DDL arm resolves its
  target through `bare_name` (e.g. `ddl/tables.rs:320`), and `ddl/mod.rs:264-277` refuses any
  `TableReference::Full` whose catalog is not `strata` **before any arm's own resolution** —
  `elsewhere(what)`: "Strata has one schema, 'public'. Tables cannot be created elsewhere."
  So the predicted "no such table" miss cannot happen, and this task builds **no resolution
  plumbing**: the work is the *wording* of that one existing refusal — `elsewhere` learns to
  tell a database connection's catalog ("'pg.public.orders' is a table in database connection
  'pg'. Strata reads remote tables; it does not create, drop, or write them") from a
  genuinely unknown catalog, by asking the same catalog list DB-02 registers into. Then the
  audit is: every arm reaches its target through `bare_name` (verify, arm by arm — an arm
  that doesn't is the finding), `INSERT`'s `is_internal` wording names the remote truth,
  `CREATE`/`DROP FUNCTION` can't take qualified names (verify sqlparser agrees), `COPY`'s
  `TO` target is a path (N/A).
- The `__snap_` fence and `ReservedName` are `strata`-catalog concerns; a remote catalog can
  legitimately contain a table named `__snap_x`. **Decided**: the fence scopes to the
  `strata` catalog, and the scoping lives in the one predicate — `is_snapshot_name` grows a
  catalog-aware form (beside `snapshot_name`, its single home) asked by the refusal, the
  hiding rule and both schema providers, so the naming rule, the refusal and the hiding
  cannot drift. Reading a remote `__snap_…` is fine; it means nothing there.
- Views: `ddl::views::create` canonicalizes the definition query — verify a view over
  `pg.public.orders` round-trips (register, reopen project, view re-registers **after**
  connections in the pass, which phase order already guarantees). A view over a *failed*
  connection lands `Reg::Failed` on its own row — confirm the message points at the
  connection.
- **The cross-source view is the load-bearing case** (a view joining a workspace table with
  `pg.public.orders` — a workspace def whose dependencies span sources). The dependency
  machinery (`plan_deps`, `dependent_views`, `ddl::left_invalid`) was built against bare
  names in one namespace; a mixed plan's scans are a bare `TableReference` *and* a qualified
  remote one. What the source actually does (corrected in review): `plan_deps` inserts
  `scan.table_name.table()` — the **bare component, catalog and schema discarded**
  (catalog.rs:785) — and `aliases` holds only `SubqueryAlias` names (catalog.rs:790-792),
  never scans. So today a view over `pg.public.orders` records the dep `"orders"`, which
  **collides with a workspace table of that name**: dropping workspace `orders` wrongly
  names the view as a dependent, and Forget's consequence match finds nothing anywhere. The
  fix is the `tables` insert: record a non-`strata` scan **qualified**
  (`pg.public.orders`), keep workspace scans bare, and update `dependent_views` /
  `left_invalid` comparisons plus DB-05's Forget match (prefix on the connection's catalog)
  accordingly — engine dependency tracking, not UI.
- A remote table vanishing **server-side** raises no event (nothing on our side can observe
  it); the cross-source view fails at the next register pass or validation, the
  reconciliation shape — state that staleness bound where the view's failure message is
  built, so the row says "…not found on 'pg'; refresh the catalog" rather than a bare
  planning error.
- Diagnostics: validation resolves through the live catalog list, so `pg.public.orders`
  resolves once DB-02's provider is registered (cached per table). Verify: no false squiggle
  on a valid remote name; a *stale* remote name (table dropped server-side, cache still warm)
  squiggles only after ↻ — state that staleness bound in the reconciliation's doc rather than
  chasing liveness.
- `SHOW TABLES` / `information_schema` (corrected in review): DataFusion's
  `information_schema` enumerates **every catalog in the list**
  (information_schema.rs:102 in datafusion-catalog 54 — `for catalog_name in
  self.catalog_list.catalog_names()`), so `SHOW TABLES` **changes** the moment DB-02
  registers `pg`: remote relations appear beside workspace ones. With DB-02's connect-time
  listing + `table_type` override this costs zero remote calls; pin the *new* answer as the
  expected one, and note `information_schema.columns` over remote catalogs builds cached
  providers per table (bounded, accepted). Also pin `SHOW TABLES IN pg.public` if the
  dialect reaches it.
- One more refusal to pin: a `postgres://…` URL typed into `CREATE EXTERNAL TABLE …
  LOCATION` splits (`split_remote`) into a URL no connection has and refuses through the
  existing membership wording — assert the message is the membership one, not a panic or a
  bare planner error (DB-02's `url()`-carries-a-path consequence).

## Build

1. Teach the **existing** gate the new sentence: `elsewhere()` (ddl/mod.rs:277) branches on
   whether the qualified catalog is a database connection's, with the refusal minted once
   there — no per-arm helper, no per-arm opt-in, because `bare_name` in front of every arm
   is already the single choke point (the audit in Current state confirms each arm goes
   through it; an arm that doesn't is a finding fixed by routing it through, never by a
   second copy of the check).
2. Tests per arm in the router's existing test style (unit, no container: a fake
   `DbCatalogProvider` registered under a test catalog name is enough — the helper reads the
   catalog list, not the network), plus phases in `postgres_federation.rs` proving the
   load-bearing ones end to end: `DROP TABLE pg.…` refused; `CREATE VIEW v AS SELECT … FROM
   pg.…` works and survives re-open; and the **cross-source view** — created over a local
   table joined to `pg.public.…`, then (a) dropping the local table names it as a dependent,
   (b) its deps carry the qualified remote name, (c) it re-registers on replay after the
   connection, (d) with the remote table renamed away server-side it settles `Failed` with
   the message naming the connection and the refresh.
3. `Capability::Agent`: one test that the agent's `run` still refuses every non-query in its
   original wording when remote names are involved, and that a plain remote read passes
   `policy_verdicts`. Audit the tool vocabulary's *name-answering* tools too:
   `list_tables` reads the store (defs only — correct; remote tables are not defs) but its
   answer should say the database catalogs exist and where to look; `describe_table` should
   resolve a qualified remote name through the engine's cached schema rather than answering
   "not found" for a table the agent can query. Small arms, agent-visible honesty.
4. `docs/STATEMENTS_SPEC.md` gains the remote-catalog column; `docs/COMPLETION_SPEC.md`
   untouched here (**DB-06** owns completion).

## Acceptance

- Every intercepted `StmtKind` has a stated, tested answer for a remote-qualified target —
  the checklist is the 14 kinds, enumerated in the PR description with their answers.
- The `__snap_` decision is recorded in STATEMENTS_SPEC with its reasoning.
- Full check green; no change to any local-table behavior (existing router tests unedited).

## Files

`crates/strata-core/src/engine/ddl/{mod.rs (`elsewhere`/`bare_name`), tables.rs, external.rs,
views.rs, copy.rs, functions.rs}` · `crates/strata-core/src/engine/catalog.rs` (`plan_deps`
qualified recording) · `crates/strata-core/src/engine/sql/validate.rs` (only if a kind needs
re-routing) · `crates/strata-core/tests/postgres_federation.rs` (the end-to-end phases in
Build 2) · `crates/strata-agent/src/…` (the `list_tables`/`describe_table` arms in Build 3) ·
unit tests beside each · `docs/STATEMENTS_SPEC.md`.
