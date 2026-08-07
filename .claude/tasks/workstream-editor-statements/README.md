# Workstream — Editor statements (ED)

Lifting the managed-DDL policy into a **full-statement editor**: internal tables persisted under
`.strata/tables/` (`CREATE TABLE` / CTAS, `INSERT`, `DROP TABLE`), typed `CREATE`/`DROP VIEW`,
typed `CREATE EXTERNAL TABLE`, editor `COPY … TO`, session statements (`SET`/`RESET`,
`PREPARE`/`EXECUTE`/`DEALLOCATE`) and
`CREATE FUNCTION` — while the agent surface stays read-only and every settled funnel (the catalog
store, the persist path, the epoch discipline, the snapshot lifecycle) stays exactly where it is.

**Spec: `docs/STATEMENTS_SPEC.md`.** Read it first — it carries the settled decisions (providers
for identity/visibility, interception for lifecycle; Arrow IPC internal tables as ordinary defs;
DROP on both origins; statements in history; session-scoped SET/PREPARE/functions) and the
**verified DataFusion 54 source facts** every task here builds on. Do not re-derive those facts;
do not re-litigate §3 (why lifecycle cannot live in the provider traits).

The architecture in one line: **classify in front of dispatch, execute through funnels that
already exist** — `policy_block` grows a capability axis and a three-way verdict, `Engine::run`
routes, and an internal table is a `TableDef` over `.strata/tables/<name>/` with `format: Arrow`,
replayed by the existing registration pass (headless host free).

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 01 | Policy router: `classify(stmt, Capability)` + `Verdict`; agent wrapper unchanged | ✅ | — | — |
| 02 | `Engine::run` + statement results: `RunOutcome`/`StatementReport`/`StoreEffect`, app folds, history | ⬜ | — | 01 |
| 03 | Strata providers: `StrataCatalogProvider` + `StrataSchemaProvider`, information_schema on | ⬜ | — | — |
| 04 | Internal tables, engine half: `TableDef.origin`, CTAS spool, `StrataArrowFormat` stats, replay | ⬜ | — | 02 |
| 05 | INSERT (native, target-gated) + DROP TABLE (both origins) | ⬜ | — | 04 |
| 06 | Typed CREATE/DROP VIEW onto the save-view funnel | ⬜ | — | 02 |
| 07 | Editor COPY TO: pre-flight NULL gate + native dispatch | ⬜ | — | 02 |
| 08 | Session statements: SET/RESET overlay · PREPARE/EXECUTE/DEALLOCATE | ⬜ | — | 02 |
| 09 | `StrataFunctionFactory` + swappable function catalog | ⬜ | — | 02 |
| 10 | Typed CREATE EXTERNAL TABLE onto the Table Config funnel | ⬜ | — | 02 |

## Why the order

01 is pure `strata-core` and unblocks everything: both surfaces consume the predicate, and no
statement can be routed until a verdict exists. 02 builds the dispatch spine and the app-side
fold — every later task returns a `StatementReport` through it, so it must exist before any
capability lands. 03 is independent of the chain (it changes enumeration, not dispatch) but
should land before or with 04, so `SHOW TABLES` works — and hides snapshots — by the time the
first internal table exists. 04 → 05 is the only hard chain: INSERT and DROP gate on the
internal-name set and the data-dir layout 04 establishes. 06/07/08/09/10 are parallel after 02 —
each is one `StmtKind` arm plus its engine method(s); 10 maps the parsed statement onto a def and
reuses the registration funnel outright, so it is the smallest of the arms.

## Standing rules this workstream inherits (AGENTS.md §2)

- **The catalog is the `ProjectState` store, not a query.** Every intercepted statement's outcome
  reaches the store as a `StoreEffect` fold — never introspection, never a refetch.
- **Classification stays in front of `ctx.sql`** — DataFusion executes DDL eagerly, so anything
  that must not run is refused before dispatch; the `SQLOptions` per-class floor is defense in
  depth, not the gate.
- **One predicate, N consumers, zero copies** — now with a capability axis. The agent gate keeps
  today's refusals verbatim; parity is a test matrix, not discipline.
- **The snapshot lifecycle is untouched.** Only the query arm retires/spools; DDL never retires a
  snapshot; no epoch enters the query cache key.
- **Silent corruption is refused** — the NULL-partition gate survives editor COPY as a pre-flight
  exact-zero count, never declared metadata.
- **Settings stays the durable config owner** — the SET overlay is session-scoped, owned keys and
  `runtime.*`/`format.*` refuse toward Settings, RESET lands on the Settings baseline.
- **Every def mutation persists at its mutation point** through the persist funnel and bumps the
  epoch via `catalog_settled` — the `save_view` shape, generalized.

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.
