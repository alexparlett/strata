# Result snapshots — the query round-trip's stable read model

The design for **P2-01**: what a Run materializes, what identifies it, what reads it, and when it
dies. Supersedes the `QuerySpec { sql, page, epoch }` sketch in `FREYA_STATE_ARCHITECTURE.md` §6
(that section now matches this spec).

> **Engine boundary note.** This work also replaced the Dioxus-era `Command`/`Event` channel
> protocol with a **direct-call async facade** (§5): the engine owns a private Tokio runtime and
> exposes plain async methods, which freya-query capabilities await directly — the shape
> freya-query is built for. The retired protocol lives on only in `crates/strata-dioxus`
> (reference code; no longer builds).

---

## 1. Why a snapshot

Keying results by raw SQL is unsafe and insufficient:

- **Freshness** — the same SQL over the same tables can read *different files* a second later.
  "Same sql → same data" is not a cache guarantee, so raw-SQL identity must never be a cache key.
- **Stable paging** — re-running the SQL per page can page over shifted data (rows inserted,
  files compacted) and show row 101 twice or never.
- **Sort / filter / export** must operate over a *fixed set*, not re-run the query each time.

So a **Run executes the SQL exactly once** and spools the full result to an on-disk parquet
**snapshot** (the `__snap_*` mechanism carried forward from the Dioxus app). Every later read —
page, sort, filter, export — is a bounded read *of that snapshot*, and the snapshot is
**immutable**: once materialized it is never rewritten. Immutability is what makes downstream
caching sound.

## 2. Identity

```
SnapshotId(u64)        — strata-model::results
```

A snapshot's id comes from the engine's own monotonic allocator — unique per engine for the life
of the process. It is the snapshot's identity and its storage name:

- table: `__snap_{id}` (registered in the engine's `strata.public` schema)
- file: `<tmp>/strata_snapshots/e_{pid}_{engine_id}/s_{id}.parquet` (pid-scoped: engine ids
  are only process-unique, and the temp root is machine-shared)

Because every *execution* allocates a fresh id, snapshot ids are never reused — a re-run of
identical SQL produces a **new** snapshot. (This deliberately drops state-arch §6's "two tabs
running the same spec share a cache entry": sharing by SQL identity is exactly the freshness bug
in §1.)

The `engine_id` directory scoping is unchanged: each window's engine only ever touches its own
subdirectory; `purge_snapshot_root()` at process start clears leftovers from a crashed run.

## 3. The handle

A successful Run answers with the snapshot **handle riding inside `QueryOutput`**:

```rust
QueryOutput {
    snapshot: Option<SnapshotId>,  // None ⇔ the query produced zero rows (nothing materialized)
    columns:  Vec<ColumnInfo>,     // the result schema
    total:    usize,               // exact row count (counted while spooling — no COUNT(*) pass)
    rows / page / page_size / elapsed_ms,   // page 1, delivered with the run
}
```

id + schema + row count — plus page 1, so the grid renders without a follow-up read. The
type-aware page-1 `RecordBatch` rides alongside in the event (unchanged from today).

An **empty result registers no snapshot** (`snapshot: None`, `total: 0`); there are no pages to
read, and the UI has the schema from `columns`.

## 4. Ownership & lifecycle

A snapshot belongs to the **workspace** (`WsId` — the query tab that ran it; the Freya `TabId`
converts directly, so the tab *is* the workspace). The engine keeps the only bookkeeping, under
one lock: `current: HashMap<WsId, SnapshotId>` + the in-flight run per workspace.

Retirement (deregister the table + delete the file) happens at exactly these points:

| Trigger | What retires |
|---|---|
| **New Run for the ws** (dispatch time) | the ws's previous snapshot + any in-flight run's partial |
| **`cancel(ws, tag)`** | the aborted run's partial file; the previous snapshot is already gone (retire-on-dispatch) |
| **Run fails** | the failed run's partial file (cleaned by the run itself) |
| **`cleanup_ws(ws)`** (tab close) | the ws's current snapshot + any in-flight partial |
| **engine drop** (window close) | the engine's whole `e_{pid}_{engine_id}` directory |
| **process start** | `purge_snapshot_root()` — all engines' leftovers from a previous crash |

**Retire-on-dispatch**: the previous snapshot is dropped when the new Run *starts*, not when it
succeeds — one lock owns the whole lifecycle, never held across an await. During the run — and
after a failed run — uncached page reads of the old snapshot fail; the UI's already-cached pages
are unaffected (§6), and the pane is in its Running / Error state anyway. A run that finishes
*after* being superseded retires its own snapshot and settles `Err("superseded")` — nothing
leaks, and only the latest dispatch may publish workspace state.

**DDL / catalog changes do not retire snapshots.** A snapshot is a point-in-time result
(Athena-style): dropping a table or reloading the catalog doesn't invalidate what a past Run
returned. This retires the `epoch` field from the query key — with per-Run identity (§5) there is
nothing for an epoch to invalidate: catalog freshness is `FetchCatalog`'s concern, result
freshness is the user's Run button.

Disk, not memory: RAM holds one page regardless of result size (unchanged).

## 5. The engine facade

The engine (`strata_core::engine::Engine`) is a **direct-call async facade**: it owns a private
multi-thread Tokio runtime (DataFusion's operators require a Tokio context, and query CPU must
never run on the render thread), spawns each call onto it, and awaits the `JoinHandle` — which is
executor-agnostic, so Freya's non-Tokio UI executor awaits engine calls like any async fn. No
channels, no request ids, no event stream.

```rust
// Run: execute once → spool a fresh snapshot → page 1 + handle back.
async fn query(ws: WsId, tag: RunTag, sql, page_size) -> Result<(QueryOutput, RecordBatch), String>

// Read: bounded LIMIT/OFFSET (+ optional whole-snapshot ORDER BY) over one snapshot.
async fn fetch_page(snapshot, page, page_size, sort: Option<(String, bool)>)
    -> Result<(Vec<Vec<Cell>>, RecordBatch), String>

// Explain: parsed plan tree, no snapshot.
async fn explain(ws: WsId, tag: RunTag, sql) -> Result<QueryPlan, String>

// Lifecycle: cancel is scoped to the dispatch `tag` (S14 — a stale cancel can't abort a
// just-started newer run); cleanup_ws is the tab-close hook; Drop clears everything.
fn cancel(ws, tag) -> Option<elapsed_ms> · fn cleanup_ws(ws) · impl Drop
```

`RunTag` is the UI's per-press nonce (§6) passed down, so "is this still the run I mean" needs no
parallel request-id scheme. `WsId` is wide enough (`u128`) to carry each frontend's native tab id.

`sort` stays a read-time parameter (an `ORDER BY` over the whole snapshot before the page
window — Rz6), never a rewrite of the snapshot. **Filter** (P2-09/P2-13's find/filter work)
extends `fetch_page` the same way when it lands: a `WHERE` over the snapshot, part of the read
key, snapshot untouched. **Export** (its own task) adds `Engine::export` over `run_export`,
streaming from one snapshot. The facade grows one method per feature; the logic lives in the
engine's submodules as plain async functions.

## 6. The UI layer (freya-query)

Two capabilities in `apps/project/query/run_query.rs`, both carrying the engine handle as
`Captured<EngineCtx>` (invisible to cache identity):

```rust
// The Run — executes SQL. Keyed by a per-click nonce, NOT by the SQL.
QuerySpec {
    tab:  TabId,        // the workspace it runs in (tab == engine WsId)
    run:  RunId,        // fresh Uuid per Run press — the cache identity (→ the engine's RunTag)
    sql:  String,       // what to execute (a snapshot of the editor text at press time)
    mode: QueryMode,    // Run | Explain { analyze } — Explain returns a plan, materializes nothing
    page_size: usize,
}
RunQuery(Captured<EngineCtx>): QueryCapability<Keys = QuerySpec, Ok = QueryOutcome, Err = String>

QueryOutcome::Rows(QueryPage { output: QueryOutput, batch: RecordBatch })   // mode: Run
QueryOutcome::Plan(QueryPlan)                                               // mode: Explain

// A page read — targets one immutable snapshot. THIS is the safe cache key.
PageSpec {
    snapshot: SnapshotId,
    page: usize,
    page_size: usize,
    sort: Option<(String, bool)>,
    // filter joins here when P2-09/13 land
}
FetchSnapshotPage(Captured<EngineCtx>): QueryCapability<Keys = PageSpec, Ok = SnapshotPage, Err = String>
```

Why the nonce: a Run is an **action**, not a fetch — pressing Run must execute, and *only*
pressing Run may execute. `RunId` gives every press its own cache entry, so:

- Remounting the results element (tab switch and back) re-reads the cached `QueryOutcome` —
  it does **not** re-execute the SQL.
- Pressing Run again builds a new `QuerySpec` (new nonce) → a genuine new execution → a new
  snapshot; the old spec's entry dies with its subscribers (freya-query `clean_time`).
- Both run/page queries set `stale_time(MAX)`: a settled entry never re-runs by itself. This
  matters — freya-query re-runs stale entries on resubscribe, and an uncontrolled re-execution
  would silently re-materialize under a *new* snapshot while cached pages still described the
  old one.

Page reads are keyed `(snapshot, page, page_size, sort)` — all reads of an immutable set, so
cache hits are sound forever: a revisited page renders with **zero** engine traffic. Reads of a
retired snapshot fail cleanly (table's gone) — reachable only through a stale subscriber, since
a new Run hands the UI a new handle and the old `PageSpec`s die with their subscribers.

Run flow end-to-end:

1. Run press: `request.set(Some(QuerySpec { tab, run: RunId::new(), sql: editor_text, mode: Run, page_size }))`.
2. Results element: `use_query(Query::new(spec, RunQuery(engine.captured())).stale_time(MAX))` →
   `Pending/Loading` renders Running.
3. `RunQuery::run` → `engine.query(tab.into(), run.into(), sql, page_size).await` — the direct
   facade call (§5); the query settles; grid renders page 1 from `QueryOutput` + holds the handle.
4. Paging/sort: the grid drives `use_query(FetchSnapshotPage)` with
   `PageSpec { snapshot: handle, … }` — fetched once per distinct key, cache-served after.
5. Cancel: `engine.cancel(tab.into(), run.into())` — the awaiting run settles `Err("cancelled")`.

## 7. `EngineCtx` — the window's handle

A thin per-window context wrapper — `Arc<Engine>` with `Deref`, plus the only UI-shaped pieces:

```rust
EngineCtx { eng: Arc<Engine> }          // Deref → Engine: call the facade directly
impl From<TabId> for WsId               // the tab IS the workspace (Uuid → u128)
EngineCtx::captured() -> Captured<EngineCtx>   // capability field, invisible to cache identity
EngineCtx::cleanup(tab)                 // → engine.cleanup_ws — the tab-close hook for §4
```

Tab-close cleanup is one funnel: a `use_side_effect` in the window root diffs the session's open
tab set on every structural change and calls `cleanup` for tabs that disappeared — every close
path (close / close-others / close-right / close-all) is covered without touching any of them.
**No UI-side Tokio runtime** anywhere: `main` stays runtime-free; the engine's is private.

## 8. What this replaces

- The Dioxus runs-by-id store and its hand-rolled page cache — freya-query owns caching.
- The Dioxus-era `Command`/`Event` channel protocol + worker loop and the router/oneshot demux
  design that bridged it into freya-query — replaced whole by the direct facade (§5). The
  protocol survives only in `crates/strata-dioxus` (reference; no longer builds).
- `QuerySpec { sql, page, epoch }` (state-arch §6, superseded): `page` moved into `PageSpec`
  reads, `epoch` retired (§4), `window` dropped — nonce + snapshot ids are process-unique, so a
  per-window discriminator adds nothing.
- Snapshot naming by `ws_id` (`__snap_{ws}` / `ws_{ws}.parquet`) — replaced by per-run identity
  (§2); the ws keeps only *ownership* of its current snapshot for lifecycle (§4).
