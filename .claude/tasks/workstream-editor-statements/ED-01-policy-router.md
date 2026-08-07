# ED-01 · Policy router: `classify(stmt, Capability)` + `Verdict`

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** —

## As built

`crates/strata-core/src/engine/sql/validate.rs`. `classify(stmt, Capability) -> Verdict` replaces
`policy_block`; `policy_verdicts` is a thin filter over `classify(_, Agent)` and neither its
signature nor the agent gate's call site moved. `strata-agent` builds and tests unchanged.

Decisions worth not re-deriving:

- **The capability axis is a column of one match arm, not a second function.** `classify_form`
  returns `(Verdict, Option<Blocked>)` — the editor's answer and the agent's refusal (`None` = the
  read-only pass, so "the agent never intercepts" is a type and not a convention). An arm cannot
  answer one surface and forget the other. `classify` then applies the reserved-name check to the
  editor column only.
- **Two forms diverge on purpose, and the divergence is at the arm.** `INSERT OVERWRITE` refuses as
  `InsertOverwrite` for the editor and as `Insert` for the agent (today's answer); `EXECUTE` is
  `Verdict::Query` for the editor and stays `Unsupported` for an agent that cannot `PREPARE`.
  Deriving the agent column from the editor's verdict would have silently changed both.
- **`PrepareNonQuery` is decided at classification**, not only at plan time: a `PREPARE` body is
  right there in the parse, so the fence is pure. The read-path `SQLOptions` at dispatch stays as
  defence in depth (`verify_plan` cannot see through the later `EXECUTE`).
- **`InsertExternal` / `SetOwned` / `SetRuntime` / `SetFormat` have no producer yet** — they are the
  vocabulary for the dispatcher's context-dependent refusals (ED-05, ED-08), and they carry no name
  payload: `Blocked` stays `Copy`. If ED-05 wants "'events' is an external table" it adds the
  payload then, with a producer in the same change. Likewise the unsupported-clause refusals
  (constraints, `TEMPORARY`, external-table data-column lists, bad `OPTIONS` keys) belong to the
  owning arm's task — they need more than the statement's shape to word well.
- **Reserved names**: `engine::query::SNAPSHOT_PREFIX` is the one constant (spec §5's move, done
  early because ED-01 needs it). sqlparser's `visit_relations` covers the reads and the sqlparser
  targets upstream annotates (`CREATE TABLE`'s name, `INSERT`'s), but `CREATE VIEW`'s name and
  `DROP`'s name list carry **no** `visit_relation` annotation, and DataFusion's own extension
  statements (`CREATE EXTERNAL TABLE`, `COPY`) are outside the visitor entirely — those targets are
  named explicitly in `names_reserved` rather than trusted to it. A plain query may still read
  `__snap_N`; only intercepted forms are gated.
- **`SET` is one `StmtKind` for every sqlparser `Set` variant** (`SET ROLE`, `SET NAMES` included).
  Classification stays a pure function of the form; ED-08 refuses the nonsense variants at dispatch.
- **An `Intercept` falls through to tiers 3 and 4**, it does not `continue`. That is what gives
  typed DDL its name-resolution squiggles for free, and `validation_never_mutates_the_session` now
  pins the stronger claim: the dry-plan reaches `CREATE VIEW` / CTAS / `DROP TABLE`, reports
  nothing, and creates nothing.
- **`BLOCKED_KEYWORDS` is now words that appear only in refused forms.** `CREATE` leads
  `CREATE TABLE` and `CREATE EXTERNAL TABLE` as well as `CREATE DATABASE`, so the refusal there is
  carried by `DATABASE`/`SCHEMA` alone. `CREATE`/`DROP`/`INSERT`/`INTO`/`COPY`/`SET`/`RESET`/
  `TABLE`/`VIEW`/`EXTERNAL`/`REPLACE`/`STORED` left the list; most were never in sqlparser's
  `ALL_KEYWORDS` filter path anyway, but `EXTERNAL` and `STORED` were (and both are needed, for
  `CREATE EXTERNAL TABLE` and `COPY … STORED AS`).

**Known interim state:** between ED-01 and ED-02 an intercepted statement draws no squiggle and
then fails at Run — `Engine::query`'s `SQLOptions::with_allow_ddl(false)` refuses it with
DataFusion's own wording. ED-02's `Engine::run` is what closes that.

## Goal

Grow the managed-DDL predicate into the statement router: one classification, a capability axis,
a three-way verdict. Pure `strata-core`; no dispatch changes yet (ED-02 consumes the verdict).
The agent surface's answers do not change by one byte.

## Current state

- `policy_block(stmt: &DFStatement) -> Option<Blocked>` — `crates/strata-core/src/engine/sql/validate.rs:343`.
  Parsed-statement match, default-deny wildcard. Consumed by `validate()`'s tier-2 diagnostics
  (`validate.rs:148`) and by `policy_verdicts` (`validate.rs:407`), which `Engine::policy_verdicts`
  (`engine/mod.rs:486`) exposes to the agent gate (`crates/strata-agent/src/tools.rs:789`).
- `Blocked` (`validate.rs:278`) + `Blocked::editor_message` (`validate.rs:301`) carry the refusal
  classifications and the editor's exact wording; parity pinned by
  `the_gate_and_the_editor_refuse_with_the_same_words` (`validate.rs:1222`).

## What to build

Per `docs/STATEMENTS_SPEC.md` §4:

- `Capability { Editor, Agent }`, `Verdict { Query, Intercept(StmtKind), Refuse(Blocked) }`,
  `StmtKind { CreateExternalTable, CreateTable, Ctas, Insert, DropTable, CreateView, DropView,
  Copy, Set, Reset, Prepare, Deallocate, CreateFunction, DropFunction }`, and
  `classify(stmt: &DFStatement, cap: Capability) -> Verdict` replacing `policy_block`'s match.
- **`Capability::Agent` returns exactly today's answers** — every non-query a `Refuse` with the
  same `Blocked` variant and the same rendered message. `policy_verdicts` becomes a thin wrapper
  over `classify(_, Agent)` filtered to refusals; its signature and the agent gate's call site do
  not change.
- **Fail closed, default deny**: parse failure stays the caller-side `Err`; the sqlparser
  wildcard stays `Refuse(Unsupported)`; the DFParser five-variant match stays wildcard-free (a
  new DF variant must be a compile error).
- **The editor's refusal set shrinks to the short list in spec §4; `Blocked`'s existing variants
  stay defined as the agent path's error messages.** `Capability::Agent` still refuses
  `CREATE EXTERNAL TABLE`/`CREATE TABLE`/`INSERT`/`CREATE VIEW`/`DROP VIEW`/`DROP`/`COPY`/`SET`/
  `RESET` with today's exact variant and words, and `strata-agent`'s tests name
  `Blocked::CreateTable`/`Insert`/`CreateDatabase` directly
  (`crates/strata-agent/src/error.rs:145`, `:159`, `tools.rs:1762`), so a deleted variant is a
  compile break. On the Editor path those variants are unreachable — every one of those
  statements classifies `Intercept` and runs. Add `InsertExternal`, `InsertOverwrite`,
  `SetOwned`, `SetRuntime`, `SetFormat`,
  `PrepareNonQuery`, `ReservedName` — the last for a `__snap_`-prefixed identifier anywhere in
  an intercepted statement, **target names included** (`CREATE TABLE __snap_2` / CTAS /
  `CREATE VIEW __snap_2` / `INSERT`/`DROP` onto the prefix must refuse before they can collide
  with a live snapshot registration — spec §4, reserved names). Message register: terse IDE
  sentences, single-quoted identifiers.
- Editor diagnostics tier 2 consumes `classify(_, Editor)` — an `Intercept` verdict produces no
  squiggle; a `Refuse` produces the message as today.
- Note: some refusals need context the bare statement lacks (INSERT target origin, SET key
  class). Shape those as `StmtKind` data the dispatcher (ED-02+) refines — classification stays a
  pure function of the parsed statement; context-dependent refusal happens at dispatch with the
  same `Blocked` vocabulary, so wording still has one home.

## Acceptance

- Per-capability parity matrix test: for every statement form, `classify(_, Agent)` equals
  today's `policy_block` answer (message-identical), and a pin that Agent refuses everything
  Editor intercepts.
- `runnable_statements_get_no_verdict`, `a_multi_statement_input_is_judged_per_statement`,
  `the_gate_fails_closed_on_input_it_cannot_judge`, `validation_never_mutates_the_session`
  carried over green.
- `strata-agent` builds unchanged; its policy tests (AA-03's "refused with the editor's own
  message") stay green.
- `policy_and_completion_agree` updated: the completion pool's `BLOCKED_KEYWORDS`
  (`engine/sql/complete/vocabulary.rs`) shrinks to the still-refused editor forms.

## Verification

`cargo test -p strata-core -p strata-agent`; full `cargo test --workspace --locked` on a Mac
build.
