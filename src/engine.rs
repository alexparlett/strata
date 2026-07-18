//! DataFusion engine on a dedicated thread with its own Tokio runtime.
//!
//! Pagination model (bounded memory): each query is executed **once** and its
//! full result is spooled to a temporary parquet **snapshot** on disk. The true
//! row count comes from a `COUNT(*)` over the snapshot, and every page is a
//! bounded `LIMIT/OFFSET` read from it — so RAM only ever holds one page, no
//! matter how far the user pages, and no query is ever recomputed per page.
//!
//! UI → engine: `tokio::mpsc::unbounded` of [`Command`]. engine → UI:
//! `tokio::mpsc::unbounded` of [`Event`], drained by a Dioxus coroutine.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use dioxus::prelude::{Global, ReadableExt, WritableExt};
use dioxus_stores::*;

use crate::plan::{PlanKind, PlanNode, QueryPlan};
use crate::sql::FunctionCatalog;
use crate::util::Kind;
use datafusion::arrow::array::Array;
use datafusion::arrow::datatypes::{DataType, Field};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::logical_expr::LogicalPlan;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::physical_plan::display::DisplayableExecutionPlan;
use datafusion::physical_plan::metrics::MetricValue;
use datafusion::physical_plan::{collect, displayable, ExecutionPlan};
use datafusion::prelude::*;
use futures::StreamExt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const MAX_CELL_LEN: usize = 400;

/// Which fact a [`Stat`] carries.
///
/// Keyed rather than positional so the two tiers can interlock: D4's profile surfaces
/// only what the source didn't already answer for free, by key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatKey {
    Nulls,
    Min,
    Max,
    Distinct,
    Mean,
    Median,
}

/// One fact about a column, ready to display.
///
/// Deliberately a **list**, not a fixed set of fields: which facts exist depends
/// entirely on where they came from. A Parquet footer yields nulls/min/max for nothing;
/// CSV and JSON yield literally none; D4's profile computes whatever the source didn't,
/// and adds distinct/mean/median besides. Fixed `Option` fields would bake the Parquet
/// shape into every source and leave the profile nowhere to put the same facts. Both
/// tiers emit this one shape, so the inspector renders a row per fact that genuinely
/// exists rather than a grid of blanks.
///
/// `exact` is false when the source truncated the value (Parquet does this to long
/// strings / binary routinely), making it a bound rather than the value — the inspector
/// marks those `~`. Computed facts are always exact.
#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub key: StatKey,
    pub text: String,
    pub exact: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub dtype: String,
    pub kind: Kind,
    pub nullable: bool,
    pub children: Vec<ColumnInfo>,
    /// Facts the source reports **for free** — read, never computed. Empty for any
    /// format without metadata to read, which is every format but Parquet (and Arrow).
    pub stats: Vec<Stat>,
}

/// What a (re)registration learned about a table: its columns, plus the free row count
/// (`None` when the source doesn't report one).
#[derive(Clone, Debug, PartialEq)]
pub struct TableMeta {
    pub columns: Vec<ColumnInfo>,
    pub rows: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub null: bool,
}

/// The current page of a query, plus the snapshot's true total.
#[derive(Clone, Debug, Default)]
pub struct QueryOutput {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Cell>>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug)]
pub struct TableSpec {
    pub name: String,
    pub paths: Vec<String>,
    pub format: String,
    pub partitions: Vec<(String, String)>,
}

pub enum Command {
    Register(TableSpec),
    Deregister {
        table: String,
    },
    /// Re-infer every registered table's schema — picks up new columns or a changed
    /// partition scheme. Files, rows, and partition *values* are already live (each
    /// scan re-`LIST`s; we run no `ListFilesCache`), so this only re-registers to
    /// refresh the inferred schema. Emits a `Registered` per table.
    RefreshCatalog,
    /// Full-scan profile of one table (D4) — see [`crate::profile`]. Runs spawned and
    /// keyed by table, so profiles of different tables run concurrently.
    Profile {
        table: String,
    },
    /// Abort an in-flight profile.
    CancelProfile {
        table: String,
    },
    CreateView {
        name: String,
        sql: String,
    },
    DropView {
        name: String,
    },
    /// Run a query → spool a snapshot → return page 1 + total.
    Query {
        req_id: u64,
        ws_id: u64,
        sql: String,
        page_size: usize,
    },
    /// Run an `EXPLAIN [ANALYZE]` and return its parsed plan tree (no snapshot).
    Explain {
        req_id: u64,
        ws_id: u64,
        sql: String,
    },
    /// Abort the in-flight Query/Explain for `ws_id`, but only if it's still request
    /// `req_id` (S14).
    Cancel {
        ws_id: u64,
        req_id: u64,
    },
    /// Read a page from the workspace's existing snapshot (no recompute). `sort` =
    /// `(column name, ascending)` applied as an `ORDER BY` over the snapshot before the
    /// page window; `None` = snapshot order (Rz6).
    FetchPage {
        ws_id: u64,
        page: usize,
        page_size: usize,
        sort: Option<(String, bool)>,
    },
    /// Drop one workspace's snapshot (table + temp file) — e.g. on tab close.
    CleanupWorkspace {
        ws_id: u64,
    },
    /// Remove all snapshots (e.g. on app exit).
    CleanupAll,
    /// Apply new engine config overrides live (W2). The `ConfigOptions` keys take
    /// effect on the running context immediately; the two `datafusion.runtime.*`
    /// keys can't change on a live `RuntimeEnv`, so a change there emits a `Notice`
    /// (they apply when the window is reopened).
    SetEngineConfig(BTreeMap<String, String>),
    /// Write a workspace's snapshot to a file (or, with `partition_cols`, a
    /// Hive-partitioned directory) via `COPY … TO`.
    Export {
        ws_id: u64,
        path: String,
        format: String,
        all: bool,
        page: usize,
        page_size: usize,
        csv_delimiter: char,
        csv_header: bool,
        csv_null: String,
        pq_compression: String,
        pq_level: u32,
        partition_cols: Vec<String>,
        keep_partition: bool,
    },
}

pub enum Event {
    Registered {
        table: String,
        path: String,
        result: Result<TableMeta, String>,
    },
    Deregistered {
        table: String,
    },
    /// A profile scan finished (D4). The row's `profiling` flag clears either way.
    Profiled {
        table: String,
        result: Result<crate::profile::CatalogProfile, String>,
    },
    ViewChanged {
        name: String,
        sql: String,
        dropped: bool,
        /// The base tables the view reads (D10) — empty on a drop or a failure.
        deps: Vec<String>,
        /// Every name the view inlines — its referenced views, plus table-alias / CTE
        /// noise the UI filters against the known views. Empty on a drop or failure.
        aliases: Vec<String>,
        result: Result<Vec<ColumnInfo>, String>,
    },
    QueryResult {
        req_id: u64,
        ws_id: u64,
        /// `(display page, page `RecordBatch`)` — the batch is the type-aware source for the
        /// results Copy / Export-to-clipboard (Rz4). Kept out of `QueryOutput` so the grid's
        /// per-render clone never touches it (it's Arc-cheap to carry).
        result: Result<(QueryOutput, RecordBatch), String>,
    },
    /// Result of an `EXPLAIN [ANALYZE]` — a parsed plan tree or an error.
    ExplainResult {
        req_id: u64,
        ws_id: u64,
        result: Result<QueryPlan, String>,
    },
    /// A Query/Explain was cancelled (S14) — clears the tab's running state.
    QueryCancelled {
        req_id: u64,
        ws_id: u64,
        elapsed_ms: u128,
    },
    PageResult {
        ws_id: u64,
        page: usize,
        /// `(display rows, page `RecordBatch`)` — see `QueryResult`.
        result: Result<(Vec<Vec<Cell>>, RecordBatch), String>,
    },
    /// Result of an export: `Ok((path, rows_written))` or an error message.
    Exported {
        result: Result<(String, usize), String>,
    },
    /// The engine's registered function names (built-ins + any UDFs), sent once on
    /// startup so the UI SQL language service (S26/S7/S25) can complete + validate
    /// real functions. Names only; signatures/detail can follow later.
    Functions {
        scalar: Vec<String>,
        aggregate: Vec<String>,
        window: Vec<String>,
    },
    Notice(String),
    /// A saved `datafusion.runtime.*` change can't be applied to the running engine
    /// (its `RuntimeEnv` is fixed at build) — the UI offers a window restart (W2).
    EngineRestartRequired,
}

/// Process-unique id per spawned engine (one per project window), used to scope
/// snapshot files so windows never collide.
static ENGINE_SEQ: AtomicU64 = AtomicU64::new(0);

/// This window's engine — the UI-side owner of the connection: the command channel
/// (`send`), the request-id counter (`next_req`), and the registered SQL functions.
/// [`Engine::spawn`] starts the worker thread and stashes the inbox, handing back the
/// event stream for the caller to drain. The instance lives in the private `Global`
/// below (Dioxus per-window state must be a `Global`; `cmd_tx` also isn't `PartialEq`,
/// so `Engine` can't derive `Store` and instead rides whole in an `Option`, like the
/// whole-value stores in `crate::events`).
#[derive(Store)]
pub struct Engine {
    cmd_tx: UnboundedSender<Command>,
    /// This window's event stream — `Some` until the single drain task takes it
    /// (`take_evt_rx`). A receiver is single-consumer, so it can't stay a live store
    /// borrow: holding one across the async drain loop collides with any other engine
    /// write (e.g. `set_functions`) on the same signal.
    evt_rx: Option<UnboundedReceiver<Event>>,
    /// Monotonic request-id source — an `AtomicU64` so `next_req` mutates it through a
    /// *read* borrow of the store (no store write ⇒ it never notifies `functions` readers).
    next_req: AtomicU64,
    /// The engine's registered SQL functions — read reactively by the language service.
    functions: FunctionCatalog,
}

/// This window's single engine — `None` until [`Engine::spawn`], then `Some`. A
/// `GlobalStore` needs a `Store`-able type and `Engine` holds a non-`PartialEq`
/// `Sender`, so it rides whole in an `Option` (accessed as one value, cf. the
/// whole-value stores in `crate::events`) rather than deriving `Store` per field.
static ENGINE: GlobalStore<Engine> = Global::new(|| Engine::spawn());

pub fn store() -> Store<Engine> {
    ENGINE.resolve()
}

impl Engine {
    /// Start this window's engine worker (seeded with the current `datafusion.*`
    /// `overrides`, W2), stash the instance, and return the event stream for the caller
    /// to drain. Later config changes arrive as [`Command::SetEngineConfig`].
    pub fn spawn() -> Engine {
        let overrides = crate::settings::engine_overrides();
        let (cmd_tx, cmd_rx) = unbounded_channel::<Command>();
        let (evt_tx, evt_rx) = unbounded_channel::<Event>();
        let engine_id = ENGINE_SEQ.fetch_add(1, Ordering::Relaxed);
        std::thread::Builder::new()
            .name(format!("df-engine-{engine_id}"))
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(engine_loop(cmd_rx, evt_tx, engine_id, overrides));
            })
            .expect("spawn engine");
        Engine {
            cmd_tx,
            evt_rx: Some(evt_rx),
            next_req: AtomicU64::new(1),
            functions: FunctionCatalog::default(),
        }
    }

    /// Send a command to this window's engine (no-op if it isn't up yet).
    pub fn send(cmd: Command) {
        let store = ENGINE.resolve();
        let e = store.peek();
        let _ = e.cmd_tx.send(cmd);
    }

    /// Allocate the next request id (monotonic).
    pub fn next_req() -> u64 {
        let store = ENGINE.resolve();
        let g = store.peek();

        g.next_req.fetch_add(1, Ordering::Relaxed)
    }

    /// A clone of the registered SQL functions — reactive (the editor's language catalog).
    pub fn functions() -> FunctionCatalog {
        let store = ENGINE.resolve();
        let g = store.read();
        g.functions.clone()
    }

    /// Replace the registered SQL functions (`Event::Functions`).
    pub fn set_functions(functions: FunctionCatalog) {
        let mut store = ENGINE.resolve();
        let mut g = store.write();
        g.functions = functions;
    }

    /// Take this window's event stream for the single drain task. A receiver is
    /// single-consumer, so it leaves the store (`None` after) rather than being held
    /// as a live borrow across the drain loop — which would collide with any other
    /// engine write. Panics if taken twice.
    pub fn take_evt_rx() -> UnboundedReceiver<Event> {
        let mut store = ENGINE.resolve();
        let mut g = store.write();
        g.evt_rx.take().expect("engine event stream already taken")
    }
}

/// Send a [`Command`] to this window's engine — sugar for [`Engine::send`] that
/// prefixes `Command::`, mirroring the `crate::event_*!` log macros. `#[macro_export]`
/// puts it at the crate root, so call it fully-qualified: `crate::command!(…)`.
/// Everything after the name is the variant, so struct / tuple / unit variants all
/// work: `crate::command!(CleanupAll)`, `crate::command!(Cancel { ws_id, req_id })`,
/// `crate::command!(Register(spec))`.
#[macro_export]
macro_rules! command {
    ($($variant:tt)+) => {
        $crate::engine::Engine::send($crate::engine::Command::$($variant)+)
    };
}

/// The catalog + schema **we own**. DataFusion's `"datafusion"` / `"public"` are only
/// *defaults*: `datafusion.catalog.default_*` renames them from the user's engine
/// config, and `SessionConfig::from_env` would inherit `DATAFUSION_*` env vars too (we
/// build from `new()`, so those can't reach us today — but that's a construction
/// detail, not a guarantee). Either would move our tables out from under
/// [`Command::RefreshCatalog`], which looks them up by name. Naming them ourselves is
/// free: the DataFusion catalog is never persisted — every window rebuilds it by
/// re-registering the project's tables on open.
const CATALOG: &str = "strata";
const SCHEMA: &str = "public";

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
        if crate::engine_config::is_owned_key(key) {
            continue; // ours (see below) — a stale saved override must not apply
        }
        if let Err(e) = config.options_mut().set(key, value) {
            tracing::warn!("engine config: skipping {key}={value}: {e}");
        }
    }
    // Name the catalog/schema ourselves. The context creates them on construction
    // (`create_default_catalog_and_schema` defaults on) and they stay the resolution
    // default for bare table names. Note this alone wouldn't hold: `SetEngineConfig`
    // re-asserts the overrides over the built config at runtime, which is why
    // `is_owned_key` fences them out of *both* apply paths, not just this one.
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

/// A `RuntimeEnv` from the `datafusion.runtime.*` overrides, or `None` when none are
/// set (default runtime). Sizes ("2G", "100G") parse via `parse_capacity_limit`.
fn build_runtime(
    overrides: &BTreeMap<String, String>,
) -> Result<Option<Arc<datafusion::execution::runtime_env::RuntimeEnv>>, String> {
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

/// The two `datafusion.runtime.*` values (memory limit, spill dir cap), for detecting
/// a change that the running engine can't apply live.
fn runtime_keys(o: &BTreeMap<String, String>) -> (Option<String>, Option<String>) {
    (
        o.get("datafusion.runtime.memory_limit").cloned(),
        o.get("datafusion.runtime.max_temp_directory_size").cloned(),
    )
}

/// An in-flight Query/Explain task for one workspace (S14). Keyed by `ws_id` in
/// `engine_loop`'s registry so a re-run or a `Cancel` can abort it; aborting drops
/// the DataFusion stream, cancelling execution cooperatively.
struct InFlight {
    req_id: u64,
    start: Instant,
    abort: tokio::task::AbortHandle,
}

async fn engine_loop(
    mut cmd_rx: UnboundedReceiver<Command>,
    evt_tx: UnboundedSender<Event>,
    engine_id: u64,
    mut overrides: BTreeMap<String, String>,
) {
    // Leftover snapshots from a previous run are cleared once at process start
    // (`purge_snapshot_root`), not here — wiping the shared root at runtime would
    // clobber other windows' engines.
    let ctx = build_context(&overrides);
    // The runtime.* config this engine's `RuntimeEnv` was built with — a later Save
    // that leaves the saved runtime.* differing from this needs a window restart.
    let built_runtime = runtime_keys(&overrides);
    // Enumerate the full function registry (built-ins + any UDFs) once, so the UI's
    // SQL language service (S26/S7/S25) can offer + validate real function names.
    // `udafs`/`udwfs` name-set enumerators are DataFusion 54 (part of why A9 gates S26).
    {
        use datafusion::execution::registry::FunctionRegistry;
        let mut scalar: Vec<String> = ctx.udfs().into_iter().collect();
        let mut aggregate: Vec<String> = ctx.udafs().into_iter().collect();
        let mut window: Vec<String> = ctx.udwfs().into_iter().collect();
        scalar.sort();
        aggregate.sort();
        window.sort();
        let _ = evt_tx.send(Event::Functions {
            scalar,
            aggregate,
            window,
        });
    }
    // In-flight Query/Explain tasks, keyed by ws_id (S14). Owned by this single loop
    // task (no locking); the spawned tasks only send events back.
    let mut inflight: HashMap<u64, InFlight> = HashMap::new();
    // In-flight profiles, keyed by **table** — a profile belongs to a table the way a
    // query belongs to a tab, so several can run at once and none blocks the loop (D4).
    let mut profiling: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::Register(spec) => {
                let path = spec.paths.first().cloned().unwrap_or_default();
                // Re-registering replaces the data under any scan of this table: its
                // result is about to be thrown away by `end_profile`, so stop paying
                // for it.
                if let Some(h) = profiling.remove(&spec.name) {
                    h.abort();
                }
                let result = register_external(&ctx, &spec).await;
                let _ = evt_tx.send(Event::Registered {
                    table: spec.name,
                    path,
                    result,
                });
            }
            Command::Profile { table } => {
                // Spawned, never awaited here: a profile is a full scan and can run for
                // minutes — awaiting it in this loop would stall every other command
                // (queries included) behind it. Keyed by table, exactly as a query is
                // keyed by tab, so profiles of different tables run concurrently.
                // Reap finished scans first — a completed handle left in the map would
                // block its own table's re-scan forever.
                profiling.retain(|_, h| !h.is_finished());
                if profiling.contains_key(&table) {
                    continue; // already scanning this table — a second pass adds nothing
                }
                let ctx = ctx.clone();
                let tx = evt_tx.clone();
                let name = table.clone();
                let task = tokio::spawn(async move {
                    let result = run_profile(&ctx, &name).await;
                    let _ = tx.send(Event::Profiled { table: name, result });
                });
                profiling.insert(table, task.abort_handle());
            }
            Command::CancelProfile { table } => {
                // No event: the action layer clears the row itself, since an abort
                // means no `Profiled` is coming.
                if let Some(h) = profiling.remove(&table) {
                    h.abort();
                }
            }
            Command::RefreshCatalog => {
                // A refresh re-infers every table, so every in-flight scan is about to
                // be describing superseded data — abort the lot rather than let them
                // run to a result nothing will keep.
                for (_, h) in profiling.drain() {
                    h.abort();
                }
                // Re-infer each user table's schema in place, straight from what's in
                // the context — no retained specs. Query snapshots (`__snap_*`) are
                // skipped by name; SQL views by not being `ListingTable`s. Files, rows,
                // and partition values are already live (each scan re-`LIST`s — we run
                // no `ListFilesCache`), so this only refreshes the inferred schema.
                if let Some(schema) = ctx.catalog(CATALOG).and_then(|c| c.schema(SCHEMA)) {
                    for name in schema.table_names() {
                        if name.starts_with("__snap_") {
                            continue;
                        }
                        let Ok(Some(provider)) = schema.table(&name).await else {
                            continue;
                        };
                        // Only `ListingTable`s back on-disk data; grab its own paths +
                        // options (drops the borrow before we await). `TableProvider`
                        // has `Any` as a supertrait in DF 54 (no more `as_any`), and
                        // `downcast_ref` is inherent on `dyn TableProvider` — auto-deref
                        // through the `Arc` targets the provider, not the `Arc`.
                        let paths_opts = {
                            use datafusion::datasource::listing::ListingTable;
                            provider
                                .downcast_ref::<ListingTable>()
                                .map(|lt| (lt.table_paths().clone(), lt.options().clone()))
                        };
                        let Some((paths, opts)) = paths_opts else {
                            continue;
                        };
                        let result = rebuild_listing(&ctx, &name, paths, opts).await;
                        let _ = evt_tx.send(Event::Registered {
                            table: name,
                            path: String::new(),
                            result,
                        });
                    }
                }
            }
            Command::Deregister { table } => {
                // The table's going away — a scan of it is now pure waste.
                if let Some(h) = profiling.remove(&table) {
                    h.abort();
                }
                let _ = ctx.deregister_table(table.as_str());
                let _ = evt_tx.send(Event::Deregistered { table });
            }
            Command::CreateView { name, sql } => {
                // Redefining the view replaces the question any in-flight scan is
                // answering — `end_profile` will discard its result, so stop paying.
                if let Some(h) = profiling.remove(&name) {
                    h.abort();
                }
                let stmt = format!("CREATE OR REPLACE VIEW {name} AS {sql}");
                let (mut deps, mut aliases) = (Vec::new(), Vec::new());
                let result = match ctx.sql(&stmt).await {
                    Ok(df) => {
                        let _ = df.collect().await;
                        match ctx.table(name.as_str()).await {
                            Ok(t) => {
                                // The same `DataFrame` gives the columns and what the
                                // view reads (D10) — the planner has already resolved it,
                                // so we never parse the SQL ourselves.
                                let d = plan_deps(t.logical_plan());
                                deps = d.tables;
                                aliases = d.aliases;
                                Ok(t.schema().fields().iter().map(|f| column_info(f)).collect())
                            }
                            Err(e) => Err(e.to_string()),
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = evt_tx.send(Event::ViewChanged {
                    name,
                    sql,
                    dropped: false,
                    deps,
                    aliases,
                    result,
                });
            }
            Command::DropView { name } => {
                // The view's going away — a scan of it is now pure waste.
                if let Some(h) = profiling.remove(&name) {
                    h.abort();
                }
                let result = ctx
                    .sql(&format!("DROP VIEW IF EXISTS {name}"))
                    .await
                    .map(|_| Vec::new())
                    .map_err(|e| e.to_string());
                let _ = evt_tx.send(Event::ViewChanged {
                    name,
                    sql: String::new(),
                    dropped: true,
                    deps: Vec::new(),
                    aliases: Vec::new(),
                    result,
                });
            }
            Command::Query {
                req_id,
                ws_id,
                sql,
                page_size,
            } => {
                // A re-run supersedes the tab's previous query — abort it (saves CPU;
                // today its stale result is merely dropped by `is_pending`).
                if let Some(f) = inflight.remove(&ws_id) {
                    f.abort.abort();
                }
                let ctx = ctx.clone();
                let tx = evt_tx.clone();
                let fmt = CellFormat::new(&overrides);
                let task = tokio::spawn(async move {
                    let result =
                        run_and_snapshot(&ctx, engine_id, ws_id, &sql, page_size, &fmt).await;
                    let _ = tx.send(Event::QueryResult {
                        req_id,
                        ws_id,
                        result,
                    });
                });
                inflight.insert(
                    ws_id,
                    InFlight {
                        req_id,
                        start: Instant::now(),
                        abort: task.abort_handle(),
                    },
                );
            }
            Command::Explain { req_id, ws_id, sql } => {
                // An explain supersedes the tab's previous run too (mutually exclusive).
                if let Some(f) = inflight.remove(&ws_id) {
                    f.abort.abort();
                }
                let ctx = ctx.clone();
                let tx = evt_tx.clone();
                let task = tokio::spawn(async move {
                    let result = run_explain(&ctx, &sql).await;
                    let _ = tx.send(Event::ExplainResult {
                        req_id,
                        ws_id,
                        result,
                    });
                });
                inflight.insert(
                    ws_id,
                    InFlight {
                        req_id,
                        start: Instant::now(),
                        abort: task.abort_handle(),
                    },
                );
            }
            Command::Cancel { ws_id, req_id } => {
                // Abort only if the tab's in-flight run is still this request.
                if inflight.get(&ws_id).map(|f| f.req_id) == Some(req_id) {
                    let f = inflight.remove(&ws_id).unwrap();
                    let elapsed_ms = f.start.elapsed().as_millis();
                    f.abort.abort();
                    // Clear the partial snapshot the aborted task may have left.
                    let _ = ctx.deregister_table(snapshot_name(ws_id).as_str());
                    let _ = std::fs::remove_file(snapshot_file(engine_id, ws_id));
                    let _ = evt_tx.send(Event::QueryCancelled {
                        req_id,
                        ws_id,
                        elapsed_ms,
                    });
                }
            }
            Command::FetchPage {
                ws_id,
                page,
                page_size,
                sort,
            } => {
                let fmt = CellFormat::new(&overrides);
                let result = fetch_page(&ctx, ws_id, page, page_size, sort, &fmt).await;
                let _ = evt_tx.send(Event::PageResult {
                    ws_id,
                    page,
                    result,
                });
            }
            Command::CleanupWorkspace { ws_id } => {
                // Abort a still-running query first so it can't re-register a
                // snapshot for a tab we're tearing down.
                if let Some(f) = inflight.remove(&ws_id) {
                    f.abort.abort();
                }
                let _ = ctx.deregister_table(snapshot_name(ws_id).as_str());
                let _ = std::fs::remove_file(snapshot_file(engine_id, ws_id));
            }
            Command::CleanupAll => {
                for (_, f) in inflight.drain() {
                    f.abort.abort();
                }
                // Only this engine's (this window's) snapshots.
                let _ = std::fs::remove_dir_all(snapshot_dir(engine_id));
            }
            Command::SetEngineConfig(new_overrides) => {
                let old_rt = runtime_keys(&overrides);
                let new_rt = runtime_keys(&new_overrides);
                // Set every live-settable option to its effective value (so a cleared
                // override resets to the default), through the shared state so
                // registered tables survive. runtime.* handled by the restart prompt.
                // Collect any DataFusion rejections (a value that slipped past the UI
                // validator, e.g. a hand-edited config) to surface below.
                let mut rejected = Vec::new();
                {
                    let state = ctx.state_ref();
                    let mut w = state.write();
                    let opts = w.config_mut().options_mut();
                    // Known keys: set each to its override, else reset to the default so a
                    // cleared override reverts. runtime.* is RuntimeEnv-level (restart).
                    for e in crate::engine_config::ENGINE_KEYS {
                        if crate::engine_config::is_restart_key(e.key) {
                            continue;
                        }
                        let val = new_overrides
                            .get(e.key)
                            .map(String::as_str)
                            .unwrap_or(e.default);
                        if let Err(err) = opts.set(e.key, val) {
                            tracing::warn!("engine config: {}={val}: {err}", e.key);
                            rejected.push(format!("{} ({val})", e.key));
                        }
                    }
                    // Custom (non-catalog) overrides: best-effort — DataFusion rejects any
                    // key it doesn't recognise. `is_owned_key` is *not* in `ENGINE_KEYS`,
                    // so without the guard a hand-typed `catalog.default_*` would land
                    // here and re-point resolution away from our own catalog.
                    for (k, val) in &new_overrides {
                        if crate::engine_config::is_restart_key(k)
                            || crate::engine_config::is_owned_key(k)
                            || crate::engine_config::key_def(k).is_some()
                        {
                            continue;
                        }
                        if let Err(err) = opts.set(k, val) {
                            tracing::warn!("engine config: {k}={val}: {err}");
                            rejected.push(format!("{k} ({val})"));
                        }
                    }
                }
                overrides = new_overrides;
                for label in rejected {
                    let _ = evt_tx.send(Event::Notice(format!(
                        "Engine setting ignored — invalid value for {label}."
                    )));
                }
                // Runtime.* changed *and* now differs from what this engine was built
                // with → the running engine is stale; ask the UI to offer a restart.
                if new_rt != old_rt && new_rt != built_runtime {
                    let _ = evt_tx.send(Event::EngineRestartRequired);
                }
            }
            Command::Export {
                ws_id,
                path,
                format,
                all,
                page,
                page_size,
                csv_delimiter,
                csv_header,
                csv_null,
                pq_compression,
                pq_level,
                partition_cols,
                keep_partition,
            } => {
                let result = run_export(
                    &ctx,
                    ws_id,
                    ExportArgs {
                        path,
                        format,
                        all,
                        page,
                        page_size,
                        csv_delimiter,
                        csv_header,
                        csv_null,
                        pq_compression,
                        pq_level,
                        partition_cols,
                        keep_partition,
                    },
                )
                .await;
                let _ = evt_tx.send(Event::Exported { result });
            }
        }
    }

    // Command channel closed → this window's engine is done → tidy its snapshots
    // (belt-and-suspenders with the `CleanupAll` from `use_drop`; the startup
    // `purge_snapshot_root` covers an abrupt app exit that skips both).
    let _ = std::fs::remove_dir_all(snapshot_dir(engine_id));
}

/// Everything an export needs (one struct to dodge a too-many-arguments fn).
struct ExportArgs {
    path: String,
    format: String,
    all: bool,
    page: usize,
    page_size: usize,
    csv_delimiter: char,
    csv_header: bool,
    csv_null: String,
    pq_compression: String,
    pq_level: u32,
    partition_cols: Vec<String>,
    keep_partition: bool,
}

/// Export a workspace's snapshot via `COPY (…) TO … STORED AS`. A plain file path
/// (extension) → one file; `partition_cols` → a Hive-partitioned directory.
/// Returns `(path, rows_written)`.
async fn run_export(
    ctx: &SessionContext,
    ws_id: u64,
    a: ExportArgs,
) -> Result<(String, usize), String> {
    let snap = snapshot_name(ws_id);
    if ctx.table(snap.as_str()).await.is_err() {
        return Err("No results to export — run a query first".to_string());
    }

    let select = if a.all {
        format!("SELECT * FROM {snap}")
    } else {
        let offset = a.page.saturating_sub(1) * a.page_size;
        format!("SELECT * FROM {snap} LIMIT {} OFFSET {offset}", a.page_size)
    };
    let stored = match a.format.as_str() {
        "json" => "JSON",
        "parquet" => "PARQUET",
        "arrow" => "ARROW",
        _ => "CSV",
    };
    let part_clause = if a.partition_cols.is_empty() {
        String::new()
    } else {
        format!(" PARTITIONED BY ({})", a.partition_cols.join(", "))
    };
    // Format options (JSON/Arrow take none here; JSON always writes NDJSON).
    let opts = match a.format.as_str() {
        "csv" => {
            let nv = match a.csv_null.as_str() {
                "null" => "NULL",
                "nan" => "NaN",
                _ => "",
            };
            format!(
                " OPTIONS ('HAS_HEADER' '{}', 'DELIMITER' '{}', 'NULL_VALUE' '{}')",
                a.csv_header, a.csv_delimiter, nv
            )
        }
        "parquet" => format!(
            " OPTIONS ('COMPRESSION' '{}')",
            pq_codec(&a.pq_compression, a.pq_level)
        ),
        _ => String::new(),
    };

    // `keep_partition_by_columns` is a session config, not a COPY option — set it
    // explicitly per partitioned export (default off).
    if !a.partition_cols.is_empty() {
        if let Ok(df) = ctx
            .sql(&format!(
                "SET datafusion.execution.keep_partition_by_columns = {}",
                a.keep_partition
            ))
            .await
        {
            let _ = df.collect().await;
        }
    }

    let esc = a.path.replace('\'', "''");
    let stmt = format!("COPY ({select}) TO '{esc}' STORED AS {stored}{part_clause}{opts}");

    let df = ctx.sql(&stmt).await.map_err(|e| e.to_string())?;
    let batches = df.collect().await.map_err(|e| e.to_string())?;
    Ok((a.path, copy_row_count(&batches)))
}

/// Parquet compression codec string, with a level for the codecs that take one.
fn pq_codec(codec: &str, level: u32) -> String {
    match codec {
        "snappy" => "snappy".into(),
        "lz4" => "lz4".into(),
        "none" | "uncompressed" => "uncompressed".into(),
        "gzip" => format!("gzip({})", level.clamp(1, 9)),
        "brotli" => format!("brotli({})", level.clamp(1, 11)),
        _ => format!("zstd({})", level.clamp(1, 22)),
    }
}

/// `COPY … TO` returns a single `UInt64` "count" column with the rows written.
fn copy_row_count(batches: &[RecordBatch]) -> usize {
    use datafusion::arrow::array::UInt64Array;
    let Some(batch) = batches.first() else {
        return 0;
    };
    if batch.num_columns() == 0 {
        return 0;
    }
    batch
        .column(0)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .filter(|a| !a.is_empty())
        .map(|a| a.value(0) as usize)
        .unwrap_or(0)
}

// ---- query → snapshot → page ----

fn snapshot_name(ws_id: u64) -> String {
    format!("__snap_{ws_id}")
}

fn snapshots_root() -> String {
    let mut d = std::env::temp_dir();
    d.push("strata_snapshots");
    d.to_string_lossy().into_owned()
}

/// Per-engine snapshot subdirectory. Each window runs its own engine with a
/// unique `engine_id`, so windows never share (or clobber) snapshot files even
/// though their workspace ids overlap (every project numbers workspaces from 1).
fn snapshot_dir(engine_id: u64) -> String {
    let mut d = std::path::PathBuf::from(snapshots_root());
    d.push(format!("e_{engine_id}"));
    d.to_string_lossy().into_owned()
}

fn snapshot_file(engine_id: u64, ws_id: u64) -> String {
    let mut d = std::path::PathBuf::from(snapshot_dir(engine_id));
    d.push(format!("ws_{ws_id}.parquet"));
    d.to_string_lossy().into_owned()
}

/// Remove *all* engines' snapshots. Safe only at process startup, before any
/// engine exists — at runtime an engine only ever cleans its own `snapshot_dir`.
pub fn purge_snapshot_root() {
    let _ = std::fs::remove_dir_all(snapshots_root());
}

/// Run the query **once**, streaming every batch straight to a parquet snapshot
/// on disk while counting the exact total and capturing the first page — no
/// separate `COUNT`, no re-read, bounded memory.
async fn run_and_snapshot(
    ctx: &SessionContext,
    engine_id: u64,
    ws_id: u64,
    sql: &str,
    page_size: usize,
    fmt: &CellFormat,
) -> Result<(QueryOutput, RecordBatch), String> {
    let start = Instant::now();
    let snap = snapshot_name(ws_id);
    let file = snapshot_file(engine_id, ws_id);

    // reset the previous snapshot for this workspace
    let _ = ctx.deregister_table(snap.as_str());
    let _ = std::fs::remove_file(&file);
    if let Some(parent) = Path::new(&file).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let opts = SQLOptions::new()
        .with_allow_dml(false)
        .with_allow_ddl(false)
        .with_allow_statements(false);

    let df = ctx
        .sql_with_options(sql, opts)
        .await
        .map_err(|e| e.to_string())?;
    // capture columns before the DataFrame is consumed by the stream
    let columns: Vec<ColumnInfo> = df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect();
    // Arrow schema of the result — captured before the DataFrame is consumed by the stream,
    // for concatenating page 1 into its `RecordBatch`.
    let arrow_schema = df.schema().inner().clone();
    let mut stream = df.execute_stream().await.map_err(|e| e.to_string())?;

    let mut writer: Option<ArrowWriter<std::fs::File>> = None;
    let mut total = 0usize;
    let mut page1: Vec<Vec<Cell>> = Vec::new();
    let mut page1_batches: Vec<RecordBatch> = Vec::new();
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(|e| e.to_string())?;
        total += batch.num_rows();
        if writer.is_none() {
            let out = std::fs::File::create(&file).map_err(|e| e.to_string())?;
            writer =
                Some(ArrowWriter::try_new(out, batch.schema(), None).map_err(|e| e.to_string())?);
        }
        if let Some(w) = writer.as_mut() {
            w.write(&batch).map_err(|e| e.to_string())?;
        }
        append_batch_capped(&batch, &mut page1, &mut page1_batches, page_size, fmt)?;
    }

    // Only register a snapshot if the query produced rows; an empty result has
    // no pages to fetch.
    if let Some(w) = writer {
        w.close().map_err(|e| e.to_string())?;
        ctx.register_parquet(snap.as_str(), file.as_str(), ParquetReadOptions::default())
            .await
            .map_err(|e| e.to_string())?;
    }

    let page1_batch = datafusion::arrow::compute::concat_batches(&arrow_schema, &page1_batches)
        .map_err(|e| e.to_string())?;
    Ok((
        QueryOutput {
            columns,
            rows: page1,
            total,
            page: 1,
            page_size,
            elapsed_ms: start.elapsed().as_millis(),
        },
        page1_batch,
    ))
}

/// Display formatting for grid cells, derived from the engine's `datafusion.format.*`
/// overrides (W2). Owns the format strings so an arrow [`FormatOptions`] can borrow
/// them; `null` is the literal shown for NULL cells (which stay flagged `null: true`
/// for the grid's own dimmed styling, so only the text changes).
struct CellFormat {
    null: String,
    date: String,
    ts: String,
}

impl CellFormat {
    fn new(overrides: &BTreeMap<String, String>) -> Self {
        let eff = |k: &str| crate::engine_config::effective(overrides, k).unwrap_or_default();
        Self {
            null: eff("datafusion.format.null"),
            date: eff("datafusion.format.date_format"),
            ts: eff("datafusion.format.timestamp_format"),
        }
    }

    /// An arrow [`FormatOptions`] borrowing this config's date/timestamp patterns.
    fn opts(&self) -> FormatOptions<'_> {
        let mut o = FormatOptions::default();
        if !self.date.is_empty() {
            o = o.with_date_format(Some(&self.date));
        }
        if !self.ts.is_empty() {
            o = o.with_timestamp_format(Some(&self.ts));
        }
        o
    }
}

/// Append up to `cap` rows of `batch` to `out` (display cells), collecting the sliced batch
/// into `batches_out` (concatenated later into the page's type-aware `RecordBatch`).
fn append_batch_capped(
    batch: &RecordBatch,
    out: &mut Vec<Vec<Cell>>,
    batches_out: &mut Vec<RecordBatch>,
    cap: usize,
    fmt: &CellFormat,
) -> Result<(), String> {
    if out.len() >= cap {
        return Ok(());
    }
    let take = (cap - out.len()).min(batch.num_rows());
    let batch = batch.slice(0, take);
    let cols = batch.columns();
    let opts = fmt.opts();
    let fmts = cols
        .iter()
        .map(|c| ArrayFormatter::try_new(&**c, &opts))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for r in 0..take {
        let mut row = Vec::with_capacity(fmts.len());
        for (ci, f) in fmts.iter().enumerate() {
            let null = cols[ci].is_null(r);
            let text = if null {
                fmt.null.clone()
            } else {
                truncate_cell(&f.value(r).to_string())
            };
            row.push(Cell { text, null });
        }
        out.push(row);
    }
    batches_out.push(batch.clone());
    Ok(())
}

async fn fetch_page(
    ctx: &SessionContext,
    ws_id: u64,
    page: usize,
    page_size: usize,
    sort: Option<(String, bool)>,
    fmt: &CellFormat,
) -> Result<Page, String> {
    let snap = snapshot_name(ws_id);
    let offset = page.saturating_sub(1) * page_size;
    read_page(ctx, &snap, offset, page_size, sort, fmt).await
}

async fn read_page(
    ctx: &SessionContext,
    snap: &str,
    offset: usize,
    limit: usize,
    sort: Option<(String, bool)>,
    fmt: &CellFormat,
) -> Result<Page, String> {
    let mut df = ctx.table(snap).await.map_err(|e| e.to_string())?;
    // Arrow schema of the page (sort/limit preserve it) — for concatenating the page batch.
    let schema = df.schema().inner().clone();
    if let Some((name, asc)) = sort {
        // ORDER BY the chosen column over the whole snapshot, then take the page window.
        // `Column::from_name` avoids identifier parsing on odd column names; `nulls_first =
        // false` ⇒ nulls always sort last, both directions (Rz6).
        let expr = col(datafusion::common::Column::from_name(name)).sort(asc, false);
        df = df.sort(vec![expr]).map_err(|e| e.to_string())?;
    }
    let batches = df
        .limit(offset, Some(limit))
        .map_err(|e| e.to_string())?
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    let batch =
        datafusion::arrow::compute::concat_batches(&schema, &batches).map_err(|e| e.to_string())?;
    let rows = batches_to_rows(&batches, fmt)?;
    Ok((rows, batch))
}

/// A page of results: display cells for the grid + the page `RecordBatch` (type-aware source
/// for Copy/Export, Rz4).
type Page = (Vec<Vec<Cell>>, RecordBatch);

fn batches_to_rows(batches: &[RecordBatch], fmt: &CellFormat) -> Result<Vec<Vec<Cell>>, String> {
    let opts = fmt.opts();
    let mut rows = Vec::new();
    for batch in batches {
        let cols = batch.columns();
        let fmts = cols
            .iter()
            .map(|c| ArrayFormatter::try_new(&**c, &opts))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for r in 0..batch.num_rows() {
            let mut row = Vec::with_capacity(fmts.len());
            for (ci, f) in fmts.iter().enumerate() {
                let null = cols[ci].is_null(r);
                let text = if null {
                    fmt.null.clone()
                } else {
                    truncate_cell(&f.value(r).to_string())
                };
                row.push(Cell { text, null });
            }
            rows.push(row);
        }
    }
    Ok(rows)
}

/// Build a structured [`QueryPlan`] for an `EXPLAIN [ANALYZE]` statement by
/// walking DataFusion's own typed plans — **no plan-text parsing**.
///
/// We plan the EXPLAIN, unwrap the `Explain`/`Analyze` wrapper to the real inner
/// `LogicalPlan`, re-plan it to a physical `ExecutionPlan`, and (for ANALYZE)
/// execute it so each operator's live `MetricsSet` is populated. Then we walk the
/// logical and physical trees into `PlanNode`s, reading each node's name,
/// one-line detail, and metrics directly from the DataFusion types.
async fn run_explain(ctx: &SessionContext, sql: &str) -> Result<QueryPlan, String> {
    let opts = SQLOptions::new()
        .with_allow_dml(false)
        .with_allow_ddl(false)
        .with_allow_statements(false);

    let df = ctx
        .sql_with_options(sql, opts)
        .await
        .map_err(|e| e.to_string())?;

    // Unwrap `EXPLAIN`/`EXPLAIN ANALYZE` to the plan being explained.
    let (inner, analyze) = match df.logical_plan() {
        LogicalPlan::Explain(e) => (e.plan.as_ref(), false),
        LogicalPlan::Analyze(a) => (a.input.as_ref(), true),
        other => (other, false),
    };

    let mut plan = QueryPlan {
        analyze,
        logical: walk_logical(inner),
        logical_text: inner.display_indent().to_string(),
        ..Default::default()
    };

    // Re-plan the inner logical plan to physical. `SessionState` has an inherent
    // `create_physical_plan` in DataFusion 43 (no `Session` trait import needed).
    let state = ctx.state();
    let physical = state
        .create_physical_plan(inner)
        .await
        .map_err(|e| e.to_string())?;

    // ANALYZE: run the query so live metrics land on the plan's operators.
    if analyze {
        let _ = collect(physical.clone(), ctx.task_ctx())
            .await
            .map_err(|e| e.to_string())?;
    }

    plan.physical = walk_physical(physical.as_ref());
    plan.physical_text = if analyze {
        DisplayableExecutionPlan::with_metrics(physical.as_ref())
            .indent(false)
            .to_string()
    } else {
        displayable(physical.as_ref()).indent(false).to_string()
    };

    if !plan.is_some() {
        return Err("Could not build the query plan".to_string());
    }
    Ok(plan)
}

/// Flatten a logical plan into depth-tagged `PlanNode`s. `LogicalPlan::display`
/// renders one node without its children (e.g. `Projection: id`).
fn walk_logical(root: &LogicalPlan) -> Vec<PlanNode> {
    fn go(p: &LogicalPlan, depth: usize, out: &mut Vec<PlanNode>) {
        let (name, detail) = crate::plan::split_name_detail(p.display().to_string().trim());
        out.push(PlanNode {
            kind: PlanKind::classify(&name),
            name,
            detail,
            depth,
            rows: None,
            self_ms: None,
            self_label: String::new(),
            metrics: Vec::new(),
        });
        for c in p.inputs() {
            go(c, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    go(root, 0, &mut out);
    out
}

/// Flatten a physical plan into depth-tagged `PlanNode`s, reading each operator's
/// one-line display and (if executed) its metrics.
fn walk_physical(root: &dyn ExecutionPlan) -> Vec<PlanNode> {
    fn go(p: &dyn ExecutionPlan, depth: usize, out: &mut Vec<PlanNode>) {
        let line = displayable(p).one_line().to_string();
        let (name, detail) = crate::plan::split_name_detail(line.trim());
        let kind = PlanKind::classify(&name);
        let (rows, metrics) = node_metrics(p);
        // Derive the one comparable per-node time (EXPLAIN_PLAN_SPEC §7) from the
        // typed metrics — logic lives in `crate::plan`, pure over `Metric`.
        let self_ms = crate::plan::self_time_ms(kind, &metrics);
        let self_label = self_ms.map(crate::plan::fmt_ms).unwrap_or_default();
        out.push(PlanNode {
            kind,
            name,
            detail,
            depth,
            rows,
            self_ms,
            self_label,
            metrics,
        });
        for c in p.children() {
            go(c.as_ref(), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    go(root, 0, &mut out);
    out
}

/// Read a physical operator's metrics: output rows (the `rows` field) plus every
/// other named metric as a typed, pre-labelled [`crate::plan::Metric`] — classified
/// by `MetricValue` variant so the UI can format + group without unit math. The raw
/// `elapsed_compute` timestamps are dropped; `output_rows` becomes `rows`.
fn node_metrics(p: &dyn ExecutionPlan) -> (Option<u64>, Vec<crate::plan::Metric>) {
    let Some(ms) = p.metrics() else {
        return (None, Vec::new());
    };
    let ms = ms.aggregate_by_name();
    let rows = ms.output_rows().map(|r| r as u64);

    let mut metrics = Vec::new();
    for m in ms.iter() {
        let mv = m.value();
        // `output_rows` is *also* kept in the list (tier-3 "Output" group) — it just
        // additionally surfaces as the headline `rows`. Timestamps aren't metrics.
        if mv.is_timestamp() {
            continue;
        }
        let kind = metric_kind(mv);
        let value = mv.as_usize() as u64;
        // Ratio/pruning have no single scalar unit → keep DataFusion's own display
        // string; everything else gets our unit-aware label.
        let label = match kind {
            crate::plan::MetricKind::Ratio => mv.to_string(),
            k => k.format(value),
        };
        metrics.push(crate::plan::Metric {
            name: mv.name().to_string(),
            value,
            kind,
            label,
            zero: value == 0,
        });
    }
    (rows, metrics)
}

/// Classify a DataFusion `MetricValue` into the UI's [`crate::plan::MetricKind`],
/// by variant first (robust — `elapsed_compute`'s name has no "time" in it), then a
/// name heuristic for the generic operator-defined `Count`/`Gauge` metrics.
fn metric_kind(v: &MetricValue) -> crate::plan::MetricKind {
    use crate::plan::MetricKind as K;
    match v {
        MetricValue::ElapsedCompute(_) | MetricValue::Time { .. } => K::Time,
        MetricValue::SpilledBytes(_) | MetricValue::OutputBytes(_) => K::Bytes,
        MetricValue::CurrentMemoryUsage(_) => K::Memory,
        MetricValue::Gauge { name, .. } if name.contains("mem") => K::Memory,
        MetricValue::Ratio { .. } | MetricValue::PruningMetrics { .. } => K::Ratio,
        MetricValue::Count { name, .. } if name.contains("bytes") => K::Bytes,
        MetricValue::Count { name, .. } if name.contains("mem") => K::Memory,
        MetricValue::Count { name, .. } if name.contains("time") => K::Time,
        _ => K::Count,
    }
}

fn truncate_cell(s: &str) -> String {
    if s.len() <= MAX_CELL_LEN {
        return s.to_string();
    }
    let mut end = MAX_CELL_LEN;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// ---- external table registration ----

async fn register_external(ctx: &SessionContext, spec: &TableSpec) -> Result<TableMeta, String> {
    use datafusion::datasource::file_format::arrow::ArrowFormat;
    use datafusion::datasource::file_format::csv::CsvFormat;
    use datafusion::datasource::file_format::json::JsonFormat;
    use datafusion::datasource::file_format::parquet::ParquetFormat;
    use datafusion::datasource::file_format::FileFormat;
    use datafusion::datasource::listing::{
        ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
    };

    let _ = ctx.deregister_table(spec.name.as_str());

    let mut urls = Vec::new();
    for p in &spec.paths {
        if p.trim().is_empty() {
            continue;
        }
        let mut loc = p.clone();
        if Path::new(&loc).is_dir() && !loc.ends_with('/') {
            loc.push('/');
        }
        urls.push(ListingTableUrl::parse(&loc).map_err(|e| e.to_string())?);
    }
    if urls.is_empty() {
        return Err("No source paths".into());
    }

    let (fmt, ext): (Arc<dyn FileFormat>, &str) = match spec.format.as_str() {
        "csv" => (Arc::new(CsvFormat::default()), ".csv"),
        "json" => (Arc::new(JsonFormat::default()), ".json"),
        "arrow" => (Arc::new(ArrowFormat), ".arrow"),
        _ => (
            Arc::new(ParquetFormat::default().with_skip_metadata(true)),
            ".parquet",
        ),
    };
    // `with_session_config_options` *before* any explicit option: it carries the
    // session's `collect_statistics` (and `target_partitions`) onto the options and
    // would otherwise clobber them.
    //
    // It is not optional. `ListingOptions::new` hardcodes `collect_stat: false`, and a
    // hand-built `ListingTable` never picks the `datafusion.execution.collect_statistics`
    // key up on its own — `ListingTableConfig::with_listing_options` does no such wiring.
    // Without this, every footer statistic comes back `Absent` while the engine setting
    // claims to be on. It's baked in at `try_new`, so a registered table can't be fixed
    // after the fact — `rebuild_listing` inherits it by cloning `lt.options()`.
    let mut opts = ListingOptions::new(fmt)
        .with_session_config_options(&ctx.copied_config())
        .with_file_extension(ext);
    if !spec.partitions.is_empty() {
        let cols = spec
            .partitions
            .iter()
            .map(|(n, ty)| (n.clone(), parse_dtype(ty)))
            .collect();
        opts = opts.with_table_partition_cols(cols);
    }

    let config = ListingTableConfig::new_with_multi_paths(urls)
        .with_listing_options(opts)
        .infer_schema(&ctx.state())
        .await
        .map_err(|e| e.to_string())?;
    let table = ListingTable::try_new(config).map_err(|e| e.to_string())?;
    ctx.register_table(spec.name.as_str(), Arc::new(table))
        .map_err(|e| e.to_string())?;

    table_meta(ctx, spec.name.as_str()).await
}

/// Rebuild a registered `ListingTable` from its own `paths` + `options`, re-inferring
/// the schema, and re-register it under `name` — the schema-refresh step
/// (`RefreshCatalog`). Re-registering the *same* provider wouldn't re-infer, so we
/// construct a fresh table from a re-`infer_schema`d config. Returns its columns + free
/// metadata — `opts` is the live table's own, so `collect_stat` carries over with it.
async fn rebuild_listing(
    ctx: &SessionContext,
    name: &str,
    paths: Vec<datafusion::datasource::listing::ListingTableUrl>,
    opts: datafusion::datasource::listing::ListingOptions,
) -> Result<TableMeta, String> {
    use datafusion::datasource::listing::{ListingTable, ListingTableConfig};
    let config = ListingTableConfig::new_with_multi_paths(paths)
        .with_listing_options(opts)
        .infer_schema(&ctx.state())
        .await
        .map_err(|e| e.to_string())?;
    let table = ListingTable::try_new(config).map_err(|e| e.to_string())?;
    let _ = ctx.deregister_table(name);
    ctx.register_table(name, Arc::new(table))
       .map_err(|e| e.to_string())?;
    table_meta(ctx, name).await
}

// ---- schema helpers ----

/// What a view plan reads (D10): its **base tables** and the **names it inlines**.
///
/// Asks the planner, not the SQL text, which matters three ways:
///
/// - **Views are already resolved away.** DataFusion 54 inlines a view at plan-*build*
///   time, so a plan from `ctx.table("a_view")` scans the view's base tables directly.
///   That's transitive for free — a view over a view was inlined when the inner one was
///   planned, so `C → B → A → orders` collapses to a single tree carrying `orders` at
///   the leaf and `A`, `B` as the inliner's alias markers on the way down. Reading the
///   SQL would stop at `FROM b`.
/// - **`apply_with_subqueries`, not `apply`.** Plain `apply` visits only direct
///   children, so a view with `WHERE id IN (SELECT id FROM other)` would silently drop
///   `other` — and a *missed* dependency is the failure that matters: a stale profile
///   nobody invalidates, or an entry dropped without warning.
/// - **`.table()`, not `to_string()`.** A `TableReference` renders as written — `t`
///   here, `public.t` there — so `to_string()` yields two keys for one thing. The engine
///   owns a single schema, so the bare name is the identity.
struct PlanDeps {
    /// Base tables scanned — for profile invalidation and the table-drop warning.
    tables: Vec<String>,
    /// Every `SubqueryAlias` name, which for an inlined sub-view is the view's own name.
    /// Raw: also includes plain table aliases (`FROM t AS x`) and CTE names, since those
    /// are indistinguishable from a view inline in the plan. The UI keeps only the ones
    /// that are actually views. Recursion is automatic — a chain leaves one alias per
    /// hop in the tree, so this is the transitive set of referenced views.
    aliases: Vec<String>,
}

fn plan_deps(plan: &datafusion::logical_expr::LogicalPlan) -> PlanDeps {
    use datafusion::common::tree_node::TreeNodeRecursion;
    use datafusion::logical_expr::LogicalPlan;
    let mut tables = std::collections::BTreeSet::new();
    let mut aliases = std::collections::BTreeSet::new();
    let _ = plan.apply_with_subqueries(|node| {
        match node {
            LogicalPlan::TableScan(scan) => {
                // A source still carrying its own plan is a view that *didn't* inline —
                // only reachable if filters were pushed at build time, which our path
                // never does. Recording it would name the view instead of what it reads.
                if scan.source.get_logical_plan().is_none() {
                    tables.insert(scan.table_name.table().to_string());
                }
            }
            LogicalPlan::SubqueryAlias(a) => {
                aliases.insert(a.alias.table().to_string());
            }
            _ => {}
        }
        Ok(TreeNodeRecursion::Continue)
    });
    PlanDeps {
        tables: tables.into_iter().collect(),
        aliases: aliases.into_iter().collect(),
    }
}

/// Profile `name` — one full scan, every column at once (see [`crate::profile`]).
///
/// Runs on this worker like any other command, so the UI stays live and the row's
/// `profiling` flag drives the spinner. Blocking is fine here; it's *meant* to be the
/// expensive thing the user opted into.
async fn run_profile(ctx: &SessionContext, name: &str) -> Result<crate::profile::CatalogProfile, String> {
    let df = ctx.table(name).await.map_err(|e| e.to_string())?;
    let columns: Vec<ColumnInfo> = df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect();
    let (exprs, slots) = crate::profile::aggregates(&columns);
    // Render *before* executing, from the same `Expr`s that are about to run, so "view
    // as query" can't drift from the facts it produced. Not `plan_to_sql` on the whole
    // plan: that inlines a view's body and names no view (see `profile_sql`).
    let sql = crate::profile::profile_sql(name, &exprs);
    let batches = df
        .aggregate(vec![], exprs)
        .map_err(|e| e.to_string())?
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    let batch = batches.first().ok_or("profile returned no batches")?;
    let mut profile = crate::profile::decode(&slots, batch, &columns)?;
    profile.sql = sql;
    Ok(profile)
}

/// A table's columns plus its **free** metadata — the row count and per-column
/// min/max/nulls, read from the source's own footers. One metadata read per file, no
/// data pages. Everything lands `None` for a source that reports nothing (CSV/JSON),
/// which the inspector renders as an absent row rather than a guess.
async fn table_meta(ctx: &SessionContext, name: &str) -> Result<TableMeta, String> {
    let df = ctx.table(name).await.map_err(|e| e.to_string())?;
    // `|f| column_info(f)`, not `column_info`: `fields()` yields `&Arc<Field>` and the
    // deref coercion to `&Field` only happens at a call site.
    let mut columns: Vec<ColumnInfo> = df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect();
    let rows = free_stats(ctx, name, &mut columns).await;
    Ok(TableMeta { columns, rows })
}

/// Fold the source's free statistics onto `columns`, returning the row count. Best
/// effort throughout: anything unavailable simply stays `None`.
async fn free_stats(ctx: &SessionContext, name: &str, columns: &mut [ColumnInfo]) -> Option<u64> {
    use datafusion::datasource::listing::ListingTable;
    let provider = ctx.table_provider(name).await.ok()?;
    // Only a `ListingTable` has files whose footers can be read — a view has none.
    let lt = provider.downcast_ref::<ListingTable>()?;
    let state = ctx.state();
    // `limit: None` — a limit would make the aggregate inexact.
    let stats = lt.list_files_for_scan(&state, &[], None).await.ok()?.statistics;
    let rows = stats.num_rows.get_value().map(|n| *n as u64);
    // Zip rather than index: DataFusion promises one entry per *table*-schema field, but
    // a table with no files short-circuits to `file_schema`, which omits the partition
    // columns — indexing would then misattribute every stat.
    for (col, cs) in columns.iter_mut().zip(stats.column_statistics.iter()) {
        // Push only what's actually there — an absent fact is an absent row, not a
        // blank one. Display order.
        let nulls = match cs.null_count.get_value() {
            // `Exact(num_rows)` is *also* DataFusion's "no stats for this column"
            // fallback, so an all-null column and an unknown one are indistinguishable.
            // Say nothing; the profile answers it for real with a COUNT ... FILTER.
            Some(n) if Some(*n as u64) == rows => None,
            Some(n) => Some(Stat {
                key: StatKey::Nulls,
                text: n.to_string(),
                exact: true,
            }),
            None => None,
        };
        col.stats = [nulls, stat_of(StatKey::Min, &cs.min_value), stat_of(StatKey::Max, &cs.max_value)]
            .into_iter()
            .flatten()
            .collect();
    }
    rows
}

/// A `Precision<ScalarValue>` as a display [`Stat`]. `Absent` → `None` (say nothing).
/// A null value means the column is in the arrow schema but absent from the source's
/// own (schema evolution) — also nothing to report. `Inexact` carries through flagged.
fn stat_of(
    key: StatKey,
    p: &datafusion::common::stats::Precision<datafusion::common::ScalarValue>,
) -> Option<Stat> {
    let v = p.get_value()?;
    if v.is_null() {
        return None;
    }
    Some(Stat {
        key,
        text: v.to_string(),
        exact: p.is_exact().unwrap_or(false),
    })
}

fn column_info(field: &Field) -> ColumnInfo {
    let dtype = short_type(field.data_type());
    ColumnInfo {
        name: field.name().clone(),
        kind: Kind::from_arrow(&dtype),
        dtype,
        nullable: field.is_nullable(),
        children: nested_children(field.data_type()),
        // Filled by `free_stats` where the source has metadata to read; a nested child
        // never gets any — footers describe leaves, and we don't traverse into them.
        stats: Vec::new(),
    }
}

fn nested_children(dt: &DataType) -> Vec<ColumnInfo> {
    match dt {
        DataType::Struct(fields) => fields.iter().map(|f| column_info(f)).collect(),
        DataType::List(f) | DataType::LargeList(f) | DataType::FixedSizeList(f, _) => {
            vec![column_info(f)]
        }
        DataType::Map(entries, _) => nested_children(entries.data_type()),
        _ => Vec::new(),
    }
}

fn short_type(dt: &DataType) -> String {
    let full = format!("{dt:?}");
    let base: String = full.split(['(', '<']).next().unwrap_or(&full).to_string();
    match base.as_str() {
        "LargeUtf8" => "Utf8".into(),
        "LargeList" | "FixedSizeList" => "List".into(),
        other => other.to_string(),
    }
}

fn parse_dtype(label: &str) -> DataType {
    match label {
        "Int8" => DataType::Int8,
        "Int16" => DataType::Int16,
        "Int32" => DataType::Int32,
        "Int64" => DataType::Int64,
        "Float32" => DataType::Float32,
        "Float64" => DataType::Float64,
        "Date" | "Date32" => DataType::Date32,
        _ => DataType::Utf8,
    }
}
