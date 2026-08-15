# DB-08 · JSON accessors over remote columns: the pushdown rewrite

**Workstream:** Database connections · **Status:** ✅ **built (2026-08-15)** · **Depends on:** DB-02

## What was built, and the four corrections that came out of building it

`crates/strata-engine/src/db/` is a directory now: `mod.rs` (DB-02's, unchanged bar the
provider construction), `json.rs` (the table, the rewrite, both refusals, unit tests) and
`federate.rs` (the provider one level below `PostgresTableFactory`, plus `PgExecutor`).

1. **The seam is the executor's `ast_analyzer`, not the table's.** Both exist; the executor's runs
   once per federated node where the table's runs once per table *scan*, and the executor is also
   where a remote error is born — so the rewrite and the error wrapper are one type
   (`PgExecutor`, delegating every other `SQLExecutor` method, `compute_context` included, since
   that is what decides whether two relations federate into one statement). It holds
   `Arc<dyn SQLExecutor>` rather than the concrete `SqlTable`, whose generic parameters would tie
   this module to `tokio-postgres`.
2. **`json_contains` is not `?`.** Postgres's `?` is also true for a string *array element* and
   takes no integer index, where the local function is false for both. The faithful spelling is
   `((x -> k) IS NOT NULL)` over the arrow chain — same answer as `jiter_json_find(...).is_some()`
   on every input, JSON `null` included (`'{"a":null}'::jsonb -> 'a'` is `'null'::jsonb`, not SQL
   NULL). It gives up a GIN index `?` could have used; correctness over an index this query never
   had.
3. **`json_length` is unmapped, not "`jsonb_array_length`-based".** It counts object keys as well
   as array elements and is NULL for anything else; `jsonb_array_length` raises on a non-array and
   the object half is set-returning. `json_get_str` is unmapped for the reason the plan already
   suspected: `->>` stringifies objects, arrays, numbers and booleans where it is NULL. So the
   mapped table is two rows, and every other family member states its own difference.
4. **The rewrite shows up in EXPLAIN as `rewritten_executor_sql=`, not inside `base_sql=`.**
   `VirtualExecutionPlan`'s `DisplayAs` prints `base_sql` *before* any analyzer runs and then
   prints each stage's output only where it differs — which is a better assertion than the plan
   asked for, since the test can pin that `base_sql` carries `json_as_text` and the statement that
   leaves carries `->>` and no longer carries the function.

Two more the **server run** found, neither visible to a unit test:

5. **`json_union_to_text` must not be in the table.** It is never something a user typed — it is
   `query::json_unions_as_text`'s own projection over a `json_get` result, added after planning —
   and it sits *above* the `json_get` in the statement, where an outer projection is traversed
   before the subquery under it. In the table, a federated `payload -> 'type'` refused with
   "'json_union_to_text' cannot run on the database connection 'pg'", naming a function the user
   can neither see nor remove. Out of it, the `json_get` beneath refuses in the user's own terms,
   and a union that somehow reached a remote statement without one fails server-side as an
   undefined function, which the wrapper answers.
6. **`->>` has to be parenthesised against a comparison, and that is the parser's doing, not this
   task's.** sqlparser gives every Postgres-style operator `PgOther` precedence (16), which is
   *looser* than `Eq` (20), so `payload ->> 'type' = 'click'` binds as
   `payload ->> ('type' = 'click')` and fails type coercion with "expected string or int, got
   Boolean" — locally and federated alike, before and after this change. The plan's own headline
   query is written that way and does not parse; `(payload ->> 'type') = 'click'` does. Worth its
   own follow-up if it bites: DataFusion's parser dialect is a Settings key, and the PostgreSQL
   dialect gets this right.

A fifth, from the review pass: **the remote-error wrapper keys on `SQLSTATE: 42883`**, which
`datafusion-table-providers-postgres` renders into every server error it hands back, not on
Postgres's prose. `undefined_function` has at least three wordings — `function … does not exist`,
`operator does not exist: …`, and `could not identify an equality operator for type …`, which a
federated `SELECT DISTINCT` over a `json` column raises — so prose-matching would have missed the
third while firing wherever the words merely co-occur. If the crate ever stops rendering the code
this stops wrapping, which is the safe direction.

Two smaller notes. `json_as_text` → `->>` is faithful **because** the column is `jsonb` on the
server: the text the local function would have run over is Postgres's own rendering of that
`jsonb`, so "the raw slice" and "what `->>` returns" are the same bytes. And `?` cannot be typed
at all under DataFusion's default parser dialect (it tokenizes as a placeholder), so the
integration test writes `json_contains` by name — the name the planner produces from `?` anyway.

**Verified against a real server** (2026-08-15): `postgres_federation.rs`'s `json_pushdown` phase
is green, and so is `cargo test --workspace` entire, both container tests included, plus clippy
`-D warnings`. Corrections 5 and 6 above are what that run found — a unit test could not have,
since both are about the statement DataFusion actually hands the executor.

## Goal

`SELECT * FROM pg.public.events WHERE payload->>'type' = 'click'` works, and works *on the
server*. The accessor family Strata plans through `datafusion-functions-json` is rewritten,
at the federation seam, into Postgres's own operator syntax in the SQL we send — and any
family member with no server-side spelling fails with a message naming the workaround, not a
remote "function does not exist".

## The gap (verified from both crates' source, 2026-08-13)

- DataFusion plans the operators via `datafusion-functions-json` 0.54's rewrite
  (rewrite.rs:122-127, verified at the pinned version): `->` → **`json_get`**, `->>` →
  **`json_as_text`** (NOT `json_get_str`, which only a literal user call produces), `?` →
  **`json_contains`**. Locally they run over `Utf8` JSON text — including the text a remote
  `jsonb` column arrives as under DB-02's `UnsupportedTypeAction::String`.
- Under federation, the largest single-provider subtree is unparsed to remote SQL. A UDF
  call unparsers **by name**: `json_get(payload, 'type')` reaches Postgres, which has no
  such function → execute-time remote error. Federation has **no per-expression fallback**
  — the subplan is not re-planned locally. (Without federation the same expressions were
  merely `Unsupported` filters, re-applied locally after streaming — federation makes this
  *worse* for the JSON case, which is why the fix belongs to this workstream.)
- The symmetry that makes the fix right: server-side the column really is `jsonb`, and
  Postgres natively speaks the operators the user typed. `payload ->> 'type'` is better SQL
  there than anything we could emulate.
- The seams: `datafusion-federation`'s `SQLExecutor` offers `logical_optimizer` /
  `ast_analyzer` hooks and its `SQLTable` a per-table query rewriter; the table-providers
  `SqlTable` leaves all of them at their `None` defaults for Postgres
  (**datafusion-federation#129** — not table-providers#129, which is an unrelated
  dependency PR — is the open issue asking for exactly this pattern). We already construct
  providers ourselves (DB-02's `DbSchemaProvider` builds through the factory) — this task
  may move that construction one level down (`SqlTable::new` + `with_dialect` +
  `create_federated_table_provider` are public) so a rewriter can ride it, **under three
  constraints that keep DB-02's decision intact rather than re-litigating it**: the
  Postgres dialect, the federation wrapper and the per-table provider cache survive the
  move byte-for-byte, DB-02's `DbSchemaProvider` remains the one construction site, and the
  workstream README's provider-construction decision is updated in the same change to name
  the new level.

## Build

1. **The mapping, in one table** (`engine::db`, beside the provider construction): the
   `functions-json` family → Postgres operator spellings, keyed by the names the planner
   **actually produces** — `json_as_text(x, k…)` → `(x ->> k)` / `#>>` path form (this is
   what `->>` plans to; the headline query lives or dies on this entry),
   `json_contains(x, k)` → `(x ? k)`, `json_length` → `jsonb_array_length`-based spelling
   where one exists, and `json_get_str`-by-name → `->>` only if its
   NULL-for-non-string semantics are preserved faithfully (Postgres `->>` stringifies
   objects/arrays — if not faithful, unmapped). **`json_get` / bare `->` is unmapped by
   design**: it returns `JsonUnion`'s Arrow *union* type, which no Postgres expression can
   produce (the app special-cases that type locally — `json_unions_as_text`,
   engine/query.rs:439-455) — so a federated `->` refuses, naming `->>` as the spelling
   that pushes down. Each mapped entry states the return-type semantics it preserves
   against the plan's declared Arrow schema; a family member with no faithful spelling is
   **not mapped** — refusal, never a lossy approximation.
2. **The rewrite hook**: an AST-level pass over the unparsed statement (the `ast_analyzer`
   seam, or the query-rewriter seam if that lands closer to our construction) replacing
   mapped function calls with the operator syntax. Installed only on Postgres providers —
   the table is per-dialect by construction.
3. **The legible refusal — one mint, two callers**: `unmapped_refusal(fn_name, conn)` is
   built once beside the mapping table (which is its only source of "mapped"), naming the
   function, the connection, and the workaround (materialize locally first — CTAS the
   remote read into an internal table, then query that; for `->`, naming `->>`). The
   pre-dispatch detector calls it where detection is possible; the remote-error wrapper
   calls **the same function** where it is not — two paths, one sentence, the
   `ddl::drop_intent` precedent. Non-JSON DF-only UDFs (a created macro that survived
   `simplify`, an arrow builtin) get the same wrapper with the generic wording — the table
   never claims to enumerate them, so their refusal does not say "unmapped", it says the
   server lacks the function.
4. **Tests** (phases in `postgres_federation.rs`): the headline query works and its EXPLAIN
   shows `->>` inside `base_sql=` (the rewrite really federated); a chained-path access
   works; `json_contains` works; a federated bare `->` refuses naming `->>`; an unmapped
   function produces the named refusal, not a raw SQLSTATE; the same accessors over a
   **local** JSON table are byte-identical in behavior to before (the rewrite must be
   reachable only from the remote SQL path); and **DB-02's pinned failure phase flips** —
   the assertion DB-02 planted ("the same accessor federated fails") is rewritten here to
   assert success, which is this task's definition of done.

## Acceptance

- The four test phases above, green in CI's container job.
- No change to local JSON behavior anywhere (existing WJ tests untouched).
- The mapping table is the single source — no second copy in the error path (the refusal
  enumerates "unmapped" by asking the same table).
- `docs/CONNECTIONS_SPEC.md`'s database section documents what pushes down; the README's
  known-risks entry for this closes.

## Files

`crates/strata-engine/src/db.rs` (the table + hook install; provider construction if
it moves down a level) · `crates/strata-engine/tests/postgres_federation.rs` ·
`docs/CONNECTIONS_SPEC.md`.
