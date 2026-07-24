# P2-23 · Validation engine fitness — multi-error + mid-edit semantics

**Phase:** 2 — Workbench · **Status:** ⬜ · **DEV_TASKS:** E1 (follow-on) · **Depends on:** P2-18 · **Related:** P2-04

## The question
P2-18 validates by **dry-planning through DataFusion** (`statement_to_plan` + `optimize`).
That makes every diagnostic engine-authoritative — the same error a Run would hit — but the
planner is built to answer *"can this execute?"*, not *"what's wrong with this draft?"*. The
mismatch keeps surfacing as point-gaps; decide deliberately whether the planner remains the
whole validator or becomes one layer of it.

## Known gaps (the evidence so far)
1. **Fail-fast, one error per statement.** `SELECT name, product_id FROM events` with both
   columns bad squiggles only `name` — the planner stops at the first resolution failure.
   IDE convention is all-at-once. DataFusion has no error-recovery planning mode.
2. **Premature mid-edit errors.** Columns before FROM ("column not found" against an empty
   schema) — patched in P2-04's wake by suppressing `SchemaError::FieldNotFound` when the
   statement has no `FROM` token (`validate.rs::is_unresolved_column`/`has_from`). Symptom,
   not cure: other half-written shapes likely misreport too (JOIN typed before its ON,
   half-written CTE bodies, GROUP BY while the select list is still moving).
3. **Valid-prefix incompleteness.** The incomplete-trailing-statement suppression
   (`is_incomplete`) predates this task — same class, third instance. Three suppressions is
   a pattern.

## Directions to evaluate (recommendation bias: A)
- **A. Layer, don't replace — native name resolution in front of the planner.** We already
  own the pieces: `lex` + `context.rs` scope analysis (aliases, CTEs, in-scope relations)
  and the `Catalog`. A sqlparser-AST walk (statements usually parse) can resolve **every**
  table/column reference and report **all** unknown names with spans — multi-error, mid-edit
  tolerant by construction (unresolvable scope ⇒ stay quiet, the P2-04 stance). The DF
  dry-plan stays behind it as the authority for types/casts/arity, where fail-fast is
  acceptable because name faults were already caught natively.
  `check_from_targets` (best-effort table check when the parse breaks) is the in-repo
  precedent for exactly this layering.
- **B. Harvest-by-re-planning.** Mask the first error's expression and re-plan for more.
  Slow, fragile, engine-version-coupled — likely reject, document why.
- **C. Full custom semantic analyzer** (own type/coercion checking). Maximum control,
  but drifts from engine truth and re-implements DataFusion — against the engine-model
  principle. Reject unless A proves insufficient.

## Build (assuming A survives contact)
1. `sql::resolve` — AST walk resolving relations (catalog + CTEs + aliases) and column refs
   (incl. qualified `t.c`) per statement scope; emits *all* unknown-name diagnostics with
   byte spans; silent where scope is unknowable mid-edit.
2. `validate()` becomes: lexical lints → policy → **resolve (multi-error)** → dry-plan
   (types/arity; skip or demote its name errors — resolve already owns them).
3. Audit + retire the point-suppressions where the layering makes them redundant; keep the
   suppression tests as behavior specs.
4. Test matrix: multi-bad-column statements, mixed name+type faults, mid-edit shapes
   (no-FROM, dangling JOIN/ON, open CTE), views/CTE resolution parity with the planner.

## Acceptance
- [ ] Every bad name in a statement squiggles, not just the first.
- [ ] Mid-edit shapes (no FROM, half-written JOIN/CTE) produce no premature name errors.
- [ ] Type/cast/arity faults still match what a Run reports (engine-authoritative).
- [ ] The three existing suppression behaviors hold as tests, whether or not their
      implementations survive.

## Review addenda (fresh-eyes audit, 2026-07-24)
Deferred findings the resolver should absorb when it lands:
- `check_from_targets`' local name test uses its own `CLAUSE_KEYWORDS` terminator set
  instead of `lex::is_reserved_in_name_position` — ANALYZE/PARTITION/SET accepted as
  table names where the context analyzer wouldn't.
- `is_incomplete` matches the parser message string (`"found: EOF"`) — no variant
  exists; the resolver replaces the need.
- Two top-level-`;` statement splitters (`context::statement_bounds` vs
  `validate::statement_ranges`) with slightly different trim/filter semantics — unify.

## Freya / references
- `crates/strata-core/src/engine/sql/validate.rs` (dry-plan pass, `check_from_targets`,
  the three suppressions), `context.rs` (scope/alias/CTE analysis), `symbols.rs::Catalog`.
- P2-04's rank-not-filter principle for heuristics under incomplete knowledge.
