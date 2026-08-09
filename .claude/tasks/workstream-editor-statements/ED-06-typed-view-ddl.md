# ED-06 · Typed CREATE/DROP VIEW onto the save-view funnel

**Workstream:** Editor statements · **Status:** ✅ · **DEV_TASKS:** — · **Depends on:** ED-02

## Goal

Typed view DDL becomes a second gesture into the funnel ⌘S already uses — one merge path, views
indistinguishable by origin. The dispatch and settle it rides: `docs/STATEMENTS_SPEC.md` §2.

## As built

`engine/ddl/views.rs` holds **both halves**: `create` / `drop` are the ctx-level bodies (moved out
of `Engine::create_view` / `drop_view`, which now spawn them), and `create_statement` /
`drop_statement` are the arms. Documented as built in `docs/STATEMENTS_SPEC.md` §6.3.

Three things the plan below did not spell out, settled while building:

- **The clause fences are exhaustive, not selective.** `create` rebuilds the statement around the
  parsed query, so DataFusion's own `CREATE VIEW` clause gate never sees the user's spelling —
  anything not read here is accepted and silently ignored, and `CREATE TEMPORARY VIEW` would
  create a permanent view. `definition` destructures sqlparser's `CreateView` with **no `..`**, so
  a clause added upstream is a compile error. A view's **column list** is refused for the same
  reason `ViewDef` is `{ name, sql }`: there is nowhere for it to round-trip.
- **A view drop's dependents are the *aliases* half of `PlanDeps`**, not the tables half — the
  inliner leaves a view's name behind as a `SubqueryAlias` and its base tables at the leaves, so
  the table drop's `dependent_views` finds nothing for a view target. `catalog::dependents_of_view`
  is its sibling over the same walk; the report's sentence is `ddl::left_invalid`, now shared with
  the table drop.
- **The profile cancel moved to `Engine::settle_effect`.** The arm runs in a task that cannot
  reach the lifecycle (the `TableRemoved` reason); the direct gestures keep theirs, since they
  never produce an effect.

`existing` and `bare_name` moved to `ddl/mod.rs` as the shared helpers both arms use — `bare_name`
gained the object noun so a `CREATE VIEW` in another schema is not told about tables.

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
