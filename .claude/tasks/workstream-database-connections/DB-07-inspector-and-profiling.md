# DB-07 · Column inspector + profiling for remote tables

**Workstream:** Database connections · **Status:** ⬜ · **Depends on:** DB-05

## Goal

Selecting a remote table in the tree points the column inspector at it, and profiling works
on the same terms as a workspace table — opt-in, confirmed, one entry point — with a
**remote-specific expression set** federating to the server (the local set's median cannot;
Current state has the proof). Two structural decisions: the selection model widens in
`strata-model` to name a remote relation, and the profile request gets a window-side slot
because a remote table has no `ProjectState` row — the store grows nothing.

## Current state (verified 2026-08-13, corrected in review)

- The inspector reads the selected table's `TableMeta` columns; profiling is P3-09's shape:
  the row holds `Option<ScanId>` (a nonce minted per ask), the numbers live only in the
  freya-query cache entry that key names (`stale_time(MAX)`, `clean_time(MAX)`), a re-scan
  is a new nonce, and every trigger goes through `ProfileActions::ask` — the confirm on
  first scan, straight through on re-scan (INVARIANTS: "an expensive, opt-in result…" and
  "one entry point per expensive action").
- **Selection is `CatalogSelection = State<Option<ColRef>>`** (`state/catalog.rs:19`,
  consumed at `views/inspector/mod.rs:35,116`) — a variant-less struct
  `ColRef { kind: CatalogKind, owner, path }` in **`strata-model/catalog.rs:48-55`**; there
  is no `Selection` enum on this surface (the only enum of that name is the results grid's
  cell selection, unrelated). Widening it to carry a remote target is therefore a
  **model-crate change** with two consumers to audit, not "add a variant".
- **`ProfileActions` is `ProjectState`-row-bound end to end**
  (`dialogs/profile_confirm.rs:79-140`): `needs_confirm` reads `project.profile_scan`,
  `start` calls `request_profile` and *bails silently when the row is absent* — precisely
  the confirmed-cost-then-nothing regression the suite pins elsewhere
  (`catalog/interaction.rs:1086-1110`). A remote table has no row by design, so
  ask/needs_confirm/start/clear/reveal all need a second storage backing, not "one new arm".
- **The remote profile cannot simply federate `run_profile`'s SQL**: `aggregates()` gives
  every `Kind::Num` column a median via `approx_percentile_cont`
  (`engine/profile.rs:98-123`) — a DF-only aggregate with no Postgres spelling, and DF 54's
  `PostgreSqlDialect` has **no aggregate override hook** (scalar overrides only). Since a
  federated subplan has no per-expression fallback, profiling any remote table with a
  numeric column would die server-side. Audit the whole expression set the same way
  (`approx_*` anything is suspect); the remote profile needs its **own expression set**,
  restricted to aggregates the unparser renders into SQL Postgres runs.
- **`profile_sql` cannot render a qualified owner**: it wraps the whole name in
  `quote_ident` (`profile.rs:166-170`), which emits `FROM "pg.public.orders"` — one
  identifier, resolving nothing (mod.rs:1770-1776). The remote arm renders the owner
  segment-by-segment through the case-preserving helper DB-06 exports.
- Columns for a remote table come from the cached provider's Arrow schema (DB-02) — cached
  **after first touch**: the first selection of a table this session performs that one
  introspection, so the inspector budgets a loading state for it; subsequent selections are
  instant.
- Free stats (`free_stats`) read listing/file metadata — a remote table has none; the
  inspector's "free" tier for remote is whatever the Arrow schema says (types, nullability
  as declared) plus the server's own cheap facts if any (`pg_class.reltuples` is a
  free-tier row-estimate candidate — clearly labeled an estimate, and already in DB-02's
  connect-time listing query's reach).

- **The tree's relation rows are leaves, and that is this task's to change** (DB-05, as built):
  a relation draws no disclosure today because its columns are the same introspection this task
  performs, and a column row under it could not be *selected* until `ColRef` widens here. So the
  affordance arrives with the capability. The tree is virtualized since DB-05's follow-up, so that
  is **two edits in the walk, not a row-local disclosure**: `connection.rs`'s `database()` gives a
  relation `Node::branch(.., open, can_open, ..)` in place of today's `Node::leaf`, and pushes its
  column rows after it, off the same read the inspector uses; `relation_row` in the same file only
  draws what the walk decided.

## Build

1. **Selection** — widen the selection model in `strata-model` (the honest scope from
   Current state): `ColRef.owner` generalizes to a target that can name a remote relation
   (`(connection url, schema, table)`) with both existing consumers audited; tree selection
   sets it; the inspector renders the remote header (connection badge + qualified name) and
   the column list from the engine's cached schema — with a loading state for the
   first-touch introspection.
2. **`ProfileTarget` with one `Query` builder** — `ProfileTarget::{Workspace(kind, name),
   Remote { url, schema, table }}`, and the freya-query `Query` (keys, `stale_time`,
   `clean_time`) is **built in one place over the target** — never a second spelling beside
   the workspace one (INVARIANTS: "the `Query` is the identity, built in one place").
   The request storage generalizes with it: the workspace arm keeps the row's
   `Option<ScanId>`; the remote arm's slot is a window-level satellite
   (`(url, schema, table) → Option<ScanId>`), dropped with the window, on Forget, and on
   epoch move (a re-connect invalidates what the scan described). The store is untouched.
3. **`ProfileActions` generalizes over the target** — `ask`/`needs_confirm`/`start`/
   `clear`/`reveal` take `ProfileTarget` and read whichever storage backs it (the silent
   remote no-op in Current state is the failure this step exists to prevent — a confirmed
   ask that starts nothing is the pinned regression class). Same confirm component, wording
   naming the server ("Profiling scans 'pg.public.orders' on the database…").
4. **The remote profile expression set** — its own, in `engine/profile.rs` beside the local
   one: count / null count / min / max / avg where the unparser provably renders them, the
   median **dropped for remote** (or spelled `percentile_cont … WITHIN GROUP` only if a
   verified unparse path exists — never assumed), every included aggregate pinned by the
   integration test's EXPLAIN. The rendered SQL builds the owner through DB-06's
   segment-quoting helper, so "view as query" hands over a runnable statement.
5. **Free tier** — types/nullability from the schema; `reltuples` as a labeled estimate if
   it fits the existing free-stats card without a new surface.
6. **Docs** — the inspector/profiling notes in `docs/reference/SETTLED_TASKS.md`-adjacent
   docs stay true; INVARIANTS' profiling entry gains the remote clause (request in a
   window-side slot when there is no row — the rule generalized, not excepted).

## Acceptance

- Selecting a remote table shows a loading state at most once (the first-touch
  introspection), then its columns; re-selecting is instant; selecting back a workspace
  table is unchanged.
- Profile confirm → the remote expression set's numbers arrive, and the integration test's
  EXPLAIN shows the federated aggregation for **that** set (the median is absent by
  design); a confirmed ask that starts nothing is impossible by construction (the
  generalized `start` has no silent bail arm).
- "View as query" on a remote profile hands over SQL that runs (segment-quoted owner).
- Re-ask re-scans (new nonce); forget/↻ invalidates; nothing lands on `ProjectState`
  (grep-proof).
- Existing inspector/profiling tests untouched; new coverage for the remote arm with a fake
  engine listing (no network in UI tests).

## Files

`crates/strata-model/src/catalog.rs` (`ColRef`/selection widening) ·
`crates/strata-freya/src/apps/project/` (inspector views, `dialogs/profile_confirm.rs`'s
`ProfileTarget` generalization, the window-side request satellite) ·
`crates/strata-core/src/engine/{profile.rs (remote expression set + qualified rendering),
catalog.rs, db.rs (`reltuples`)}` · `crates/strata-core/tests/postgres_federation.rs` (the
profile EXPLAIN phase) · `docs/reference/INVARIANTS.md`.
