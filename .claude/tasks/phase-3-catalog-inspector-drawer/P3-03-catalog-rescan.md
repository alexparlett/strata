# P3-03 · Catalog re-scan

**Phase:** 3 · **Status:** ⬜ `[core partial]` · **DEV_TASKS:** D5 · **Depends on:** P3-02

## Goal
A refresh button re-infers catalog schemas.

## Current state

**Half-built in core, and not the half the old note claimed.** `Command::RefreshCatalog` does
**not** exist — the whole `Command`/`Event` protocol was deleted from `strata-core` with P2-01
(it survives only in the non-building `strata-dioxus`).

What does exist is the helper `catalog::rebuild_listing` ([engine/catalog.rs:130](../../../crates/strata-core/src/engine/catalog.rs))
— parked `#[allow(dead_code)]` with *"Feature reservoir: consumed by `Engine::refresh_catalog`
when the catalog task lands"*. **`Engine::refresh_catalog` is that missing piece, and this task
writes it.** The facade currently exposes `register` / `deregister` / `create_view` / `drop_view`
and nothing else for the catalog.

Re-registering the *same* provider wouldn't re-infer, which is why `rebuild_listing` builds a
fresh `ListingTable` from a re-`infer_schema`d config. It takes `paths` + `opts` as arguments
rather than looking them up, because they must be the **live table's own** — `opts` carries
`collect_stat`, which is baked in at `try_new` and can't be fixed after the fact.

## Build

1. **`Engine::refresh_catalog(names: Vec<String>) -> Vec<(String, Result<TableMeta, String>)>`**
   on the facade (`engine/mod.rs`, `--- catalog ---` section). Per name: `ctx.table_provider(name)`
   → `downcast_ref::<ListingTable>()` → clone its `table_paths()` + `options()` → `rebuild_listing`.
   A name that isn't a `ListingTable` (a view) is skipped, not an error. Runs on the engine's
   runtime like every other call.
2. **Wire the sidebar refresh button** (the affordance is built inert in P3-02): spawn the call,
   land each answer on the Project store with `table_registered` / `table_failed` through
   `ProjChan::Tables` — the same landing path `register_defs` ([state/hooks.rs](../../../crates/strata-freya/src/apps/project/state/hooks.rs))
   already uses. Not a freya-query capability and **not** a `FetchCatalog` invalidation: the
   catalog is the Project store's defs + `Reg` state, not cached server data (see P3-02).
3. **Spinner** from a local `State<bool>` around the spawned call, disabling the button while it
   runs. Data — file sets, row counts, partition values — is already live (DataFusion re-`LIST`s
   per scan, no list cache); only the *inferred schema* is frozen at registration, so that is all
   this refreshes.
4. **Drop any cached profile** for a refreshed table. Re-inferring means the files may have moved
   under it, which is exactly when a cached scan becomes a lie. (Lands with P3-09; leave the hook
   obvious if profiles aren't stored yet.)

> **Decision to settle when building:** a view's plan is resolved against the table schemas at
> `CREATE VIEW` time, so a table whose schema actually drifted leaves its dependent views stale.
> Either re-create views after the tables land (the same fixed-point loop as `register_defs`), or
> declare refresh table-only and let P3-04 flag the drift. The Dioxus app did table-only.

## Acceptance
- [ ] Refresh re-infers every registered table's schema; rows flip through `Loading` and land `Ready`/`Failed`; the Events log records the outcome.
- [ ] The spinner shows for the duration and the button is disabled while it runs.

## Freya / references
- `catalog::rebuild_listing` (the parked helper this task consumes). Design: `Sidebar.dc.html` refresh affordance.
- Reference behaviour: `strata-dioxus/src/action/catalog.rs::refresh` + the in-place row update at `app.rs:422`.
