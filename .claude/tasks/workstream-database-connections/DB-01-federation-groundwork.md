# DB-01 · Federation groundwork in `build_context`

**Workstream:** Database connections · **Status:** ✅ (2026-08-13) · **Depends on:** —

## Goal

Install `datafusion-federation`'s optimizer rule and query planner into the engine's session
state, prove the whole existing suite is untouched, and land nothing else. With no
`FederatedTableProviderAdaptor` registered anywhere the rule is a structural no-op, so this is
the cheapest possible first landing — it de-risks DB-02 by separating "the session state
changed" from "Postgres exists".

## Current state (verified 2026-08-13, corrected in review)

- `build_context` (`crates/strata-core/src/engine/mod.rs:1784`) does **not** use
  `SessionStateBuilder`: it builds via `SessionContext::new_with_config_rt(config, rt)`
  (mod.rs:1813-1822), registers `StrataCatalogProvider` on the *context* (mod.rs:1828,
  displacing the `MemoryCatalogProvider` registered under the same name), runs
  `datafusion_functions_json::register_all(&mut ctx)` and returns
  `ctx.with_function_factory(…)` (mod.rs:1845-1853). `with_optimizer_rules` and
  `with_query_planner` exist only on `SessionStateBuilder` — so installing federation means
  **restructuring `build_context` onto the builder**: `SessionStateBuilder::new()` with the
  config, the runtime env, `with_default_features()`, the rule list and the planner, then
  `SessionContext::new_with_state(…)` — and every step that follows (catalog registration
  order, json UDF registration, the function-factory tail) must be re-verified against the
  builder's defaults. That is the real size of this task; it is still the right first
  landing, but it is a restructure with a regression net, not a two-line diff.
- `datafusion-federation` 0.5.5 pairs with DataFusion 54 (`strata-core` pins `datafusion = "54"`,
  `Cargo.toml:12`). Its `default_optimizer_rules()` takes DataFusion's default rule list and
  inserts `FederationOptimizerRule` **immediately after `scalar_subquery_to_join`** — that
  position is load-bearing (scalar subqueries must be decorrelated before the rule walks the
  plan) and the helper panics if the anchor rule is missing, which cannot happen unless we strip
  default rules. `FederatedQueryPlanner` is `DefaultPhysicalPlanner::with_extension_planners(
  vec![FederatedPlanner])` — the default planner plus one extension planner for the `"Federated"`
  plan node, so swapping it in changes nothing for plans that contain no such node.
- The statement router (`engine::sql::validate::classify` → `Engine::run`) sits entirely in
  front of DataFusion and never sees optimizer output; created functions (`FunctionFactory` +
  `simplify`-substituting UDFs, ED-09) run in earlier optimizer passes than the federation rule.
  Both should be unaffected — that is what this task verifies rather than assumes.

## Build

1. **Dependency** — `datafusion-federation = { version = "0.5.5", features = ["sql"] }` in
   `strata-core/Cargo.toml`, with a why-comment stating the lockstep rule: *federation, the
   table-providers crates, arrow and datafusion cross one type boundary and are bumped together
   with the `datafusion` pin* (the `sql` feature is what DB-02's providers need; declaring it
   here keeps the manifest edit in one task).
2. **`build_context`** — restructure onto `SessionStateBuilder` (the Current state above is
   the checklist of what must survive the move byte-for-byte: config, runtime env, default
   features, the catalog registration displacing the memory catalog, `register_all`, the
   `FunctionFactory`). Then install
   `datafusion_federation::default_optimizer_rules()` and
   `.with_query_planner(Arc::new(FederatedQueryPlanner::new()))`. A comment on the planner line
   records the single-occupancy contract: a future custom `QueryPlanner` must include
   `FederatedPlanner` among its extension planners, not displace it.
3. **Nothing else.** No providers, no model change, no new engine methods.

## Acceptance

- `cargo clippy --workspace --all-targets --locked -- -D warnings` and the full
  `cargo test --workspace` (container runtime attached) are green with **no test edited** — the
  suite is the regression net: query round-trips, EXPLAIN (both plan shapes), the statement
  router's 14 intercepted kinds, created-function macros, snapshots, charts.
- A manual `EXPLAIN SELECT …` over a local table in the running app renders exactly as before
  (no `"Federated"` node, no `VirtualExecutionPlan` — nothing federates without an adaptor).
- `restart_owed` / engine-config behavior untouched (the rule list is not a config key).

## Files

`crates/strata-core/Cargo.toml` · `crates/strata-core/src/engine/mod.rs` (`build_context`).
Reference: `datafusion-federation/src/lib.rs` (`default_optimizer_rules`,
`FederatedQueryPlanner`), its `examples/df-csv-advanced.rs` for the non-sugar builder form.

## As built (2026-08-13)

The restructure was smaller than the Current state feared. `SessionContext::new_with_config_rt`
**is** `SessionStateBuilder::new().with_config(c).with_runtime_env(rt).with_default_features()
.build()` verbatim (DF 54 `execution/context/mod.rs:357-363`), so `build_context` moves onto the
builder by inlining that body and appending the two federation calls; every step after it —
`register_catalog`, `register_all`, the `with_function_factory` tail — is untouched, and the
runtime fallback arm collapses from two `new_with_config_rt` calls to one `rt` binding. Verified
from source rather than assumed: `with_default_features` touches neither the `optimizer` nor the
`query_planner` slot (so call order is free), `Optimizer::default() == Optimizer::new()` (so the
rule list really is DataFusion's own plus one insertion), and `FederatedQueryPlanner` is exactly
`DefaultPhysicalPlanner::with_extension_planners([FederatedPlanner])` against a
`DefaultQueryPlanner` whose planner is `DefaultPhysicalPlanner::default()` — an empty
`extension_planners` vec. One package entered the lock (`datafusion-federation 0.5.5`); its four
dependencies were all already in the graph.

### Correction: the rule is a no-op *for plans DataFusion can execute*, not structurally

`FederationOptimizerRule` walks a plan's **expressions** before it learns anything about
providers, and `scan_expr_recursively` has an unconditional
`Expr::InSubquery(_) => not_impl_err!("InSubquery")` arm (`optimizer/mod.rs:110`). With
`datafusion.optimizer.skip_failed_rules` defaulting to `false`, any plan still carrying an
`Expr::InSubquery` when the rule runs now **errors**, adaptor or no adaptor — so the task file's
"structural no-op" was too strong. `DecorrelatePredicateSubquery` runs immediately before
`ScalarSubqueryToJoin` and therefore immediately before the federation rule, but it only rewrites
`Filter` predicates split on `AND`.

Measured, both ways, on a local-only engine (throwaway probe, not committed — pinning a
third-party error string is not worth a test):

| Shape | Before | After |
|---|---|---|
| `WHERE a IN (SELECT …)` | runs | runs |
| `WHERE a NOT IN (SELECT …)` | runs | runs |
| `WHERE a > 5 OR a IN (SELECT …)` | runs | runs |
| `WHERE a > 5 OR EXISTS (…)` | runs | runs |
| `SELECT a IN (SELECT …) FROM t` (projection position) | **fails** — *Physical plan does not support logical expression InSubquery(…)* | **fails** — *Optimizer rule 'federation_optimizer_rule' failed / This feature is not implemented: InSubquery* |

So **nothing that worked stopped working**: DataFusion's physical planner has no `InSubquery`
support at all, and the federation rule runs strictly in front of physical planning, so any plan
that reaches the rule with one would have failed a few steps later regardless. The only change is
the **wording** of an already-failing query — and it now names an internal rule, which reads oddly
in a project with no database connection. Accepted rather than worked around (there is nothing to
patch short of forking the rule); flagged here because DB-02's integration test is where a
*federated* `IN (subquery)` gets pinned, and because it is the answer if someone later reports
"federation" in an error on a file-only project.

### Acceptance evidence

- `cargo clippy --workspace --all-targets --locked -- -D warnings` → exit 0.
- `cargo test --workspace --locked` → exit 0, **1563 passed / 0 failed** across 24 test binaries,
  container runtime attached so `object_store_minio` ran. **No test edited.**
- `EXPLAIN` and `EXPLAIN ANALYZE` over a local table, through `Engine::query` + `fetch_page`:
  ordinary DataFusion logical/physical plan text, no `Federated` node and no
  `VirtualExecutionPlan`. Checked headlessly (the app has no headless mode, and this is the same
  read the results pane renders) rather than by typing into the window.
- `restart_owed` / engine-config behaviour untouched — the rule list is not a config key and
  nothing in `ENGINE_KEYS` or the override loop moved.

Docs: none. Nothing in `docs/` claimed anything about the optimizer or the query planner, so
nothing became false; ENGINE.md's federation write-up stays DB-02's, where there is a Postgres arm
to explain it against.
