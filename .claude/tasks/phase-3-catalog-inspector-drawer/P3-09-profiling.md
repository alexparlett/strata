# P3-09 · Column/table profiling (PROFILE zone)

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** D4 · **Depends on:** P3-08

## Goal
A PROFILE zone in the inspector: a full-scan profile of a table's columns, on demand.

## Current state
Not built. Core has `Command::Profile` / `CancelProfile` / `Event::Profiled` and the DataFrame-API
scan logic. **In Freya, freya-query is the profile cache** — plan §4 says it *replaces* the Dioxus
hand-rolled `CatalogTable.profile` cache + dedup + spinner. Do **not** re-add that cache.

**P3-08 built the zone this fills in** (`views/inspector/`), and left three seams:

- **The trigger.** `column.rs::profile_card` renders the canvas's scan card in full; its button has
  **no press handler**, and adding one is the whole wiring (through P3-10's confirm on a *first*
  profile, straight through on a re-scan). The row menus' parked `Profile table` / `Profile view` items
  (`sidebar/catalog/menu.rs`) are the same action from the catalog side and must call it, not a
  second copy. Once a scan exists the card has nothing left to offer: per the canvas its controls
  (age · view-as-query · re-scan) move to the **STATISTICS header**, which is where they go in
  `column.rs::statistics`.
- **The facts.** `model.rs::fact_rows` already walks a fixed `FACT_ORDER` over `ColumnFacts.stats`
  and renders a row per fact that exists — fold the scanned facts into that one list (free wins a
  tie, matched on `StatKey`), rather than adding a second box. `NULLS` is deliberately excluded
  from the rows: it is the completeness bar, and `model.rs::completeness` will take a scanned null
  count as readily as a footer one, which is what finally answers the `null_count == num_rows`
  case the engine has to drop.
- **What a scan may not describe.** Only top-level columns are profiled and the profile is keyed
  by their names, so a nested path (`ColRef::is_child`) must refuse the lookup outright — by leaf
  name, `address.city` would collect an unrelated top-level `city`'s facts. The distribution bars
  and the "Full scan · N rows" footnote are yours to add to `statistics()`.

## Build
1. Model profiling as a freya-query **query keyed by table** (server data): loading/error/cancel come
   from `query.read().state()`; a duplicate request for the same table dedups automatically.
2. **Per-type facts** (from core): Num → distinct/min/max/mean/median · Ts, Str → distinct/min/max ·
   Bool, nested → nulls only; everything gets nulls. A fact never appears in both the free-metadata
   box (P3-08) and the profile zone (matched on `StatKey`).
3. **Invalidation:** register / deregister / refresh mutations (`on_settled`) invalidate the table's
   profile query (they also abort in-flight scans engine-side).
4. A **per-row sidebar spinner** while a table is profiling (drive from the query state).

## Acceptance
- [ ] Profiling a table shows per-type facts; a second request while running dedups; cancel works.
- [ ] Registering/refreshing a table invalidates its profile.

## Freya / references
- Freya `use_query` (plan §4: replaces the profile cache/dedup/spinner). Core `Command::Profile` /
  `CancelProfile` / `Event::Profiled`. DEV_TASKS D4 (per-type facts + the honesty calls). Confirm from
  P3-10 first (cost confirm).
