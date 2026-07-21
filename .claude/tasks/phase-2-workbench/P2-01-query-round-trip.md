# P2-01 · Query round-trip + result snapshot system (design + build)

**Phase:** 2 — Workbench · **Status:** ⬜ **needs design** · **Depends on:** — · **Unblocks:** most of Phase 2

> **Design-first for the snapshot half.** Write the short `docs/SNAPSHOT_SPEC.md` and agree it before
> building — pagination, sort, filter, and export all rest on it, and the current
> `FREYA_STATE_ARCHITECTURE.md` §6 query model (`QuerySpec { sql, page, epoch }`) doesn't capture it.
> The round-trip and the snapshot are one task: a Run *is* what materializes the snapshot, so you
> can't build one without the other.

## Goal
Running a tab's SQL executes on the engine, flows back through **freya-query**, and materializes a
**stable snapshot** of the result that pagination / sort / filter / export all read. Per state-arch
§2/§6 there is **no `runs`-by-id store and no query state on the session** — the results element owns
the query.

## The snapshot problem
Keying results by raw `QuerySpec { sql, page }` is unsafe and insufficient:
- **Freshness:** a spec doesn't know whether the underlying files changed, so "same sql+page → same
  data" is **not** a valid cache guarantee.
- **Stable paging:** re-running the SQL per page can page over *shifted* data.
- **Sort / filter / export** must operate over a fixed set, not re-run the whole SQL each time.

So a **Run** materializes a snapshot (the Dioxus app used an on-disk `__snap_*` table — carry that
forward) and returns a **handle** (id + schema + row count). Reads then target *that snapshot*:
pagination `{ snapshot_id, page }`, sort `{ …, sort }`, filter `{ …, filter }`, export streams it.
Because a snapshot is **immutable**, freya-query can safely cache the *reads* keyed by
`(snapshot_id, page, sort, filter)` — that's the correct cache key, not the raw SQL. A **new Run**
makes a new snapshot (old one retired); DDL / reload retires snapshots (the `epoch`).

## Current state
`contexts/engine_ctx.rs` only exposes `send(cmd)`. No request/reply demux, no query capability, no
event router, no snapshot.

## Build (state-arch §6/§7/§9)
1. **Spec** the snapshot in `docs/SNAPSHOT_SPEC.md`: creation on Run, the handle shape, read ops
   (page/sort/filter/export), lifecycle + cleanup (per-tab vs shared, disk vs memory, drop on tab/
   project close), invalidation. Reconcile with `__snap_*` + the engine `Command`/`Event` protocol.
   Update state-arch §6 to match.
2. **`EngineCtx` demux** (→ `contexts/ctx.rs`): `async fn query(&self, spec) -> Result<QueryPage,
   String>` allocating a `ReqId`, registering a `oneshot::Sender` in
   `pending: Arc<Mutex<HashMap<ReqId, …>>>`, sending the command, awaiting the oneshot. Add
   `captured()` (`Captured<EngineCtx>`) and `take_evt_rx()`.
3. **Event router**: a Freya `spawn` in the project root drains `evt_rx`; `Event::QueryResult
   { req_id, result }` completes the matching `oneshot` (logs `Err`); other events feed log/signals.
   Never `tokio::spawn` for UI writes.
4. **`apps/project/query/run_query.rs`**: `QuerySpec` (mode `Run|Plan|Explain` + `window`/`epoch`),
   `QueryPage`, `RunQuery(Captured<EngineCtx>): QueryCapability`. A Run produces the snapshot; page/
   sort/filter reads are keyed by `(snapshot_id, page, sort, filter)`.
5. Engine: Run materializes a snapshot + handle; add read commands over `snapshot_id`.
6. The Tokio runtime entered in `main` backs the `oneshot` bridge (plan §9).

## Acceptance
- [ ] `docs/SNAPSHOT_SPEC.md` exists and is agreed; state-arch §6 updated.
- [ ] `EngineCtx::query(spec).await` resolves; the drain completes oneshots by `req_id` under Freya
      `spawn`; no `tokio::spawn` writes UI state.
- [ ] Run materializes a snapshot; paging/sort/filter/export read the *same* snapshot (stable).
- [ ] Re-reading the same `(snapshot_id, page, sort, filter)` is cache-served; a new Run makes a new
      snapshot (raw-SQL identity is **not** the cache key).

## Freya / references
- `FREYA_STATE_ARCHITECTURE.md` §2 (no runs store), §6 (freya-query), §7 (demux + router), §9 (module
  layout). Plan §4 (invalidation), §9 (Tokio bridge). Freya skill "freya-query".
- Core `Command`/`Event`; the Dioxus `__snap_*` snapshot + `FetchPage.sort` (DEV_TASKS Rz6/D5).
