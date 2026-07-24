# P2-23 · Validation engine fitness — multi-error + mid-edit semantics

**Phase:** 2 — Workbench · **Status:** ✅ · **DEV_TASKS:** E1 (follow-on) · **Depends on:** P2-18 · **Related:** P2-04

> **As built (2026-07-24):** Direction A. New `sql::resolve` — an AST walk over the parsed
> statement resolving every table/column reference (catalog + CTEs + aliases + derived
> tables + correlated scopes), multi-error with byte spans, silent where scope is
> unknowable (`Resolution::complete=false` generalizes the no-FROM grace and replaces
> `has_from`). Dry-plan stays behind it: skipped when the resolver found name faults;
> its `FieldNotFound` suppressed only when the walk was incomplete. All three review
> addenda absorbed: splitters unified into `lex::split_statements`/`statement_at`,
> `check_from_targets` aligned to `lex::is_reserved_in_name_position`, `is_incomplete`
> hardened to a positional test (parser choked past the last written token).
>
> **Sweep hardening (same day):** `tests/sql_validation_sweep.rs` — four property
> batteries over a realistic multi-table catalog (structs, lists, dates, keyword-named
> tables, views): ~85 valid queries must stay clean (each guarded to genuinely plan),
> ~30 bad-name cases must produce exactly the expected spans *and* fail the real
> planner (the resolver never invents an error the engine wouldn't), mid-edit drafts
> stay quiet, and every prefix of every valid query validates with well-formed spans.
> The sweep found and fixed: `CompoundFieldAccess` roots (`c.address['zip']`) checked
> as bare columns; the lex-error span overrunning the buffer at EOF; the keyword-typo
> lint second-guessing legitimate aliases (`FROM orders od`) and qualified refs
> (`od.amount`); non-recursive CTEs shadowing the table they read
> (`WITH t AS (SELECT … FROM t)`). Industry-alignment audit vs DataGrip/DBeaver/
> mssql/sqlfluff/BigQuery: policy matches IDE convention (errors for unresolved names
> with authoritative catalog, all-per-statement, token spans, engine-truth types);
> one divergence found — Strata gated Run on errors, industry is unanimously
> advisory-only — resolved by **removing the Run gate** (Alex's call): diagnostics
> advise, the engine decides. `SessionState::blocking_errors` + `diagnostics_rev`
> deleted; Run disables only for a blank buffer or an in-flight run; safety holds
> because `Engine::query` refuses DDL/DML/statements via `SQLOptions` regardless.

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
