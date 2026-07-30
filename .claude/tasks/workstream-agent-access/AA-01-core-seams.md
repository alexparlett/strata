# AA-01 · Core seams: policy verdict + project registration pass

**Workstream:** Agent access · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** —

## As built (2026-07-30)

1. **The policy gate** is `sql::policy_verdicts(ctx: &SessionContext, sql: &str) ->
   Vec<PolicyRefusal>` (`engine/sql/validate.rs`, exported from `sql`), with
   `PolicyRefusal { statement: Range<usize>, message: String }` — the *statement's* byte range,
   not the leading-keyword span the editor squiggles, because the gate's consumer wants "which
   statement", not an underline. It takes the `SessionContext` so the parse dialect is the
   engine's own (a configurable `datafusion.sql_parser.dialect` must not make the gate and the
   editor read one statement differently). Same `policy_block`, zero copies — and the test
   `the_gate_and_the_editor_refuse_with_the_same_words` makes that executable by asserting the
   gate's message equals `validate`'s diagnostic byte-for-byte per blocked form. **A parse
   failure yields no verdict** (and never hides a neighbour statement's refusal): unparseable
   SQL cannot run either — dispatch fails with the engine's own parse error, the same terminal
   the editor path reaches.

2. **The registration pass** is the new `strata_core::register` module:
   `RegOutcome::{Table, View}` (name + `Result<TableMeta|ViewMeta, String>`),
   `table_spec(root, &TableDef)` (the one copy of the def→spec projection, `resolve_source`
   included), `register_pass(engine, tables, views, settled)` — tables in given order, then
   views by the fixed-point rounds moved verbatim from the hook; a view settles **once**, on
   its final answer — and `register_project(engine, root, &defs, settled)`, the whole-project
   wrapper AA-05 replays. `settled: impl FnMut(&RegOutcome)` is called as each outcome lands
   **and** the collected `Vec` is returned: the sink exists because the Freya hook folds
   `Reg<T>` rows and log entries per answer (rows flip Loading → Ready one by one), which a
   return-only shape would have batched to the end of the pass. The Freya `register_defs`
   now keeps only the store's half: the work-list snapshot (names → defs, via `table_spec`)
   and the per-outcome fold. Event-log wording unchanged.

## Goal
Two exports from `strata-core`, both extractions of logic that already exists and is already
right — no new behaviour:

1. **The managed-DDL policy verdict**, callable outside validation.
2. **The project registration pass** (load defs → register tables → create views), callable
   without the Freya app.

Pure `strata-core`; no UI. Unblocks AA-02 (the gate) and AA-05 (the pass).

## 1. The policy verdict

### Current state
The managed-DDL policy — queries / `EXPLAIN` / `SHOW` / `DESCRIBE` pass, everything else gets a
message naming the owning surface — lives as the **private** `policy_block(stmt) -> Option<String>`
inside `crates/strata-core/src/engine/sql/validate.rs` (~line 275), consumed only by `sql::validate`.
**`Engine::query` does not enforce it**: the editor simply never dispatches what validation
flagged. That discipline doesn't extend to an agent tool layer, which must refuse a blocked
statement *before* dispatch.

### What to build
Export a policy gate from the `sql` module — shape it as the natural public face of what exists,
e.g. `sql::policy_verdicts(sql: &str) -> Vec<PolicyRefusal>` (parse the statements, run
`policy_block` per statement, return the refusals with their spans/messages), or a
`Result<(), PolicyRefusal>` form if that reads better at the call site. Requirements:

- **One predicate, zero copies.** `validate` and the new entry point must consume the *same*
  `policy_block` — the whole point is that the agent gate and the editor diagnostics can never
  disagree. Don't move the messages; don't restate them.
- The messages are already right (IDE register, name the owning surface) — reuse verbatim.
- A parse failure is not a policy pass: decide the shape (likely: unparseable SQL yields no
  verdict here and fails downstream in the engine with its own message, which is the same thing
  the editor path does — verify and document in the doc comment).

### Acceptance
- Unit tests beside `validate`'s existing policy tests: blocked forms (`CREATE EXTERNAL TABLE`,
  CTAS, `INSERT`, `COPY`, `SET`, `CREATE`/`DROP VIEW`, `CREATE DATABASE`) refused through the
  new entry point with the editor's exact messages; `SELECT` / `EXPLAIN` / `SHOW` / `DESCRIBE`
  pass; multi-statement input reports per statement.
- `sql::validate`'s behaviour is byte-for-byte unchanged (its tests already assert the messages).

## 2. The project registration pass

### Current state
The sequence that turns a project folder into a registered engine lives in the Freya app's
project-open hooks (`crates/strata-freya/src/apps/project/state/hooks.rs`, ~318–430):
`project_io::load_defs(&root)` → per table `engine.register(spec)` → per view
`engine.create_view(name, sql)`, folding each outcome into the catalog store (`Reg<T>` rows) and
the event log. A headless host (AA-05) needs the same sequence with no store to fold into.

### What to build
Extract the engine-facing half into `strata-core` (natural home: `project.rs` or a small
`engine`-adjacent fn) — e.g.
`register_project(engine: &Engine, defs: &ProjectDefs) -> Vec<RegOutcome>` where `RegOutcome`
names the def and carries `Ok(TableMeta | ViewMeta) / Err(String)` per entry. The Freya hook
then **consumes this** and keeps only what is genuinely the store's: folding outcomes into
`Reg<T>` rows, epochs, and log entries.

Constraints:

- **Ordering is part of the contract**: tables before views (a view's SQL reads tables), and
  whatever order-within-kind the current hook guarantees — read the hook first and preserve it,
  including error handling (a failed table must not abort the pass; its row is the product).
- The catalog-is-the-store rule is untouched: this returns outcomes, it does not introspect
  DataFusion, and nothing refetches.
- Don't move `load_defs` calls or path resolution semantics (`resolve_source`) — the pass takes
  loaded defs; loading stays the caller's.

### Acceptance
- Unit tests in `strata-core` over a real engine + a temp project: happy path (tables + views
  registered, outcomes in order), a bad table def (its outcome `Err`, the rest proceed), a view
  over a failed table (its outcome is whatever the engine answers — asserted, not assumed).
- The Freya app builds and behaves identically (its hook now calls the extracted pass); the
  project-open event-log entries are unchanged.

## Verification
`cargo test -p strata-core` green; full `cargo test --workspace --locked` green on a Mac build.
