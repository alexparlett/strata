# ED-02 · `Engine::run` + statement results

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** ED-01

## Goal

The dispatch spine and the app-side fold: `Engine::run` routes on ED-01's verdict, a non-query
statement returns a typed report the app folds into `ProjectState` exactly the way `save_view`
does today, and the results pane learns to render a statement row. Every later ED task is one
`StmtKind` arm through this spine.

## Current state

- Editor Run: `press_query` (`crates/strata-freya/src/apps/project/views/workbench/editor/actions.rs:33`)
  writes a `QuerySpec` into the tab's request slot; the freya-query subscription
  (`apps/project/query/run_query.rs`) calls `Engine::query` (`engine/mod.rs:510`) → snapshot +
  page 1. The hard stop is the `SQLOptions` triple in `query::materialize` (`engine/query.rs:450`).
- The discrete-mutation fold shape to generalize: `save_view`
  (`editor/actions.rs:254`) — store upsert → `persisted_defs` → engine call → fold outcome →
  `catalog_settled` epoch bump.
- History: `append_history` (`strata-core/src/project.rs:232`), only successful data runs today.

## What to build

Per `docs/STATEMENTS_SPEC.md` §4 + §7:

**Engine (`strata-core`):**
- `RunOutcome { Rows(QueryOutput, RecordBatch), Statement(StatementReport) }`;
  `StatementReport { kind, message, count, elapsed_ms, effect: Option<StoreEffect> }`;
  `StoreEffect { TableUpserted { def, meta }, TableRemoved { name, dependents },
  ViewUpserted { def, meta }, ViewRemoved { name }, RescanTable { name }, FunctionsChanged, None }`
  (final shape settled here; later tasks add variants only if a capability genuinely needs one).
- `Engine::run(ws, tag, sql, page_size) -> Result<RunOutcome, String>`: parse the single
  statement, `classify(_, Editor)`; `Query` → delegate to today's `query()` byte-for-byte
  (only this arm touches the snapshot lifecycle); `Intercept(kind)` → `engine/ddl.rs::execute`
  (new submodule — this task ships the module with the dispatch skeleton and stub refusals
  "not yet implemented" per kind, each later task fills its arm); `Refuse(b)` →
  `Err(b.editor_message())`.
- In-flight bookkeeping: intercepted long-running kinds register an `InFlight` entry so
  `cancel` / `is_running` / the close-confirm flag keep working; cleanup mirrors
  `run_and_snapshot`'s partial-output removal.

**App (`strata-freya`):**
- The Run subscription dispatches `Engine::run`; `RunOutcome::Rows` flows into the existing
  results states unchanged. `RunOutcome::Statement` becomes a new results-pane state: a status
  row (message · count · elapsed), no grid, no snapshot handle — the tab's previous snapshot
  survives (DDL never retires).
- The settle fold, one function shared by every effect: apply the `StoreEffect` to the store on
  the matching `ProjChan` → `persisted_defs` through the persist funnel → `catalog_settled` →
  event-log entry. `RescanTable` requests `ScanScope::Table` instead of touching rows.
- **History amendment**: successful statements append to `history.jsonl` like data runs, same
  `collapse_sql` dedupe and cap. Update the history invariant text (AGENTS.md §2 +
  `docs/reference/INVARIANTS.md`, same bolded lead sentence) in this change, per the upkeep rule
  — and the managed-DDL invariant becomes the router invariant (spec §10, first row).

## Acceptance

- A `SELECT` through `Engine::run` is indistinguishable from today's `Engine::query` (existing
  round-trip tests pass against `run`).
- A refused statement fails the run with the squiggle's exact message; an intercepted-but-stub
  kind fails with its stub message; neither touches the snapshot lifecycle (previous snapshot
  still pages).
- A statement settle folds its effect, persists, bumps the epoch, logs, and appends history;
  a failed statement appends nothing.
- Multi-statement input refuses the batch with a policy message (one statement per Run kept).

## Verification

`cargo test -p strata-core`; run the app (`run-app` skill): Run a SELECT (grid unchanged), run a
still-stubbed `CREATE TABLE` (statement row with stub refusal, previous results intact).

---

## As built

**Engine.** `Engine::run` (`engine/mod.rs`) parses through `sql::classify_one` — a new one-statement
gate over the parse funnel `policy_verdicts` now shares (`validate::parse`, extracted so the two
gates cannot resolve a dialect differently), on the engine runtime like every other parse. `Query`
delegates to `query()` unchanged; `Intercept(kind)` goes to the new `engine/ddl.rs`; `Refuse(b)`
returns `b.editor_message()`. Multi-statement input is `Err("Run executes one statement at a
time")`; a comments-only buffer is `Err("Nothing to run")`.

**In-flight bookkeeping is `Engine::bookkeep`, shared with `explain`** rather than a second copy —
supersede, abort handle, `DispatchGuard`, settle by `dispatch` not `tag`, and `Lifecycle::current`
untouched. `explain` was rewritten onto it, so there is exactly one non-snapshot dispatch
lifecycle. **Every** intercepted kind is bracketed, not a hand-picked "long-running" subset: the
subset would be a table to keep in step, and a `SET` costs one lock acquire. Partial-output
cleanup is per-arm (a CTAS's tmp dir) and belongs to ED-04, not here.

**`StoreEffect` has no `None` variant** — `StatementReport::effect` is already `Option`, and two
ways to say "nothing changed" is a second arm every fold has to remember. `TableRemoved.dependents`
ships as specified and is written by ED-05; the fold reads `name` only, because there is no cascade
(the epoch bump is what tells the user).

**App.** `QueryMode::Run` dispatches `Engine::run`; `QueryOutcome` gains `Statement(StatementReport)`.
The fold is `state/statement.rs` — `use_statement_settle`, mounted in the request keeper beside
history and the log, one `apply` over every `StoreEffect`. A def-mutating arm names a **channel
and a mutation and nothing else**: persisting at the mutation point and bumping the catalog epoch
are `mutated`'s, held once, so a later ED task's arm cannot forget either half. It **owns the
statement's log row**:
`run_event` returns `Option` and answers `None` for a statement, because a message claiming
something durable must not be logged over a failed `project.json` write (the `save_view` lesson).
Results pane: `ResultsState::Statement` + `results/statement.rs` (icon tile · `StmtKind::label` ·
the engine's sentence), status bar `"CREATE TABLE · 12 ms"` off the same label table.

**Not in this change, and flagged:** the spec (§1) settles **no confirm dialog** on a typed `DROP`
— typing the statement and pressing Run *is* the deliberate gesture, where the catalog menu's is a
pointer gesture that needs one. If that is revisited, it lands in ED-05 (`DROP TABLE`) / ED-06
(`DROP VIEW`), not here.

`StmtKind::label()` is new in `validate.rs` — one table behind the stub refusal, the results title
and the status bar. `TableDef` / `ViewDef` gained `Debug` (every neighbour had it) so `StoreEffect`
could derive it; `RunOutcome` deliberately did **not**, since a derived one puts a whole
`RecordBatch` behind a `{:?}`.
