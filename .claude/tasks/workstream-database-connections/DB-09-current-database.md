# DB-09 · A current database, so unqualified names resolve

**Workstream:** Database connections · **Status:** ⬜ · **Depends on:** DB-02

## Goal

`SELECT * FROM orders` against a connected Postgres, rather than
`SELECT * FROM pg.public.orders` every time. A **current database and schema** for the session,
the way Trino's `USE catalog.schema` and psql's search path work, so the three-part name is what
you type to reach *across* sources and not what you type to work in one.

Asked for directly (2026-08-14): "We want users to be able to write `select * from orders` and not
`select * from pg.public.orders` every time. This should be default catalog (pg) and default
schema (public) aware."

## Current state (verified 2026-08-14)

**DataFusion has one default catalog and one default schema — there is no search path.** So
"unqualified `orders` finds the Postgres table" and "unqualified `orders` finds the workspace's
own table" cannot both hold at once. Whatever this task builds is a *switch*, not an addition.

- `build_context` pins `datafusion.catalog.default_catalog` / `default_schema` to `CATALOG`
  (`strata`) and `SCHEMA` (`public`) — `engine/mod.rs`.
- Both are **owned keys** (`engine::config::is_owned_key`), so Settings skips them on apply and a
  typed `SET` refuses with `Blocked::SetOwned` (`ddl/session.rs`). That gate is deliberate and is
  the smallest part of this task to move.
- **`engine::providers::in_workspace` answers `true` for every bare name**
  (`TableReference::Bare { .. } => true`), and that is the load-bearing assumption:

  | Reader | What it does with `in_workspace` | What a moved default breaks |
  |---|---|---|
  | `is_snapshot_ref` | fences `__snap_` names in the workspace catalog | a bare `__snap_3` is still fenced; the *argument* for the fence stops holding |
  | `ddl::bare_name` / `Engine::is_internal` | decides whether a write may target a relation | a bare INSERT target reads as workspace-owned when it is remote |
  | `PlanDeps` / `ViewMeta` | records workspace scans **bare**, remote scans **qualified whole** | **the sharp one** — a view whose body says `orders` is recorded as a workspace dep while reading Postgres, so dropping an unrelated workspace table names a view that never read it, `view_problem` cries wolf, and forgetting the connection matches nothing |

  That last row is exactly the failure INVARIANTS.md's two-list split exists to prevent, so this
  task cannot land by flipping config and leaving `in_workspace` alone.

## Build

1. **A session-scoped current database**, not a project setting and not a per-connection flag: one
   `(catalog, schema)` on the engine's `SessionScope`, defaulting to the workspace's own. The
   overlay `ddl::session` already owns is the right home — this is the same shape as a `SET` that
   `RESET` restores.
2. **Unblock the two keys *through that overlay only***. `is_owned_key` keeps refusing them from
   Settings (a persisted default catalog would make a project file that opens differently
   depending on which connections happen to be live), and the gesture below is the one writer.
3. **Teach `in_workspace` to resolve rather than assume.** A bare name is the workspace's *when
   the current catalog is the workspace's* — otherwise it resolves against the current
   `(catalog, schema)` like DataFusion does. One function, so every reader in the table above
   moves together; `PlanDeps` then records a bare `orders` read under a Postgres default as the
   **qualified remote** dep it actually is.
4. **The gesture.** DB-05's tree is where a database is looked at, so "Use this database" belongs
   on its node (and on a schema node for the schema half). A typed `USE pg.public` is the second
   gesture into the same funnel, on ED-11's precedent — the router already intercepts statements
   and this is one more arm.
5. **Say which one is current**, or the same query silently means two things on two days: the
   status bar carries it, and the tree marks the current node. Completion offers unqualified names
   from the current database first.
6. **A restart clears it** — a new `Engine` is a fresh `SessionScope`, which is already true of
   every other session overlay and is what stops a stale default outliving the window.

## Acceptance

- With a Postgres connection current, `SELECT * FROM orders` returns its rows and
  `SELECT * FROM strata.public.<a workspace table>` still resolves.
- A view created while a database is current records its dependencies **qualified**, and dropping
  a same-named workspace table does not name that view (the `PlanDeps` regression this task exists
  to avoid — assert it, it is the whole risk).
- `__snap_` stays fenced under a moved default, for reads and writes.
- An `INSERT` into a bare name that resolves remote is refused as remote, not accepted as internal.
- `RESET` and a restart both put the workspace back.
- Driven against the real container in `postgres_federation.rs`, since a fake catalog cannot show
  the resolution difference.

## Files

`crates/strata-engine/src/{providers.rs, config.rs, ddl/session.rs, mod.rs}` ·
`crates/strata-freya/src/apps/project/views/` (the tree gesture and the status bar) ·
`docs/CONNECTIONS_SPEC.md`, `docs/STATEMENTS_SPEC.md`.
