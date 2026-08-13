# DB-06 · Gestures + completion over the tree

**Workstream:** Database connections · **Status:** ⬜ · **Depends on:** DB-05

## Goal

The tree becomes a place work starts, and the editor knows the remote names: a remote table
node opens a query, pins as a view, and completion offers catalog-qualified names scoped to
the enabled schemas.

## Current state (verified 2026-08-13)

- Completion pool: built from the catalog store + the engine's function catalog
  (`docs/COMPLETION_SPEC.md`); no catalog-qualified names are offered today (`strata.public`
  names are offered bare). The shape for carrying remote listings into the pool is the
  **`Functions::catalog()` handle** (`engine/functions.rs:65` — a swappable
  `Arc<FunctionCatalog>` the completion input carries, `engine/sql/symbols.rs:86-89`, no
  deep copy per rebuild; `functions::snapshot` at functions.rs:111 is the *private rebuild*
  behind it, not the handle). **The timing constraint is real** (corrected in review): the
  completion input is rebuilt per **catalog epoch**, and expanding a tree node bumps no
  epoch — so "the tree warms it" only works if the remote-listing handle is
  interior-swappable *inside* an already-built input (an `Arc` whose contents the listing
  warm replaces, exactly how a new `Arc<FunctionCatalog>` swap works), never by waiting for
  the next epoch and never by bumping the epoch on expand (diagnostics key tab staleness
  off the same counter — a bump would re-validate every open tab per tree click).
- The editor-popup constraint (AGENTS §8): extend the **autocomplete** surface only; the
  hover model is off-limits.
- Promotion precedent: a statement lands in a **new** tab (`actions::open_sql`), never a
  write into the user's buffer. **Quoting needs a decision, not a pointer** (corrected in
  review — "the one helper" is three helpers with three semantics, none right): the
  completion insert's `ident_insert`/`needs_quoting` are private to
  `engine/sql/complete/mod.rs:717,732`; `engine::quote_ident` **lower-cases** a bare word
  (mod.rs:2375 pins `"DailySales"` → `dailysales` — wrong for a remote relation whose case
  is the server's); `export::quote_col` quotes unconditionally. This task **exports the
  completion insert's case-preserving pair** from `strata-core` (one home, the same rule
  the popup already applies) and renders qualified names segment-by-segment through it —
  used by both gestures here and by DB-07's remote `profile_sql` fix.
- Diagnostics need nothing: DB-03 verified remote names resolve through the registered
  catalog (providers cached per table, so validation costs no network per keystroke).

## Build

1. **Query gesture** — a remote table/view node's double-press or ⋮ *Query*: a new tab with
   `SELECT * FROM {catalog}.{schema}.{table} LIMIT 100`, identifiers quoted as needed.
2. **Pin as view** — ⋮ *Pin as view…*: a new tab pre-filled with
   `CREATE VIEW {table} AS SELECT * FROM {catalog}.{schema}.{table}` for the user to rename
   and run — composing, never executing (the Shape-panel precedent: compose into an unrun
   tab). This is the workstream's "make it a bare-named def" gesture, and it lands in the
   store through the view funnel that already exists.
3. **Completion** — the offer grows qualified names: connection catalog names always (from
   the store); schema and table/view segments from `Engine::db_listing`'s scoped answer
   (the one visibility source, DB-02) through the interior-swappable handle described in
   Current state — so a listing warmed by the tree or a query reaches the *next* completion
   pass without an epoch bump. **No dial-out from the completion path**: a schema not yet
   listed is offered as a name but completes no children until warmed — state that bound,
   and the swap mechanism, in COMPLETION_SPEC. Non-enabled schemas are not offered (they
   still resolve if typed — visibility, not policy; DB-02).
4. **Docs** — COMPLETION_SPEC's pool description; CONNECTIONS_SPEC's gestures.

## Acceptance

- Both gestures produce runnable tabs against a container-backed connection; Pin as view,
  once run, puts a bare-named row under the workspace node that survives reopen.
- Typing `pg.` offers enabled schemas; after expanding `pg → public` in the tree (no rescan
  in between), `pg.public.` offers the listed table/view names on the next completion pass;
  a non-enabled schema name is absent from the offer but a typed query against it runs.
- No completion-path network (assert by construction: the pool reads caches only).
- Existing completion tests untouched; new ones cover the qualified offer.

## Files

The completion pool module (per COMPLETION_SPEC) · `sidebar/catalog/` (menu entries) ·
`workbench/editor/actions.rs` (tab-opening helpers) · `docs/{COMPLETION_SPEC,
CONNECTIONS_SPEC}.md`.
