# ED-09 · `StrataFunctionFactory` + swappable function catalog

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** ED-02

## Goal

`CREATE FUNCTION` for SQL-bodied scalar macros, session-scoped, with the function registry's
live-ness kept honest: a created function reaches autocomplete/signature/docs on the next
keystroke. The dispatch and report it rides: `docs/STATEMENTS_SPEC.md` §2.

## Current state

- `functions::snapshot(&ctx)` (`engine/functions.rs:23`) walks `ctx.udfs()/udafs()/udwfs()`
  **once** at `Engine::new` (`engine/mod.rs:302`) into an immutable field — the "function set is
  the live registry" invariant, which runtime-created functions would silently break today.
- Verified (workstream README, DataFusion 54 facts): `FunctionFactory` trait (`create(&self, state, CreateFunction) ->
  RegisterFunction`), installed via `SessionContext::with_function_factory`; without one, CREATE
  FUNCTION errors "Function factory has not been configured"; the body arrives as a parsed
  `Expr`; DROP FUNCTION deregisters across all registries with no factory needed.

## What to build

- `engine/functions.rs` (or a sibling): `StrataFunctionFactory` installed at `build_context`.
  v1 accepts `language` None/SQL with `function_body: Some(expr)` as a `ScalarUDF` substituting
  arguments into the stored body (the upstream `function_factory.rs` pattern); volatility from
  `behavior`. Refusals in the message register: other languages ("LANGUAGE 'python' is not
  supported. Functions are SQL expressions"), aggregate/window/table forms, missing body.
- `Engine::functions()` becomes swappable: `Arc<FunctionCatalog>` behind a lock (callers get the
  `Arc`), plus a `functions_rev: AtomicU64`. After a successful CREATE/DROP FUNCTION the ddl arm
  re-runs `functions::snapshot` and bumps the revision; the completion/docs layer keys on it so
  the pool refreshes on the next derivation. `StoreEffect::FunctionsChanged` tells the app side
  to poke whatever caches the revision doesn't already invalidate — prefer the revision doing
  all the work; the effect exists for the settle's event-log line.
- DROP FUNCTION: native dispatch + re-snapshot + report. Session-scoped: reports say "for this
  session"; nothing persisted (a future `FunctionDef` list in `project.json` replayed by the
  pass is the noted extension — do not scaffold it).

## Acceptance

- `CREATE FUNCTION add_one(x BIGINT) RETURNS BIGINT RETURN x + 1` succeeds; `SELECT add_one(41)`
  returns 42 through the ordinary Run; the function appears in autocomplete with its signature
  and in `SHOW FUNCTIONS`.
- `DROP FUNCTION add_one` removes it from execution and from completion.
- `LANGUAGE python` and window/aggregate forms refuse with exact messages; a CREATE with no body
  refuses.
- Engine restart clears created functions (asserted); the built-in function catalog is unchanged
  when no functions were created (revision stays 0, no re-snapshot cost on the happy path).

## Verification

`cargo test -p strata-core`; run the app: create, complete, run, drop, restart.
