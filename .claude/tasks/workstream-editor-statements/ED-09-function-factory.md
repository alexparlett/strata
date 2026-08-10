# ED-09 · `StrataFunctionFactory` + swappable function catalog

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** ED-02

## What it does

`CREATE FUNCTION` for SQL-bodied scalar macros and the `DROP FUNCTION` that takes one back, both
session-scoped, with the "function set is the live registry" invariant kept honest: a created
function reaches autocomplete, signature help, the docs panel and `SHOW FUNCTIONS` on the next
derivation. Built behaviour: **`docs/STATEMENTS_SPEC.md` §6.6**, and the module doc on
`crates/strata-core/src/engine/ddl/functions.rs`, which carries the reasoning.

Shape, in one line each:

- **`engine/ddl/functions.rs`** — `StrataFunctionFactory` (installed at `build_context`, so the
  headless host runs the statement identically), the two arms, and `Definition::read`, which is the
  **one** judgement of a `CREATE FUNCTION`: the arm calls it for the sentence the user reads, the
  factory calls it to build from, so a form the arm accepts is a form the factory can build.
- **`engine/functions.rs`** — `Functions`: the catalog as a swappable `Arc<FunctionCatalog>` plus
  the folded names this session created, shared by handle for the reason `InternalTables` and
  `SessionScope` are. Re-walked by the arm that moved the registry and by nothing else.
- **`Engine::functions()`** hands out the `Arc`; `Catalog::functions` holds one too, so the
  language service's memoized snapshot stopped deep-copying a thousand symbols per catalog epoch.
- **`ddl::Dispatch`** — the five things an arm can reach of the engine, gathered once in
  `Engine::run`. Introduced here because the parameter list reached eight; it grows by one member
  per capability the workstream lifts.

## Three corrections to the draft, settled while building

**1. DataFusion cannot plan `RETURN x + 1`, which is the acceptance example and the standard SQL.**
The body is planned against an **empty schema** with the argument list supplied as *placeholder*
types (`datafusion-sql/src/statement.rs`, the `CreateFunction` arm), so a bare identifier fails name
resolution outright — "Schema error: No field named x". Its planner accepts positional `$1` and
named `$x` only. `bind_parameters` rewrites the bare form into `$name` **on the parsed statement,
before planning**, so all three spellings land on one planned body of positional placeholders and
`simplify` has one substitution to make. Do not re-derive this as "DataFusion supports named
arguments"; it supports dollar-prefixed ones.

**2. There is no `functions_rev`, because nothing would read it.** The draft asked for an
`AtomicU64` beside the catalog with `StoreEffect::FunctionsChanged` there only for the log line.
Built the other way round: `FunctionsChanged` → the settle's `catalog_settled` → the catalog epoch
is what every consumer already keys on (the editor tab's memoized `Catalog` is rebuilt on it, and
the agent's `list_functions` reads live). A revision counter would have had **zero** readers, which
is the unreferenced pre-work AGENTS.md §5 forbids. "The revision does all the work" is true — the
revision is the catalog epoch, which every other catalog mutation already moves.

**3. Aggregate, window and table forms have nothing to refuse.** `RegisterFunction` has those
variants, but the *factory* chooses which to return and this one only ever returns `Scalar`; SQL has
no `CREATE FUNCTION … AGGREGATE` form for the parser to produce. `RETURNS SETOF` is the nearest
thing and DataFusion refuses it itself, in its own words. The refusals that were needed instead are
the ones the draft did not name, and each exists because the statement would otherwise **succeed as
something else**:

- `AS '<string>'` — `AS` takes a string literal in this dialect family, so `AS 'x + 1'` would create
  a function returning the *text* `x + 1`.
- Every clause DataFusion's planner drops silently (`STRICT`, `SECURITY`, `SET`, `PARALLEL`, …),
  refused off the parsed statement from a destructure with no `..` — `views::definition`'s rule.
- A body reaching outside its arguments (a bare column, a subquery): a hidden dependency on a table
  that nothing persists and no `DROP TABLE` can name.
- **A built-in name, on either statement.** This is the one worth reading the module doc for.
  DataFusion's registry cannot tell a built-in from a session's own function and its `DROP FUNCTION`
  deregisters across *every* registry at once, so `DROP FUNCTION abs` would take the built-in away
  for the rest of the session with nothing able to put it back. `Functions::created` is what makes
  the difference nameable. It is `CREATE OR REPLACE VIEW` over a table name (ED-06) from the other
  side.

## Six defects the reviews found, fixed here

Each is worth reading as a rule rather than a bug. Four of the six are the *same* rule the create
arm already kept — judge the statement where the answer is reachable, off a destructure with no
`..` — applied to a place that had not been given it. The other two are predicates that were right
by accident: one about which registries a drop clears, one about which reading of an identifier
answers which question.

The first four came from the adversarial panel, the last two from the single-pass review after it.

1. **`registered_function` asked three registries; `DROP FUNCTION` clears five.** Scalar, aggregate
   and window are one method call away — table and higher-order are not. `array_filter`,
   `array_transform` and `array_any_match` are registered **only** as higher-order, so the fence
   read them as free names: a session could take one and the matching drop would then destroy the
   built-in, which is the exact loss the fence exists to prevent. Fixed to ask all five. The
   predicate is now "what would the drop clear", never "what happens to be callable" — `range`
   escaping the narrower fence by having a scalar twin was luck.
2. **`DROP FUNCTION a, b` dropped `a` and reported success.** The drop arm trusted DataFusion's
   planner to refuse the forms it does not implement; that is true of a qualified name and false of
   everything else. Its `DropFunction` arm takes `func_desc.first()` with no length check, binds
   `drop_behavior: _`, and never reads a `FunctionDesc`'s argument list, while sqlparser parses the
   comma list in **every** dialect. Fixed with `unsupported_drop_clause`, exhaustive with no `..` —
   the create arm's own rule, which the drop arm had simply not been given.
3. **An aggregate or window body created fine and could never be called.** `sql_to_expr` builds an
   `AggregateFunction` with no aggregate-context requirement, nothing simplifies it away, and the
   physical planner answers `not_impl_err!("… {other:?}")` — a `Debug` dump of the node in the
   results pane. Refused beside the subquery, where the body's other "plans but is not a scalar
   expression" case already was.
4. A comment cited `Definition::bind`, which the same change had renamed to `check`.

Both of the last two are one shape: a refusal **reachable only for an input nobody types**.

5. **The `LANGUAGE` refusal sat after planning.** A body in another language is not SQL, so
   planning it answered about the body — `LANGUAGE python RETURN np_abs(x)` reported "Invalid
   function 'np_abs'" — and the sentence naming the actual problem only ever fired for a body that
   happened to be valid, fully resolvable SQL, which is what the test used. Moved to
   `supported_language`, asked off the parsed statement by `unsupported_clause` **and** off the
   planned one by `Definition::read`: one body, two sides of planning, so the factory stays closed
   to a caller that did not come through the arm.
6. **A quoted argument name was normalized by the body's rule, not the argument list's.**
   `Definition::read` used `param_name` where `bind_parameters` uses `declared_name` — two readings
   of one identifier — so `spaced("My Arg" BIGINT)` rendered as `spaced("my arg")` in completion,
   signature help and `SHOW FUNCTIONS`, quotes kept and case folded, and DataFusion's named-argument
   notation was unusable for it.

## Noted extension, deliberately not scaffolded

A `FunctionDef` list in `project.json`, replayed by the registration pass exactly as a view is.
Nothing persists today and every report says "for this session", which is true by construction: a
restart is a new `Engine`, whose `Functions` is a fresh walk of the built-in registry.

## Verified

`cargo test --workspace` (10 new tests in `engine/ddl/functions.rs`), clippy clean. The suite covers
the acceptance chain end to end — create, run over a constant *and* a column, complete with the
argument's own name, `SHOW FUNCTIONS`, drop, restart — plus every refusal above, the three argument
spellings, name folding, `OR REPLACE`, and that the built-in catalog is byte-identical before a
create and after the matching drop. Two tests exist for the review's findings 1 and 2 specifically,
each ending on the assertion that would have failed: the built-in still runs, and both functions a
multi-name drop named are still callable.

Not run: the app itself. The completion half is asserted through the same `Catalog::build` +
`complete` the editor calls, and the `FunctionsChanged` → `catalog_settled` → memoized-`Catalog`
chain is the one ED-08's `PreparedChanged` already rides, unchanged here.
