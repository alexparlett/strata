# ED-11 · Completion for the statements the editor now runs

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** ED-08 (ideally
ED-09 and ED-10 too, so the lead list lands once)

## Goal

The editor has spent this workstream learning to **run** statements. Completion has not moved with
it: the offer is still the query/inspection vocabulary S7 shipped, so a user who types `SET ` or
`CREATE TABLE ` gets column names and clause keywords for a statement that has neither. Close that
gap in one pass, across every statement the router intercepts — rather than a table entry per ED
task, which is how the two encodings would drift.

## Current state

`engine/sql/complete/` (+ `context.rs`, `vocabulary.rs`) resolves against a `Catalog` snapshot the
editor tab rebuilds on catalog change. The model is **clause × role**: `context::analyze_caret`
names the governing `Clause` and whether the caret wants an `Operand`, a `Continuation` or a
`Binding`, and `complete` offers per that pair. `vocabulary.rs` holds the declared tables —
`STATEMENT_KEYWORDS` (the leads offered first at a blank statement), `LADDER`, `CORE_KEYWORDS`,
`BLOCKED_KEYWORDS` — each a named policy.

What has landed of statement completion:

- `Clause::Execute` (`EXECUTE` / `DEALLOCATE`) offers the session's prepared statements from
  `Catalog::prepared`, off `Engine::prepared` (ED-08). Nothing else.
- `BLOCKED_KEYWORDS` is already honest against the router — `policy_and_completion_agree_on_statement_leads`
  asserts that a word leading something the editor runs is never filtered out.

What has **not**:

- `STATEMENT_KEYWORDS` is still `SELECT · WITH · EXPLAIN · EXPLAIN ANALYZE · SHOW · SHOW TABLES ·
  DESCRIBE`. Its own doc comment says "a lead only earns promotion here once the statement behind
  it runs, so each arrives with the ED task that implements it" — and ED-04 through ED-08 each
  shipped without adding theirs. The comment describes an intent nothing has honoured; this task is
  where it becomes true or the comment goes.
- No statement operand position is modelled beyond `Execute`. `SET |`, `CREATE TABLE |`,
  `DROP TABLE |`, `COPY |`, `INSERT INTO |` all fall through to `Clause::Unknown` or `Start` and
  offer expression vocabulary.

## What to build

**1. Statement leads.** Add the intercepted forms the editor runs to `STATEMENT_KEYWORDS`, in a
declared priority: the query leads stay first (they are what a blank tab is usually for), the
statement leads follow. Decide and state the phrase set — `CREATE TABLE`, `CREATE TABLE AS`,
`CREATE VIEW`, `CREATE OR REPLACE VIEW`, `CREATE EXTERNAL TABLE`, `DROP TABLE`, `DROP VIEW`,
`INSERT INTO`, `COPY`, `SET`, `RESET`, `PREPARE`, `EXECUTE`, `DEALLOCATE`, and whatever ED-09 adds
— as `MULTI_WORD` entries where the two-word form is the useful one. Extend
`policy_and_completion_agree_on_statement_leads` so a lead offered here has to classify
`Intercept` or `Query` for `Capability::Editor`: the point of one table is that the offer cannot
promise something Run refuses.

**2. `SET` / `RESET`: the config key, and its value.** This is the piece that needs real work, and
the reason this is a task rather than a table entry.

- A config key is **one dotted name** (`datafusion.execution.batch_size`), and `analyze_caret`'s
  `.` rule reads a dotted chain as a qualified column reference (`Context::Dot(rel)` → the columns
  of relation `rel`). So a `SET` clause has to be read **before** that rule, and the whole chain has
  to become one `partial` with one `replace` span — otherwise accepting `datafusion.execution.batch_size`
  at `SET datafusion.|` appends a second copy of the namespace. That is a change to the caret model,
  narrow but real; it wants its own tests over `SET |`, `SET dat|`, `SET datafusion.|`,
  `SET datafusion.execution.b|`.
- The offer is `config::ENGINE_KEYS`, **minus the three classes the dispatch refuses**
  (`is_owned_key`, `is_restart_key`, `is_display_key`) — the `BLOCKED_KEYWORDS` rule applied to
  keys: offering what Run refuses misleads. Assert that agreement by test, from the predicates
  themselves rather than a second list.
- The **value** position (`SET k = |`) is worth having and comes free from the same table:
  `EngineKey::kind` already names `Kind::Bool` (`true` / `false`) and `Kind::Enum(opts)`. Every
  other kind offers nothing, which is the correct empty offer. Telling the key position from the
  value position means capturing the key the caret is writing a value for — a `CaretAnalysis`
  field, in the shape `comparand` already has.
- Detail column: decide between the key's `default` and a rendering of its `Kind`. `EngineKey::desc`
  is a full sentence and too long for the row.

**3. The DDL operands.** Each is a name position the catalog can answer, and each currently offers
expression vocabulary instead:

| Position | Offer |
|---|---|
| `DROP TABLE \|` | registered tables — and **not** views (`DROP VIEW` is the other statement, and `ddl::tables` says so by name) |
| `DROP VIEW \|` | saved views only, for the mirror reason |
| `INSERT INTO \|` | internal tables only — `Engine::is_internal` is the gate the statement itself uses, so the offer has to come from the same answer rather than from "all tables" |
| `COPY \|` | relations, like a FROM target; `COPY … TO '\|'` is a path and offers nothing |
| `CREATE TABLE \|` / `CREATE VIEW \|` | a name being invented — `Role::Binding`, the empty offer, like `AS` |
| `PREPARE \|` | likewise `Role::Binding` — but **not** the `PREPARE` inside `DEALLOCATE PREPARE p`, which names one that exists (`Clause::Execute`'s operand). The `clause_of` table deliberately leaves `PREPARE` out for exactly this reason; whatever handles it has to keep that distinction |

`INSERT INTO` needs the internal-name set on the `Catalog` snapshot, which the editor tab builds —
so `Engine` grows an enumeration beside `is_internal`, or the store's `TableOrigin` answers it (the
store *is* the catalog, so prefer the store). Decide once and say which in the module doc.

**4. Continuations.** `continuation_keywords` needs arms for the new clauses. Keep them honest:
`DROP TABLE t |` is complete and offers nothing, `CREATE TABLE t |` wants `AS`, `COPY x |` wants
`TO`, `SET k |` wants nothing (a `=` follows, which is punctuation).

## Acceptance

- Every statement the editor runs is offered as a lead at a blank statement, and the lead list is
  kept honest against `classify(_, Capability::Editor)` by test — a lead that Run would refuse
  fails the suite.
- `SET datafusion.exec|` completes to the full key and replaces the whole dotted chain, never
  appending to it; the three refused key classes are absent from the offer; `SET <bool key> = |`
  offers `true` / `false` and `SET <enum key> = |` offers that key's options.
- `DROP TABLE |` offers tables and no views; `DROP VIEW |` the reverse; `INSERT INTO |` offers only
  internal tables; `CREATE TABLE |` and `PREPARE |` offer nothing, while `DEALLOCATE PREPARE |`
  still offers prepared names.
- The existing completion suite is unchanged — every ranking scalpel and the torture sweep still
  pass. A statement clause must not leak vocabulary into a query position.

## Verification

`cargo test -p strata-core`; run the app and type each statement lead through to its operand.

## Why this is one task and not five

Split per statement, the lead table and the "offer what Run accepts" agreement would be edited five
times, and the caret-model change `SET` needs would land under whichever task happened to go first.
One pass, one table, one test that the offer and the router agree — which is the shape
`BLOCKED_KEYWORDS` already has and the reason it has not drifted.
