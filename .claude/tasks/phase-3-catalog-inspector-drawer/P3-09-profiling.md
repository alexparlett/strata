# P3-09 · Column/table profiling (PROFILE zone)

**Phase:** 3 · **Status:** ⬜ `[core ✓]` · **DEV_TASKS:** D4 · **Depends on:** P3-08

## Goal
A PROFILE zone in the inspector: a full-scan profile of a table's columns, on demand.

## Current state
Not built. Core has `Command::Profile` / `CancelProfile` / `Event::Profiled` and the DataFrame-API
scan logic. **In Freya, freya-query is the profile cache** — plan §4 says it *replaces* the Dioxus
hand-rolled `CatalogTable.profile` cache + dedup + spinner. Do **not** re-add that cache.

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
