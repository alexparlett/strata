# P2-01 · Query round-trip + result snapshot system (design + build)

**Phase:** 2 — Workbench · **Status:** ✅ **built** · **Depends on:** — · **Unblocks:** most of Phase 2

> **Design amended during build** (agreed in-session): the original steps here prescribed an
> `EngineCtx` oneshot demux + event router bridging the Dioxus-era `Command`/`Event` protocol into
> freya-query. That protocol was **retired instead** — the engine is now a **direct-call async
> facade** (freya-query's native shape; cf. Freya's `state_query_sqlite` example), and
> `strata-dioxus` is reference code that no longer builds. `docs/SNAPSHOT_SPEC.md` is the design
> of record; state-arch §6/§7 updated to match.

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

So a **Run** materializes an immutable snapshot (the `__snap_*` mechanism carried forward) and its
handle (`SnapshotId` + schema + row count) rides back in `QueryOutput`. Reads then target *that
snapshot*, cache-keyed by `(snapshot, page, page_size, sort)`. A new Run makes a new snapshot (old
one retired); raw-SQL identity is never a cache key.

## What was built
1. **`docs/SNAPSHOT_SPEC.md`** — the agreed design: identity (§2), the handle (§3), lifecycle +
   retire rules (§4), the engine facade (§5), the freya-query layer (§6), `EngineCtx` (§7).
2. **`strata-model`** — `SnapshotId` newtype; `QueryOutput.snapshot: Option<SnapshotId>` (`None` ⇔
   empty result, nothing materialized).
3. **`strata-core::engine`** — the `Command`/`Event` protocol + worker loop **deleted**; replaced by
   the direct facade `Engine` (private multi-thread Tokio runtime; calls spawn onto it and the UI
   awaits the executor-agnostic `JoinHandle`): `query(ws, tag, sql, page_size)` /
   `fetch_page(snapshot, page, page_size, sort)` / `explain(ws, tag, sql)` / `cancel(ws, tag)` /
   `cleanup_ws(ws)` / `register(spec)` + `Drop` cleanup. Lifecycle bookkeeping (supersede,
   retire-on-dispatch, partial cleanup) lives under one lock in the facade. Snapshots are keyed
   `s_{SnapshotId}.parquet` / `__snap_{SnapshotId}` — per-run identity, immutable.
4. **`strata-freya`** — `EngineCtx` = `Arc<Engine>` + `Deref` + `captured()` + `cleanup(tab)`;
   `TabId → WsId` directly (the tab **is** the workspace — no parallel id). Capabilities in
   `apps/project/query/run_query.rs`: `RunQuery` (keyed by `QuerySpec { tab, run: RunId nonce, sql,
   mode, page_size }`, settling `QueryOutcome::Rows|Plan`) and `FetchSnapshotPage` (keyed by
   `PageSpec`). Tab-close cleanup: one root `use_side_effect` diffs the open-tab set. `main` calls
   `purge_snapshot_root()`; no UI-side Tokio.
5. **Tests** — `strata-core/tests/engine_round_trip.rs` (6: run→page→sort, re-run retires, ws
   independence + cleanup, empty result, failure/DDL-block, cancel scoping) and headless capability
   tests in `run_query.rs` (2: run→page→sorted-page, explain→plan) driven by `block_on` in place of
   the UI executor.

## Acceptance
- [x] `docs/SNAPSHOT_SPEC.md` exists and is agreed; state-arch §6/§7 updated.
- [x] freya-query capabilities await the engine facade directly; no UI-side runtime; no
      `tokio::spawn` writes UI state (nothing crosses threads except the awaited `JoinHandle`).
- [x] Run materializes a snapshot; paging/sort read the *same* snapshot (stable); export/filter key
      off it when their tasks land.
- [x] Re-reading the same `(snapshot, page, page_size, sort)` is safe to cache-serve (immutable
      snapshot); a new Run makes a new snapshot (raw-SQL identity is **not** the cache key).

## Follow-ups (their own tasks)
- P2-02 wires `use_query` into the results pane (the query layer is `#[allow(dead_code)]`-marked
  until then); P2-03 feeds the grid from `QueryPage`.
- Facade methods for refresh-catalog / views / profile / export / set-config land with their
  features (the logic fns are kept in the engine submodules as marked feature reservoirs).

## Freya / references
- `docs/SNAPSHOT_SPEC.md` (design of record) · `FREYA_STATE_ARCHITECTURE.md` §2/§6/§7.
- Freya `examples/state_query_sqlite` — the direct-call capability idiom this follows.
