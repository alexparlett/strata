# Result snapshots — the query round-trip's stable read model

What a Run materializes, what identifies it, what reads it, and when it dies.
`FREYA_STATE_ARCHITECTURE.md` §6 is this model's summary.

> **Engine boundary note.** The engine boundary is a **direct-call async facade** (§5): the
> engine owns a private Tokio runtime and exposes plain async methods, which freya-query
> capabilities await directly — the shape freya-query is built for. (The Dioxus-era
> `Command`/`Event` channel protocol it replaced was removed with the Dioxus app.)

---

## 1. Why a snapshot

Keying results by raw SQL is unsafe and insufficient:

- **Freshness** — the same SQL over the same tables can read *different files* a second later.
  "Same sql → same data" is not a cache guarantee, so raw-SQL identity must never be a cache key.
- **Stable paging** — re-running the SQL per page can page over shifted data (rows inserted,
  files compacted) and show row 101 twice or never.
- **Sort / filter / export** must operate over a *fixed set*, not re-run the query each time.

So a **Run executes the SQL exactly once** and spools the full result to an on-disk **Arrow IPC
snapshot** (LZ4-compressed). Every later read —
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
- file: `<tmp>/strata_snapshots/e_{pid}_{engine_id}/s_{id}.arrow` (pid-scoped: engine ids
  are only process-unique, and the temp root is machine-shared)
- lock: `<tmp>/strata_snapshots/e_{pid}_{engine_id}.lock` — a **sibling** of the directory,
  opened and exclusively locked by `Engine::new` and held open for the engine's whole life

Because every *execution* allocates a fresh id, snapshot ids are never reused — a re-run of
identical SQL produces a **new** snapshot. There is deliberately no sharing between two tabs
running the same SQL: sharing by SQL identity is exactly the freshness bug in §1.

Each window's engine only ever touches its own subdirectory. The **lock file** is what makes that
survive a *second process*: a pid can be recycled, so the directory name alone never proves its
owner is alive, but an advisory lock is released by the OS on exit or crash and by nothing else.
So the startup sweep is **selective**, not a `remove_dir_all` of the root:

- claim order is lock-then-`mkdir`, so any directory a concurrent sweep can see already has a
  held lock — a starting engine never looks abandoned (a lock file *inside* the directory would
  have exactly that window);
- `purge_snapshot_root()` deletes a directory only when it can *take* that directory's lock.
  A lock held by a live engine (another instance, a parallel test binary) means skip. A lock it
  can neither take nor find held — an unwritable root, a filesystem with no working advisory
  locking — also means skip, but is logged: nothing is deleted on a guess, because deleting a
  running instance's results is worse than leaking temp files, and a sweep that can never resolve
  anything must not fail invisibly;
- anything under the root that is neither an `e_*` directory nor its `e_*.lock` file is a stray
  and is removed.

An engine whose own claim fails still runs (the directory is created on demand) but is
unprotected against another instance's sweep — `Engine::new` warns with the reason.

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
type-aware page-1 `RecordBatch` rides alongside in the return value.

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
| **engine drop** (window close) | the engine's whole `e_{pid}_{engine_id}` directory + its `.lock` sibling |
| **last `SnapshotPin` released** | a snapshot whose retire arrived while it was pinned (see below) |
| **process start** | `purge_snapshot_root()` — every *dead* engine's leftovers (§2: lock-gated, live directories spared) |

**Retire-on-dispatch**: the previous snapshot is dropped when the new Run *starts*, not when it
succeeds — one lock owns the whole lifecycle, never held across an await. During the run — and
after a failed run — uncached page reads of the old snapshot fail; the UI's already-cached pages
are unaffected (§6), and the pane is in its Running / Error state anyway. A run that finishes
*after* being superseded retires its own snapshot and settles `Err("superseded")` — nothing
leaks, and only the latest dispatch may publish workspace state.

**Pins defer a retire, never skip it.** Retire-on-dispatch is right for the grid, whose pages
follow the tab, and wrong for any reader that outlives one press. `Engine::pin_snapshot(id)`
returns a `SnapshotPin` (RAII — dropping it releases): while at least one pin is out, a retire of
that snapshot is recorded in `deferred` instead of executed, and lands when the last pin drops.
Pins are counted, so two holders are independent.

The export window is the canonical holder and the reason this exists: it is opened *on a
result*, the user may go back and re-run the query while it sits there, and it must still write
the rows that were on screen when they asked. Without a pin a re-run deregisters the table
mid-`COPY` — a truncated file under the user's chosen name — or, more quietly, makes a later
Export report no results at all when there are plainly some on screen. `Engine::export` also
brackets its own call with a pin, so the facade is correct for a caller with no window.

Two retires deliberately **bypass** the deferral, because nothing can be holding their subject: a
run's own partial, and a superseded run's output. Neither id is ever returned to a caller.

**DDL / catalog changes do not retire snapshots.** A snapshot is a point-in-time result
(Athena-style): dropping a table or reloading the catalog doesn't invalidate what a past Run
returned. This retires the `epoch` field from the query key — with per-Run identity (§5) there is
nothing for an epoch to invalidate: catalog freshness is the `ProjectState` store's concern (it is
a store, not a query — see `FREYA_STATE_ARCHITECTURE.md` §6), result freshness is the user's Run
button.

Disk, not memory: RAM holds one page regardless of result size.

## 5. The engine facade

The engine (`strata_core::engine::Engine`) is a **direct-call async facade**: it owns a private
multi-thread Tokio runtime (DataFusion's operators require a Tokio context, and query CPU must
never run on the render thread), spawns each call onto it, and awaits the `JoinHandle` — which is
executor-agnostic, so Freya's non-Tokio UI executor awaits engine calls like any async fn. No
channels, no event stream, and no request ids *crossing the boundary* — the caller awaits its own
call's return value (the engine's private dispatch id, below, is bookkeeping the UI never sees).

```rust
// The editor's entry point: classify the statement, then run a query (delegating to
// `query` byte-for-byte), execute an intercepted statement, or refuse it — the
// statement router, docs/STATEMENTS_SPEC.md.
async fn run(ws: WsId, tag: RunTag, sql, page_size) -> Result<RunOutcome, String>

// Run's Query arm: execute once → spool a fresh snapshot → page 1 + handle back.
async fn query(ws: WsId, tag: RunTag, sql, page_size) -> Result<(QueryOutput, RecordBatch), String>

// Read: bounded LIMIT/OFFSET (+ optional whole-snapshot ORDER BY) over one snapshot.
async fn fetch_page(snapshot, page, page_size, sort: Option<(String, bool)>)
    -> Result<(Vec<Vec<Cell>>, RecordBatch), String>

// Explain: parsed plan tree, no snapshot.
async fn explain(ws: WsId, tag: RunTag, sql) -> Result<QueryPlan, String>

// Lifecycle: cancel is scoped to the run `tag`, so a stale cancel can't abort a
// just-started newer run; cleanup_ws is the tab-close hook; Drop clears everything.
fn cancel(ws, tag) -> Option<elapsed_ms> · fn cleanup_ws(ws) · impl Drop
```

`RunTag` is the UI's per-press nonce (§6) passed down; `WsId` is wide enough (`u128`) to carry
each frontend's native tab id.

**Two identities, and they are not interchangeable.** `RunTag` names *the run the caller can
see*, so it is what `cancel` matches on: a Cancel press means "stop the run I'm looking at". It
is **not** unique engine-side — freya-query re-runs an entry when a subscriber remounts while it
is still in flight, so one logical run can be dispatched twice under the same tag. Supersede
checks therefore key on `InFlight::dispatch`, an engine-private monotonic id from
`Engine::dispatch_seq` allocated per `query`/`explain` call. Keying them on the tag instead let
the first call's settle path adopt the *second* call's `InFlight` entry, tear down a perfectly
good run and fail both. (This is the one request-id-shaped thing in the facade, and it is
deliberately engine-internal: it never crosses the boundary, has no UI representation, and
replaces nothing the tag can do.)

`sort` stays a read-time parameter (an `ORDER BY` over the whole snapshot before the page
window), never a rewrite of the snapshot. An **unsorted** read is `ORDER BY` the row
ordinal (§9) — a bare `LIMIT/OFFSET` over the registered table has **no** inherent order, and
above the scan-split threshold it is measured-nondeterministic. **Export** is `Engine::export`
over `export::run_export`, streaming from one snapshot. The facade grows one method per
feature, always as a read of the immutable snapshot (a filter, should one ever land, is a
`WHERE` in the read key — never a rewrite); the logic lives in the engine's submodules as plain
async functions.

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
QueryOutcome::Statement(StatementReport)   // mode: Run, intercepted statement — no snapshot,
                                           // and none retired (docs/STATEMENTS_SPEC.md)

// A page read — targets one immutable snapshot. THIS is the safe cache key.
PageSpec {
    snapshot: SnapshotId,
    page: usize,
    page_size: usize,
    sort: Option<(String, bool)>,
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

Cache-entry **lifetime is subscriber presence**, so a press must keep a subscriber for as long
as it is some tab's `request` — the results element alone can't provide that (it mounts only for
the *active* tab, and freya-query cleans an unsubscribed entry after `clean_time`, which for a
backgrounded run would mean a silent re-execution on revisit). The workbench's **request
keepers** (`views::keeper`, one invisible pin per open tab's current press) are that
subscriber: while a press stays current its entry is held live; once superseded / cancelled /
its tab closed, the pin unmounts and the entry ages out. Two supporting rules: every Run
subscription is built through `QuerySpec::query` (a `Query`'s settings are part of its cache
identity — a hand-built variant is a *different* entry, i.e. a duplicate execution), and
freya-query itself (fork) never cleans an entry whose execution is still in flight, so even an
eviction can't orphan a running press into a duplicate dispatch.

Page reads are keyed `(snapshot, page, page_size, sort)` — all reads of an immutable set, so
cache hits are sound forever: a revisited page renders with **zero** engine traffic. Reads of a
retired snapshot fail cleanly (table's gone) — reachable only through a stale subscriber, since
a new Run hands the UI a new handle and the old `PageSpec`s die with their subscribers.

Run flow end-to-end:

1. Run press: `request.set(Some(QuerySpec { tab, run: RunId::new(), sql: editor_text, mode: Run, page_size }))`.
2. Results element (and the tab's keeper pin): `use_query(spec.query(&engine))` →
   `Pending/Loading` renders Running.
3. `RunQuery::run` → `engine.run(tab.into(), run.into(), sql, page_size).await` — the direct
   facade call (§5, the statement router; a `SELECT` reaches `query` one match arm further in);
   the query settles; grid renders page 1 from `QueryOutput` + holds the handle.
4. Paging/sort: the grid drives `use_query(FetchSnapshotPage)` with
   `PageSpec { snapshot: handle, … }` — fetched once per distinct key, cache-served after.
5. Cancel: `engine.cancel(tab.into(), run.into())` — the awaiting run settles `Err("cancelled")`.

The workbench owns the `request` slot and hands it down as props to the toolbar and results
pane (placement rationale in `FREYA_STATE_ARCHITECTURE.md` §6).

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

Two shapes were considered and rejected, and the reasons still constrain the design:

- **Keying results by SQL** (`QuerySpec { sql, page, epoch }`): raw-SQL identity is the
  freshness bug of §1. `page` belongs on the read (`PageSpec`), and with per-Run identity there
  is nothing for an `epoch` to invalidate (§4). A per-window discriminator adds nothing either —
  nonce + snapshot ids are process-unique.
- **Snapshot naming by workspace** (`__snap_{ws}`): a re-run rewrites the name in place under
  any reader. Per-run identity (§2) keeps every snapshot immutable; the ws keeps only
  *ownership* of its current snapshot for lifecycle (§4).

## 9. Row order — the ordinal column

**A snapshot read has no order of its own, and pretending otherwise is a measured bug.** The
snapshot is registered as an Arrow *File* table, and DataFusion range-splits such a file across
`target_partitions` once it passes `datafusion.optimizer.repartition_file_min_size` (10 MB
default). Any read without an `ORDER BY` then sits above a `CoalescePartitionsExec`, whose own
contract is that output order is arbitrary. Measured on stock config, `fetch_page` with no sort:

| Snapshot | Behaviour |
|---|---|
| 3M rows (`i`, `md5` text) | **Unstable**: the same page re-read returns different rows — page 1 came back starting at row 1,843,201 on one read and row 101 on another |
| 200k rows (`i`, `md5` text — the file already crosses 10 MB) | Stable but **wrong**: every read starts the stream at row 57,345, so pages 2+ (served by `fetch_page`) disagree with page 1 (served from the spool, in true order) — rows duplicated and missing as the user pages |
| 200k rows, narrow (`i` only, under 10 MB) | Perfect file order — which is why this never showed up in tests |

Each read is a *contiguous* run of the file (within-page order survives); it is the stream's
starting partition that races. The failure is invisible below the threshold and total above it —
and the freya-query page cache then **freezes** whichever answer a read happened to get, so two
views can hold contradictory copies of one page. §1 promises stable paging; without an order key
the read path cannot deliver it.

**The fix: order is a column, written by the writer as it spools.** `materialize` appends
`__strata_ord` — a **UInt64, 1-based** column (nothing reads its values, only their order) — to
each batch on its way into the IPC file, numbered from the count already written, and **after**
`QueryOutput::columns` is captured, so the user-visible schema never contains it. The value is
therefore the row's literal position in the file, which is precisely what every reader's
`ORDER BY __strata_ord` means: the property holds by construction rather than by measurement,
and the regression suite below pins it as confirmation. If the result already has a column of
that name, the name escalates by prefix (`___strata_ord`, …) until free — the chosen name rides
in the write pass's `SnapshotStats`, beside the null counts, with exactly the snapshot's
lifetime.

*Tried in between, and withdrawn:* for a time this was `row_number() OVER ()` added to the plan
(`with_column`) rather than to the batches. It was adopted because review asked why the order
was not simply part of the query, and it was measured to hold — contiguous across 3M rows on the
racy over-threshold shape, a user's `ORDER BY` preserved beneath the window, and no spool cost,
since the plan kept its `RepartitionExec` and only the numbering rode the merged stream.

What that measurement could not cover is a *federated* result (DB-02). A plan-level window is
the **query's** to evaluate, so a read over a remote database had Strata's snapshot bookkeeping
pushed across the wire for Postgres to compute — numbering the remote result rather than the
stream the writer consumes, which is the one thing the ordinal is defined as. It also dragged
the scan into DataFusion 54's unparser along a derived-table path that does not rebase outer
column qualifiers, so the generated statement named a relation its own `FROM` had aliased away
and **every** federated read failed with Postgres's `42P01` — a defect in the SQL we emit, not
in anything Postgres refuses. Numbering at write time cannot reach a plan, an optimizer rule or
an unparser at all, for that database arm or any later one, and it makes the ordinal's
definition tautological instead of measured. The window's one genuine advantage — being visible
to the planner — was never used by anything.

**Two plans spool without an ordinal** (`SnapshotStats.ord: None`), and their reads are
unordered exactly as every snapshot's were before ordinals existed — both are small or
degraded shapes where that is the honest behavior:

- An `EXPLAIN` / `EXPLAIN ANALYZE`. This was once a hard constraint — DataFusion requires those
  at the plan root, so the window that used to carry the ordinal failed the whole run, and the
  statement router's Query arm promises the editor can run them (`docs/STATEMENTS_SPEC.md`).
  Numbering at write time removes the constraint; the exclusion stays because it would buy such
  a result nothing — a handful of plan rows, nowhere near the split threshold.
- A result with **duplicate column names** (`SELECT a.i, b.i FROM … JOIN …`): the registered
  table resolves columns by name, so a typed column appended after two same-named ones made
  every later read mis-map the second onto the ordinal's slot and fail. Ordinal-less, such a
  result reads as it did at base — degraded by the duplicate, but readable.

**The registration declares the order.** The snapshot registers as a listing table with
`with_file_sort_order` on the ordinal — a promise about the file that is exactly what the
single-stream spool constructs and this section's regression suite pins. Declared, an ordered
read **plans as a stream**: measured on a 3M-row / 157 MB snapshot, a page at offset 2.9M is
543 ms as an undeclared TopK (holding every candidate row in memory, no spill path) and 97 ms
declared, shallow pages plan as scan-level limit pushdown with the sort elided, and a
whole-snapshot export streams into its `COPY` instead of buffering the result first.

The discipline the column demands — every reader accounts for it:

- **Unsorted reads** (`fetch_page`, the chart's `Rows`): `ORDER BY __strata_ord`, then project
  it away. (Scatter's `Raw` and the histogram deliberately read unordered — `ChartData::Points`
  is documented orderless, a scatter draws marks, and a histogram's bins are order-free.)
- **Sorted reads**: the user's `ORDER BY` gets the ordinal appended as the **tie-break**, making
  sorts stable across page windows — ties were the same nondeterminism one layer down.
- **Export** (`select_sql`): selects the result's columns explicitly, never `SELECT *` — a
  `COPY` must not write bookkeeping into the user's file.

Cost: 8 bytes/row uncompressed; a monotonic sequence is near-free under the LZ4 the snapshot
already uses. Written during the spool pass that is already streaming every batch.
