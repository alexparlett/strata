# AA-01 · Core seams: policy verdict + project registration pass

**Workstream:** Agent access · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** —

## As built (2026-07-30, reshaped by the PR #78 review)

1. **The policy gate** is `sql::policy_verdicts(ctx: &SessionContext, sql: &str) ->
   Result<Vec<PolicyRefusal>, String>` (`engine/sql/validate.rs`, exported from `sql`), plus the
   facade twin `Engine::policy_verdicts(sql)` — the reachable entry point, since `Engine.ctx` is
   private and no consumer crate depends on DataFusion; it spawns onto the engine runtime like
   `Engine::validate`. Review-settled shapes:
   - **Classification, not prose.** `policy_block` returns `Blocked` (an enum of the refused
     forms); the editor's wording is `Blocked::editor_message()`, used verbatim by `validate`'s
     diagnostics. A headless consumer (AA-02) renders the same variant in its own words — over
     stdio there is no Table Config pane to point at. The parity test pins
     `editor_message() == validate`'s diagnostic per blocked form, so the predicate stays one
     copy while the prose stays per-surface.
   - **Fail closed.** `Err` means "could not judge" (the input does not parse under the
     session's own dialect): the caller refuses dispatch and surfaces the parse error. Never an
     empty `Ok` for unjudgeable input, and one broken statement never silently approves its
     neighbours — the pre-review shape returned `[]` for a lex failure, byte-identical to a
     clean pass.
   - **No offset arithmetic.** The input is parsed whole (`DFParserBuilder` with the session's
     dialect + recursion limit, the same resolution `sql_to_statement` performs); a
     `PolicyRefusal` is `{ index, statement (canonical rendering), blocked }`. The editor's
     char-column→byte-offset statement splitting stays where a squiggle tolerates it and a gate
     would not (it mis-splits over non-ASCII).

2. **The registration pass** is the `strata_core::register` module: `RegOutcome::{Table, View}`,
   `table_spec(root, &TableDef)` (the one copy of the def→spec projection — and
   `tests/project_load.rs` now drives `register_project`, so no second copy hides in a test),
   `register_pass(engine, tables, views, settled)` and `register_project(engine, root, &defs,
   settled)`. Review-settled shapes:
   - **One output channel.** `settled: impl FnMut(RegOutcome)` takes each outcome **by value**
     as it lands and nothing is returned: the app folds rows/log entries per answer with no
     clone (moves `meta` straight into the store, as the pre-extraction loop did), and a caller
     that wants the list writes `|o| out.push(o)`.
   - **`register_project` is the cold pass only.** Against an engine already holding these
     views, defs order silently inlines stale plans (`CREATE OR REPLACE` succeeds round one, so
     the fixed-point retry orders nothing). The Kahn sort now lives in core as
     `register::view_order` — `ProjectState::refresh_order` is its store projection — and a
     re-running host orders views with it over the previous pass's answers before calling
     `register_pass`.
   - **Named caller responsibilities** (module doc): the pass is *additive* (removal stays the
     caller's — the app's drop confirm; a replayer diffs shrunken defs and deregisters first),
     and the *registration window* (`Engine::register` deregisters before re-inferring) means a
     host serving validation/queries concurrently must gate them the way the app's
     `CatalogState::Scanning` does, or it answers false transient 'not found'.

   The Freya `register_defs` keeps only the store's half: the work-list snapshot (names → defs,
   via `table_spec`) and the per-outcome fold. Event-log wording unchanged.

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
