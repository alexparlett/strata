# Internal tables in the UI (IT)

**Status:** ⬜ open · **Planned:** 2026-08-13

An internal table — one whose data Strata owns, spooled into `.strata/tables/<slug>/` — can
today be **created only by typing SQL**. Everything else about it already has a surface: the
catalog row drops it (with the confirm that knows the data goes), refreshes it, profiles it,
and the assistant can be asked about it. Creation is the one verb in that set with no gesture.

This workstream closes that, without moving anything the ED workstream settled: an internal
table stays an ordinary def whose data Strata owns, `ddl::tables` stays the one implementation,
and every gesture added here is **a second entry into a funnel that already exists** (AGENTS §2).

## The two gestures

| # | Task | Statement | Status |
|---|---|---|---|
| IT-01 | [The empty-table panel](IT-01-empty-table-panel.md) — name + columns, from the catalog `+` | `StmtKind::CreateTable` | ⬜ |
| IT-02 | Save results as table — from the results toolbar, beside Export | `StmtKind::Ctas` | ⬜ *(file not written)* |

The classifier already splits these two ([`validate.rs:568`](../../../crates/strata-core/src/engine/sql/validate.rs), on
`create.query.is_some()`), and `ddl::tables::create` already serves both from one body. Two
kinds in the engine, zero in the UI — that split is what makes these two tasks rather than one.

**IT-02 in one line:** the results toolbar gains an action beside Export, enabled on the same
`Option<ExportLaunch>` that already means "a run settled with rows"; it asks for a name and
composes `CREATE TABLE <name> AS <the tab's SQL>`. The copy must say it **re-runs the query**
rather than saving the rows on screen — the spool is the parsed plan, never the snapshot
(AGENTS §2), so a source that moved underneath gives a table that differs from the grid. Write
this file before starting it.

## Settled while planning (2026-08-13, with Alex)

- **The type field is free text, validated per row** — not a curated type picker. Deriving the
  offer from Arrow was explored first and the finding is recorded in IT-01: DataFusion ships no
  Arrow → SQL inverse, the mapping is many-to-one, and the same spelling yields *different*
  Arrow types depending on session config. Free text costs nothing to keep in step and covers
  every spelling the planner accepts on day one. The condition attached to it — validation is
  **per row and eager**, never deferred to the press — is what makes it usable, and the probe
  that does it hands back the resulting Arrow type as the row's own detail line.
- **A picker, if it ever earns its place, is a suggestion list writing into the same box.** The
  composed statement, the funnel and the validation do not change. Nothing here has to be
  unpicked to add one.
- **No constraints, no defaults, in either task.** `ddl::tables::create` refuses both outright
  (tables.rs:118-123) — a constraint DataFusion plans but never enforces "would be a promise
  nothing keeps". A form offering NOT NULL or PRIMARY KEY would compose statements the engine
  rejects.

## Known seam

The catalog `+` is where IT-01's panel is triggered from, and **DB-05 moves that `+` to the
data-sources tree's header**. Build the menu where the `+` lives when the task is picked up; if
DB-05 has landed first, it lands on the tree. Noted in both files.
