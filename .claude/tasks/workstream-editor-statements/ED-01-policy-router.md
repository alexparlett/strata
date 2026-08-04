# ED-01 · Policy router: `classify(stmt, Capability)` + `Verdict`

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** E5 · **Depends on:** —

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
  `StmtKind { CreateTable, Ctas, Insert, DropTable, CreateView, DropView, Copy, Set, Reset,
  Prepare, Deallocate, CreateFunction, DropFunction }`, and
  `classify(stmt: &DFStatement, cap: Capability) -> Verdict` replacing `policy_block`'s match.
- **`Capability::Agent` returns exactly today's answers** — every non-query a `Refuse` with the
  same `Blocked` variant and the same rendered message. `policy_verdicts` becomes a thin wrapper
  over `classify(_, Agent)` filtered to refusals; its signature and the agent gate's call site do
  not change.
- **Fail closed, default deny**: parse failure stays the caller-side `Err`; the sqlparser
  wildcard stays `Refuse(Unsupported)`; the DFParser five-variant match stays wildcard-free (a
  new DF variant must be a compile error).
- `Blocked` reshaped: keep `CreateExternalTable`, `CreateDatabase`, non-table/view `Drop`,
  `Unsupported`; add `InsertExternal`, `InsertOverwrite`, `SetOwned`, `SetRuntime`, `SetFormat`,
  `PrepareNonQuery`, `ReservedName` (a `__snap_`-prefixed reference in an intercepted statement).
  Delete or reword every message that pointed at a surface which now accepts the statement.
  Message register: terse IDE sentences, single-quoted identifiers.
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
