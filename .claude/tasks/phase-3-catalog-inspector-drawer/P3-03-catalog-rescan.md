# P3-03 · Catalog re-scan

**Phase:** 3 · **Status:** ✅ · **DEV_TASKS:** D5 · **Depends on:** P3-02

## Goal
A refresh button re-infers catalog schemas.

## What was built

The sidebar's ↻ runs a **catalog re-scan**: every table's schema re-inferred from its def, then
every view re-created over what that found. Rows drop to `Loading` first and land
`Ready`/`Failed` through the normal registration path; the button spins and is disabled for the
duration — including the registration pass at project open, which is the *same* scan.

- `state/hooks.rs` — `refresh_catalog` (the ↻ action) + `scan_catalog` (one pass, flag held).
  `register_defs` is now shared by project open and the re-scan.
- `state/catalog.rs` — `CatalogScan`, a context signal (`State<bool>`) beside `CatalogSelection`.
  Provided by `use_init_project`, which owns the first pass.
- `state/project.rs` — `reload_tables` / `reload_views`, and the profile-invalidation seam
  documented on `table_registered` (the one funnel every answer lands through).
- `views/sidebar/mod.rs` — the affordance P3-02 shipped inert, wired.

**Also fixed a P3-02 layout bug this surfaced:** the header row hugged its content instead of
distributing it, and the filter was `Size::fill()` — which takes the whole parent width regardless
of its siblings. Both 24px controls (↻ *and* the collapse ×) were laid out at `256..280` on a
260px panel, stacked and off the edge, so ↻ was never visible. Fixed with `Content::Flex` on the
header row + `Size::flex(1.)` on the filter — the trap `SidebarRow` already documents.
`views/sidebar/mod.rs`'s `mod tests` pins it: nothing lays out past the panel edge, both controls
are present at full size and on screen, and the filter leaves the button room. All three fail
against the old layout.

## The engine decision (this overrides the original D5 design)

**A re-scan is a re-registration from the defs, not a walk of the live providers.**
`Engine::register` → `catalog::register_external` already deregisters and rebuilds each table
from a re-`infer_schema`d config — the same re-infer the old `Command::RefreshCatalog` did via
`rebuild_listing`. Driving it from the **def** instead of the live `ListingTable` buys the case
the button most needs to serve: a table whose registration **failed** has no provider to rebuild
from, so a live-provider walk can't retry it at all — the user fixes a path, presses ↻, and
nothing happens. It also needs no new engine surface.

So `Engine::refresh_catalog` was **not** written, and the parked `catalog::rebuild_listing`
helper was **deleted** — its only intended consumer is this task, and this task doesn't need it.
(`is_owned_key`'s doc lost its stale `RefreshCatalog` reference; the `strata`/`public` naming
stays — it's what keeps *any* table lookup resolvable after a config apply.)

**Views are re-created too** — the open decision, settled. A view captures each base table **by
`Arc`** in the plan it stores at `CREATE VIEW` time and never re-resolves the name at query time
(verified against DF 54 for D10/D11, commit `8acf831`). So re-registering `orders` doesn't break
a view over it — worse, the view keeps scanning the *old* provider with the *old* schema. Only
re-issuing `CREATE OR REPLACE VIEW` re-plans it. That is a refresh, **not** a validity check: a
view-of-a-view masks a missing table behind the still-live inner `Arc`, which is why P3-04
derives validity from `deps`.

Only the inferred *schema* is refreshed — file sets, row counts and partition values are already
live (no `ListFilesCache`, so DataFusion re-`LIST`s per scan).

## Left for the tasks that own it

- **Events log** — no `LogCtx` exists yet (P3-13 writes it); the scan logs through `tracing`,
  where every other registration outcome already goes.
- **Profile invalidation** (P3-09) — no profile cache exists yet to drop. The seam is documented
  on `ProjectState::table_registered`: a landing answer must drop the table's profile *and* the
  profile of every view whose `ViewInfo::deps` name it, and abort scans in flight.

## Acceptance
- [x] Refresh re-infers every registered table's schema; rows flip through `Loading` and land `Ready`/`Failed`.
- [x] Views are re-created over the re-inferred tables, so a schema change reaches them.
- [x] A failed table is **retried** by a re-scan.
- [x] The spinner shows for the duration and the button is disabled while it runs.
- [ ] Events log records the outcome — blocked on P3-13 (no log store yet); `tracing` for now.

## Freya / references
- Design: `Sidebar.dc.html` refresh affordance.
- Reference behaviour: `strata-dioxus/src/action/catalog.rs::refresh` + the in-place row update at
  `app.rs:422`; the original D5 commit `be64338`; the D10/D11 `Arc`-capture finding in `8acf831`.
