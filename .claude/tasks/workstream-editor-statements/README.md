# Workstream — Editor statements (ED)

Lifting the managed-DDL policy into a **full-statement editor**: internal tables persisted under
`.strata/tables/` (`CREATE TABLE` / CTAS, `INSERT`, `DROP TABLE`), typed `CREATE`/`DROP VIEW`,
editor `COPY … TO`, typed `CREATE EXTERNAL TABLE`, session statements (`SET`/`RESET`,
`PREPARE`/`EXECUTE`/`DEALLOCATE`) and
`CREATE FUNCTION` — while the agent surface stays read-only and every settled funnel (the catalog
store, the persist path, the epoch discipline, the snapshot lifecycle) stays exactly where it is.

**Docs: `docs/STATEMENTS_SPEC.md`** — the statement surface **as built** (router, dispatch,
providers, internal tables, the two writes over them, typed view DDL and typed `COPY`). Read it
first; do not re-litigate its
§3 (why lifecycle cannot live in the provider traits). The settled design for each *unbuilt*
statement — typed view DDL onto the save-view funnel, session-scoped SET/PREPARE/functions, and
the rest — lives in its own task file
below, each self-contained, on top of the **verified DataFusion 54 facts** at the bottom of this
file. Do not re-derive those facts. When a task lands, move its statements out of the doc's
*Not yet implemented* list and document the built behaviour there, in the same change.

The architecture in one line: **classify in front of dispatch, execute through funnels that
already exist** — `classify(stmt, Capability)` answers `Query`/`Intercept`/`Refuse` (ED-01, done),
`Engine::run` routes, and an internal table is a `TableDef` over `.strata/tables/<name>/` with
`format: Arrow`, replayed by the existing registration pass (headless host free).

## Tasks

| # | Task | Status | DEV_TASKS | Depends on |
|---|---|---|---|---|
| 01 | Policy router: `classify(stmt, Capability)` + `Verdict`; agent wrapper unchanged | ✅ | — | — |
| 02 | `Engine::run` + statement results: `RunOutcome`/`StatementReport`/`StoreEffect`, app folds, history | ✅ | — | 01 |
| 03 | Strata providers: `StrataCatalogProvider` + `StrataSchemaProvider`, information_schema on | ✅ | — | — |
| 04 | Internal tables, engine half: `TableDef.origin`, CTAS spool, `StrataArrowFormat` stats, replay | ✅ | — | 02 |
| 05 | INSERT (native, target-gated) + DROP TABLE (both origins) | ✅ | — | 04 |
| 06 | Typed CREATE/DROP VIEW onto the save-view funnel | ✅ | — | 02 |
| 07 | Editor COPY TO: pre-flight NULL gate + native dispatch | ✅ | — | 02 |
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
internal-name set and the data-dir layout 04 establishes. 07/08/09/10 are parallel after 02 —
each is one `StmtKind` arm plus its engine method(s); 10 maps the parsed statement onto a def and
reuses the registration funnel outright, so it is the smallest of the arms.

## The catalog pane is part of 04 and 05, not a later polish pass

Found while building ED-03, settled 2026-08-08. An internal table lands in `ProjectState.tables`
like any other def — it has to, because the store *is* the catalog — so it arrives holding the
whole table-row affordance set, and three of those five items do not mean the same thing on a def
whose data Strata owns. The first draft of this workstream gave the App half one line (ED-04's
read-only Configure window) and left the rest implied.

Corrected in place rather than as a new task, because splitting it out is what would let the two
halves disagree: **ED-04** owns the row saying which origin it is and the menu treatment for an
internal def, **ED-05** owns the drop — one destructive action, one funnel
(`engine::ddl::drop_table`), with the existing dialog as the confirm in front of it. As drafted,
the editor's `DROP TABLE` deleted the data directory and the sidebar's drop did not, which is
silent data left on disk; and the dialog's fixed "files on disk are not deleted" would have been
reassuring the user at exactly the moment the action became destructive.

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

## Verified DataFusion 54 facts (do not re-derive)

Verified against the sources this workspace compiles
(`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`, `datafusion-54.0.0` and siblings).
The open tasks hang off these; the facts behind the *landed* tasks (eager DDL, the in-RAM CTAS,
`information_schema` enumeration, ED-05's `insert_into` and `find_and_deregister` behaviour,
ED-06's replace-a-table hazard, and ED-07's COPY parser/planner behaviour) are restated in the
code's own module docs (`engine/ddl/tables.rs`, `engine/ddl/views.rs`, `engine/ddl/copy.rs`,
`engine/providers.rs`) and in `docs/STATEMENTS_SPEC.md`.

| Fact | Evidence | Task |
|---|---|---|
| PREPARE/EXECUTE/DEALLOCATE supported; plans stored in `SessionState.prepared_plans` (**`pub(crate)`** — no public enumeration); EXECUTE returns the bound plan as a plain DataFrame | `context/mod.rs:733-772`, `:1534-1587`; `session_state.rs:208`, `:984-1013` | ED-08 |
| `SQLOptions::verify_plan` rejects `Ddl` / `Dml`+`Copy` / `Statement` per flag, visiting subqueries — and **cannot see through EXECUTE**, so DML must be fenced at PREPARE | `context/mod.rs:2305-2339` | ED-08 |
| Native SET applies `datafusion.runtime.*` live (rebuilds the RuntimeEnv); native RESET restores **DataFusion's** default, not the Settings baseline — both are why SET/RESET never run natively | `context/mod.rs:1115-1219` | ED-08 |
| CREATE FUNCTION requires a `FunctionFactory` (`with_function_factory`); the body arrives as a parsed `Expr`; DROP FUNCTION deregisters across all registries with no factory needed | `context/mod.rs:2204-2227`, `:474-481`, `:1481-1486` | ED-09 |

## Legend
✅ done · 🟢 UI only · 🟡 partial · ⬜ todo · `[core ✓]` logic in `strata-core`.
