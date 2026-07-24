//! The DataFusion engine — a **direct-call async facade** over a runtime it owns.
//!
//! [`Engine`] holds the `SessionContext` plus a private multi-thread Tokio runtime:
//! every call spawns its work onto that runtime and awaits the `JoinHandle`, which is
//! executor-agnostic — so a non-Tokio UI executor (Freya's) awaits engine calls
//! directly, the way a freya-query capability expects, while DataFusion's own
//! parallelism runs on the engine's threads and never on the render thread.
//!
//! Pagination model (bounded memory): each query is executed **once** and its full
//! result is spooled to an immutable on-disk parquet **snapshot**, keyed by
//! [`SnapshotId`] (`docs/SNAPSHOT_SPEC.md`). Every page is a bounded `LIMIT/OFFSET`
//! read of that snapshot — RAM only ever holds one page, and no query is recomputed
//! per page. The engine also owns the snapshot **lifecycle**: a re-run for the same
//! workspace retires the previous snapshot at dispatch, cancel/cleanup retire
//! partials, and dropping the engine clears its whole snapshot directory.
//!
//! The facade grows one method per feature that lands in the Freya app; the
//! underlying logic lives in the sibling modules (`query`, `explain`, `catalog`,
//! `export`, `profile`) as plain async functions over `&SessionContext`.
//!
//! (The retired `Command`/`Event` channel protocol this replaces lives on only in
//! `crates/strata-dioxus`, which is reference code and no longer builds.)

mod catalog;
mod explain;
mod export;
mod query;
pub mod config;
pub mod serialize;
pub mod plan;
pub mod sql;
pub mod profile;

pub use catalog::{TableMeta, TableSpec, ViewMeta};
pub use query::purge_snapshot_root;

/// The Arrow batch type engine results carry (the type-aware source for Copy/Export),
/// re-exported so frontends can name it without their own DataFusion dependency (this
/// crate is the one DataFusion boundary).
pub use datafusion::arrow::record_batch::RecordBatch;

/// The Arrow schema type, re-exported for the same reason — code (and tests) holding a
/// [`RecordBatch`] sometimes needs to name its schema.
pub use datafusion::arrow::datatypes::Schema;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use datafusion::prelude::*;
use tokio::task::AbortHandle;

use crate::engine::plan::QueryPlan;
use query::{retire_snapshot, run_and_snapshot, snapshot_dir, CellFormat};
use sql::FunctionCatalog;
use strata_model::{Cell, QueryOutput, SnapshotId};

/// A workspace's stable identity — the query tab that owns a run and its current
/// snapshot (`docs/SNAPSHOT_SPEC.md` §4). Wide enough that a frontend passes its
/// **native** tab id (the Freya `TabId` is a Uuid → `as_u128`) rather than
/// maintaining a parallel one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WsId(pub u128);

/// One dispatched run's identity — the UI's per-press nonce (`QuerySpec::run`), passed
/// down so [`Engine::cancel`] and the settle path can tell "still this run" from "a
/// newer run replaced it" without a parallel request-id scheme.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RunTag(pub u128);

/// Process-unique id per engine (one per project window), scoping snapshot files so
/// windows never collide.
static ENGINE_SEQ: AtomicU64 = AtomicU64::new(0);

/// A workspace's in-flight run or explain: its dispatch identity, the snapshot it is
/// materializing (`None` for an explain), and the abort handle that cancels it.
struct InFlight {
    tag: RunTag,
    snapshot: Option<SnapshotId>,
    abort: AbortHandle,
    start: Instant,
}

/// The engine's lifecycle bookkeeping, all under one lock (never held across an await):
/// which run is in flight per workspace, and which snapshot each workspace currently owns.
#[derive(Default)]
struct Lifecycle {
    inflight: HashMap<WsId, InFlight>,
    current: HashMap<WsId, SnapshotId>,
}

/// A window's engine. Create once per project window (cheap to share as `Arc<Engine>`);
/// dropping it aborts in-flight work and removes its snapshot directory.
pub struct Engine {
    engine_id: u64,
    /// DataFusion's home: the private multi-thread runtime every call spawns onto.
    /// `Option` only so `Drop` can take it for a context-safe `shutdown_background`
    /// (a plain field drop panics when the engine is dropped inside another runtime,
    /// e.g. a `#[tokio::test]`); always `Some` while the engine lives.
    rt: Option<tokio::runtime::Runtime>,
    ctx: SessionContext,
    /// The `datafusion.*` config overrides this engine runs with (W2). Mutex'd so a
    /// future live `set_config` doesn't change the field's shape.
    overrides: Mutex<BTreeMap<String, String>>,
    /// Snapshot-id allocator — ids are per-engine unique for the process lifetime,
    /// which is what makes a snapshot immutable-by-identity.
    snap_seq: AtomicU64,
    lifecycle: Mutex<Lifecycle>,
    /// The registered SQL functions (built-ins + UDFs), enumerated once at build for
    /// the language service (S26/S7/S25).
    functions: FunctionCatalog,
}

impl Engine {
    /// Build a window's engine, honouring the given `datafusion.*` `overrides` (W2).
    pub fn new(overrides: BTreeMap<String, String>) -> Engine {
        let engine_id = ENGINE_SEQ.fetch_add(1, Ordering::Relaxed);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name(format!("df-engine-{engine_id}"))
            .enable_all()
            .build()
            .expect("tokio runtime");
        let ctx = build_context(&overrides);
        let functions = {
            use datafusion::execution::registry::FunctionRegistry;
            let mut scalar: Vec<String> = ctx.udfs().into_iter().collect();
            let mut aggregate: Vec<String> = ctx.udafs().into_iter().collect();
            let mut window: Vec<String> = ctx.udwfs().into_iter().collect();
            scalar.sort();
            aggregate.sort();
            window.sort();
            FunctionCatalog { scalar, aggregate, window }
        };
        Engine {
            engine_id,
            rt: Some(rt),
            ctx,
            overrides: Mutex::new(overrides),
            snap_seq: AtomicU64::new(1),
            lifecycle: Mutex::default(),
            functions,
        }
    }

    /// The registered SQL functions (the editor's language catalog).
    pub fn functions(&self) -> &FunctionCatalog {
        &self.functions
    }

    /// The engine's runtime (always present while the engine lives — see the field).
    fn rt(&self) -> &tokio::runtime::Runtime {
        self.rt.as_ref().expect("engine runtime")
    }

    // --- run / read -------------------------------------------------------

    /// Run `sql` **once** for workspace `ws`: materialize a fresh immutable snapshot
    /// and return its handle + page 1 (`docs/SNAPSHOT_SPEC.md` §3). Dispatch retires
    /// the workspace's previous snapshot and aborts its in-flight run (§4); `tag` is
    /// the run's dispatch identity for [`cancel`](Engine::cancel) / supersede checks.
    pub async fn query(
        &self,
        ws: WsId,
        tag: RunTag,
        sql: String,
        page_size: usize,
    ) -> Result<(QueryOutput, RecordBatch), String> {
        let snapshot = SnapshotId(self.snap_seq.fetch_add(1, Ordering::Relaxed));
        let fmt = CellFormat::new(&self.overrides.lock().unwrap());
        let task = {
            let mut lc = self.lifecycle.lock().unwrap();
            if let Some(prev) = lc.inflight.remove(&ws) {
                self.abort_inflight(prev);
            }
            // Retire-on-dispatch: the previous snapshot goes when the new run starts,
            // keeping all lifecycle in this one lock (spec §4). Cached UI pages of it
            // are unaffected; uncached reads of it now fail cleanly.
            if let Some(old) = lc.current.remove(&ws) {
                retire_snapshot(&self.ctx, self.engine_id, old);
            }
            let ctx = self.ctx.clone();
            let engine_id = self.engine_id;
            let task = self.rt().spawn(async move {
                run_and_snapshot(&ctx, engine_id, snapshot, &sql, page_size, &fmt).await
            });
            lc.inflight.insert(
                ws,
                InFlight {
                    tag,
                    snapshot: Some(snapshot),
                    abort: task.abort_handle(),
                    start: Instant::now(),
                },
            );
            task
        };

        let joined = task.await;

        let mut lc = self.lifecycle.lock().unwrap();
        // Only the still-latest run may settle workspace state; a newer dispatch has
        // already retired everything this one owned.
        let latest = lc.inflight.get(&ws).map(|f| f.tag) == Some(tag);
        if latest {
            lc.inflight.remove(&ws);
        }
        match joined {
            Ok(Ok((output, batch))) => {
                if latest {
                    if let Some(snap) = output.snapshot {
                        lc.current.insert(ws, snap);
                    }
                    Ok((output, batch))
                } else {
                    // Finished after being superseded — its snapshot must not leak.
                    retire_snapshot(&self.ctx, self.engine_id, snapshot);
                    Err("superseded by a newer run".into())
                }
            }
            // `run_and_snapshot` cleaned its own partial on failure.
            Ok(Err(e)) => Err(e),
            // Aborted — the aborter (cancel / supersede / cleanup) retired the partial.
            Err(join) if join.is_cancelled() => Err("cancelled".into()),
            Err(join) => {
                retire_snapshot(&self.ctx, self.engine_id, snapshot);
                Err(format!("query task failed: {join}"))
            }
        }
    }

    /// Read one page of one immutable snapshot — `sort` = `(column, ascending)` applied
    /// as an `ORDER BY` over the whole snapshot before the page window (Rz6). Reads are
    /// snapshot-scoped and side-effect free: safely cacheable by `(snapshot, page,
    /// page_size, sort)`.
    pub async fn fetch_page(
        &self,
        snapshot: SnapshotId,
        page: usize,
        page_size: usize,
        sort: Option<(String, bool)>,
    ) -> Result<(Vec<Vec<Cell>>, RecordBatch), String> {
        let ctx = self.ctx.clone();
        let fmt = CellFormat::new(&self.overrides.lock().unwrap());
        self.rt()
            .spawn(async move { query::fetch_page(&ctx, snapshot, page, page_size, sort, &fmt).await })
            .await
            .map_err(|e| format!("page task failed: {e}"))?
    }

    /// Run an `EXPLAIN [ANALYZE]` statement for `ws` — a parsed plan tree, no snapshot.
    /// Supersedes the workspace's in-flight run (mutually exclusive, like a re-run) but
    /// leaves its settled snapshot alone (spec §4: explains materialize nothing).
    pub async fn explain(&self, ws: WsId, tag: RunTag, sql: String) -> Result<QueryPlan, String> {
        let task = {
            let mut lc = self.lifecycle.lock().unwrap();
            if let Some(prev) = lc.inflight.remove(&ws) {
                self.abort_inflight(prev);
            }
            let ctx = self.ctx.clone();
            let task = self.rt().spawn(async move { explain::run_explain(&ctx, &sql).await });
            lc.inflight.insert(
                ws,
                InFlight { tag, snapshot: None, abort: task.abort_handle(), start: Instant::now() },
            );
            task
        };

        let joined = task.await;

        let mut lc = self.lifecycle.lock().unwrap();
        if lc.inflight.get(&ws).map(|f| f.tag) == Some(tag) {
            lc.inflight.remove(&ws);
        }
        match joined {
            Ok(res) => res,
            Err(join) if join.is_cancelled() => Err("cancelled".into()),
            Err(join) => Err(format!("explain task failed: {join}")),
        }
    }

    /// Cancel `ws`'s in-flight run/explain **iff** it is still dispatch `tag` (S14 — a
    /// stale cancel can't abort a just-started newer run). Returns the elapsed time when
    /// something was actually cancelled; the awaiting `query`/`explain` settles
    /// `Err("cancelled")`.
    pub fn cancel(&self, ws: WsId, tag: RunTag) -> Option<u128> {
        let mut lc = self.lifecycle.lock().unwrap();
        if lc.inflight.get(&ws).map(|f| f.tag) == Some(tag) {
            let f = lc.inflight.remove(&ws).unwrap();
            let elapsed = f.start.elapsed().as_millis();
            self.abort_inflight(f);
            Some(elapsed)
        } else {
            None
        }
    }

    // --- catalog ----------------------------------------------------------

    /// (Re)register one external table from its spec, returning its inferred schema +
    /// free row count.
    pub async fn register(&self, spec: TableSpec) -> Result<TableMeta, String> {
        let ctx = self.ctx.clone();
        self.rt()
            .spawn(async move { catalog::register_external(&ctx, &spec).await })
            .await
            .map_err(|e| format!("register task failed: {e}"))?
    }

    /// Drop a registered table.
    pub fn deregister(&self, table: &str) {
        let _ = self.ctx.deregister_table(table);
    }

    /// Create (or redefine) the SQL view `name` over `sql`, returning its columns and
    /// what it reads (D10). `CREATE OR REPLACE` — redefinition is the ⌘S-on-a-view path.
    pub async fn create_view(&self, name: String, sql: String) -> Result<ViewMeta, String> {
        let ctx = self.ctx.clone();
        self.rt()
            .spawn(async move {
                let stmt = format!("CREATE OR REPLACE VIEW {name} AS {sql}");
                let df = ctx.sql(&stmt).await.map_err(|e| e.to_string())?;
                // The DDL only takes effect when its (empty) result is driven.
                let _ = df.collect().await;
                // The freshly-registered view's own `DataFrame` gives both the columns
                // and what it reads — the planner has already resolved it, so we never
                // parse the SQL ourselves.
                let t = ctx.table(name.as_str()).await.map_err(|e| e.to_string())?;
                let deps = catalog::plan_deps(t.logical_plan());
                let columns = t
                    .schema()
                    .fields()
                    .iter()
                    .map(|f| catalog::column_info(f))
                    .collect();
                Ok(ViewMeta {
                    columns,
                    tables: deps.tables,
                    aliases: deps.aliases,
                })
            })
            .await
            .map_err(|e| format!("create view task failed: {e}"))?
    }

    /// Drop the SQL view `name` (idempotent — `IF EXISTS`).
    pub async fn drop_view(&self, name: String) -> Result<(), String> {
        let ctx = self.ctx.clone();
        self.rt()
            .spawn(async move {
                ctx.sql(&format!("DROP VIEW IF EXISTS {name}"))
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| format!("drop view task failed: {e}"))?
    }

    // --- lifecycle --------------------------------------------------------

    /// Tear down one workspace (tab close): abort its in-flight run and retire its
    /// current snapshot (spec §4).
    pub fn cleanup_ws(&self, ws: WsId) {
        let mut lc = self.lifecycle.lock().unwrap();
        if let Some(f) = lc.inflight.remove(&ws) {
            self.abort_inflight(f);
        }
        if let Some(snap) = lc.current.remove(&ws) {
            retire_snapshot(&self.ctx, self.engine_id, snap);
        }
    }

    /// Abort an in-flight run and retire whatever snapshot it was materializing. (An
    /// abort drops the DataFusion stream at its next await — cooperative cancel — so the
    /// partial's error-path cleanup never runs; the retire here covers it.)
    fn abort_inflight(&self, f: InFlight) {
        f.abort.abort();
        if let Some(snap) = f.snapshot {
            retire_snapshot(&self.ctx, self.engine_id, snap);
        }
    }
}

impl Drop for Engine {
    /// The window is closing: abort everything in flight and remove this engine's
    /// snapshot directory. (`purge_snapshot_root` at process start covers an abrupt
    /// exit that skips this.)
    fn drop(&mut self) {
        let mut lc = self.lifecycle.lock().unwrap();
        for (_, f) in lc.inflight.drain() {
            f.abort.abort();
        }
        lc.current.clear();
        drop(lc);
        // Context-safe shutdown: don't block on worker threads (a plain `Runtime` drop
        // panics inside another async context); aborted tasks are dropped in the background.
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
        let _ = std::fs::remove_dir_all(snapshot_dir(self.engine_id));
    }
}

/// Build a `SessionContext` honouring the engine config `overrides`: the
/// `ConfigOptions` keys go on the `SessionConfig`; the `datafusion.runtime.*` keys
/// build a `RuntimeEnv` (parsed via `parse_capacity_limit`). Bad values are logged
/// and skipped rather than failing the whole engine.
fn build_context(overrides: &BTreeMap<String, String>) -> SessionContext {
    let mut config = SessionConfig::new();
    for (key, value) in overrides {
        if key.starts_with("datafusion.runtime.") {
            continue; // runtime.* live on the RuntimeEnv, not ConfigOptions
        }
        if config::is_owned_key(key) {
            continue; // ours (see below) — a stale saved override must not apply
        }
        if let Err(e) = config.options_mut().set(key, value) {
            tracing::warn!("engine config: skipping {key}={value}: {e}");
        }
    }
    // Name the catalog/schema ourselves (`strata`/`public`): DataFusion's defaults are
    // renameable via `datafusion.catalog.default_*`, which would move our tables out
    // from under name-based lookups; `is_owned_key` fences those keys out of the apply
    // paths so the naming holds.
    let config = config.with_default_catalog_and_schema(CATALOG, SCHEMA);
    match build_runtime(overrides) {
        Ok(Some(rt)) => SessionContext::new_with_config_rt(config, rt),
        Ok(None) => SessionContext::new_with_config(config),
        Err(e) => {
            tracing::warn!("engine runtime config invalid ({e}); using defaults");
            SessionContext::new_with_config(config)
        }
    }
}

/// The catalog + schema **we own** — see [`build_context`].
const CATALOG: &str = "strata";
const SCHEMA: &str = "public";

/// A `RuntimeEnv` from the `datafusion.runtime.*` overrides, or `None` when none are
/// set (default runtime). Sizes ("2G", "100G") parse via `parse_capacity_limit`.
fn build_runtime(
    overrides: &BTreeMap<String, String>,
) -> Result<Option<std::sync::Arc<datafusion::execution::runtime_env::RuntimeEnv>>, String> {
    use datafusion::execution::runtime_env::RuntimeEnvBuilder;
    let val = |k: &str| {
        overrides
            .get(k)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let mem = val("datafusion.runtime.memory_limit");
    let tmp = val("datafusion.runtime.max_temp_directory_size");
    if mem.is_none() && tmp.is_none() {
        return Ok(None);
    }
    let mut b = RuntimeEnvBuilder::new();
    if let Some(m) = mem {
        let bytes = SessionContext::parse_capacity_limit("datafusion.runtime.memory_limit", &m)
            .map_err(|e| e.to_string())?;
        b = b.with_memory_limit(bytes, 1.0);
    }
    if let Some(t) = tmp {
        let bytes =
            SessionContext::parse_capacity_limit("datafusion.runtime.max_temp_directory_size", &t)
                .map_err(|e| e.to_string())?;
        b = b.with_max_temp_directory_size(bytes as u64);
    }
    b.build_arc().map(Some).map_err(|e| e.to_string())
}
