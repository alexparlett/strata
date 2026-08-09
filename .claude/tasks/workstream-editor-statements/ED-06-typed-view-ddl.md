# ED-06 · Typed CREATE/DROP VIEW onto the save-view funnel

**Workstream:** Editor statements · **Status:** ⬜ · **DEV_TASKS:** — · **Depends on:** ED-02

## Goal

Typed view DDL becomes a second gesture into the funnel ⌘S already uses — one merge path, views
indistinguishable by origin. The dispatch and settle it rides: `docs/STATEMENTS_SPEC.md` §2.

## Current state

- `Engine::create_view(name, sql)` (`engine/mod.rs:1025`) renders
  `CREATE OR REPLACE VIEW {quote_ident} AS {sql}`, reads back columns + `plan_deps`;
  `drop_view` (`mod.rs:1070`). The Save flow (`editor/actions.rs:254`) folds
  `ViewDef` upsert → persist → engine call → `ViewMeta` fold → epoch bump.
- Verified hazard (workstream README, DataFusion 54 facts): DF's own `CREATE OR REPLACE VIEW` over a **table** name silently
  replaces the table — the interceptor must fence it; another reason the statement never runs
  natively. The other reason: the store write-back needs `ViewMeta`, and introspecting for it
  would violate catalog-is-the-store.

## What to build

`engine/ddl.rs::{create_view_stmt, drop_view_stmt}` (thin — fences plus delegation):

- From the parsed `CreateView`: folded name + the definition query's canonical rendering (this
  is what lands in `ViewDef.sql`, so it round-trips through Save exactly as ⌘S text does).
- Fences: name resolves to a base table → refuse ("'sales' is a table"); plain `CREATE VIEW`
  over an existing view → "View 'v' already exists. Use CREATE OR REPLACE VIEW."; a
  `__snap_`-prefixed view **name** or a `__snap_` reference in the body → `Blocked::ReservedName`
  (spec §4, both halves — a reserved view name would collide with a live snapshot registration).
- Otherwise delegate to `Engine::create_view` → `StoreEffect::ViewUpserted { def, meta }` — the
  app-side fold is ED-02's shared settle, which is the same sequence `save_view` performs.
- `DROP VIEW`: type-check (a table name → the DROP TABLE arm's territory, refuse here),
  `Engine::drop_view`, `StoreEffect::ViewRemoved`. `IF EXISTS` honored.
- Update the views-are-Save's-artifact invariant text (AGENTS.md §2 + INVARIANTS.md + the
  `Blocked::CreateView` doc comment) in this change — typed view DDL is a second gesture into the
  same funnel; the variant and message stay as the agent path's refusal — and move CREATE/DROP
  VIEW out of `docs/STATEMENTS_SPEC.md`'s *Not yet implemented* list, documenting the built
  behaviour there.

## Acceptance

- `CREATE OR REPLACE VIEW v AS SELECT …` in the editor lands the same store row, `project.json`
  entry, deps, and epoch bump as Save-as-view of the same SQL; editing it later via either
  gesture updates the same row.
- `CREATE VIEW` over an existing view refuses toward OR REPLACE; over a table name refuses with
  the table message; the DF replace-a-table hazard is unreachable (test drives the statement
  through `Engine::run`).
- `DROP VIEW` removes row + def and bumps the epoch; dependents-of-view behavior matches the
  catalog surface's (D11 semantics).
- Replay ordering: a typed view over another view survives restart (`view_order` already covers
  it — pinned by a test with a typed-created chain).

## Verification

`cargo test -p strata-core`; run the app: type-create a view, see it in the sidebar VIEWS
section, ⌘S-edit it from the row menu, restart, still correct.
