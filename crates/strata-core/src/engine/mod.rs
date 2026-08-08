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
//! Profiling ([`Engine::profile`]) is the third thing the engine tracks, beside runs and
//! snapshots: one full scan per catalog entry, keyed by the entry rather than by a
//! workspace, because a profile is a property of the *data* and not of any tab.
//!
//! The facade grows one method per feature that lands in the Freya app; the
//! underlying logic lives in the sibling modules (`query`, `explain`, `catalog`,
//! `export`, `profile`) as plain async functions over `&SessionContext`.
//!
//! (The retired `Command`/`Event` channel protocol this replaces lives on only in
//! `crates/strata-dioxus`, which is reference code and no longer builds.)

mod arrow_stats;
mod catalog;
mod chart;
pub mod config;
mod ddl;
mod explain;
pub mod export;
mod functions;
pub mod json_poly;
pub mod plan;
pub mod profile;
mod providers;
mod query;
pub mod serialize;
pub mod sql;
/// `pub` for the connection editor, which offers the client options this module knows how to
/// apply ([`store::CLIENT_KEYS`]) and refuses the ones it does not ([`store::check_client_config`])
/// — the same call `connect` makes, so a form and the store cannot disagree about an option.
pub mod store;
pub mod value_tree;

/// [`column_info`] and [`chart_role`] are `pub` because a column's vocabulary row is derived
/// from an Arrow field in exactly one place, and anything building a column — a fixture
/// included — should go through it rather than hand-writing a row whose `kind` and `role` are
/// then a second opinion about the same type.
pub use catalog::{chart_role, column_info, TableMeta, TableSpec, ViewMeta};
/// The intercepted-statement vocabulary (ED-02): what an arm answers with, what the app folds.
/// [`drop_intent`](ddl::drop_intent) rides with them because a drop's wording is the engine's
/// (ED-05) — the catalog's confirm says before the fact what the report says after it.
pub use ddl::{drop_intent, StatementOutcome, StatementReport, StoreEffect};
pub use query::purge_snapshot_root;

use sql::{PolicyRefusal, Verdict};

/// The Arrow batch type engine results carry (the type-aware source for Copy/Export),
/// re-exported so frontends can name it without their own DataFusion dependency (this
/// crate is the one DataFusion boundary).
pub use datafusion::arrow::record_batch::RecordBatch;

/// The Arrow schema type, re-exported for the same reason — code (and tests) holding a
/// [`RecordBatch`] sometimes needs to name its schema.
pub use datafusion::arrow::datatypes::Schema;

/// A call the caller (or the app on their behalf) **stopped**: [`Engine::cancel`] aborted it, or
/// [`Engine::cancel_profile`] did.
pub const CANCELLED: &str = "cancelled";
/// A run that finished but was no longer the latest dispatch for its workspace — a newer press
/// replaced it, so its result is discarded and its snapshot retired ([`Engine::query`]).
pub const SUPERSEDED_RUN: &str = "superseded by a newer run";
/// The scan equivalent: a re-scan replaced this one ([`Engine::profile`]).
pub const SUPERSEDED_SCAN: &str = "superseded by a newer scan";

/// Did this `Err` mean the call was **stopped**, rather than that it *failed*?
///
/// The three strings above are one concept with three causes, and the distinction matters at every
/// surface that shows a settled error: a stopped call is news the user already has (they cancelled,
/// or they pressed Run again), so it must never be presented as a fault.
///
/// **A named predicate rather than a literal at each call site**, because the consumers used to
/// match the engine's own prose: `state::log::run_event` compared against `"cancelled"` (and so
/// mapped a *supersede* to a red `Error` row reading "superseded by a newer run"), while the
/// inspector's scan zone did `== "cancelled" || starts_with("superseded")`. Two copies of one rule,
/// each able to drift from the strings this module actually produces — and one of them already had.
pub fn stopped_on_purpose(error: &str) -> bool {
    matches!(error, CANCELLED | SUPERSEDED_RUN | SUPERSEDED_SCAN)
}

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use datafusion::common::TableReference;
use datafusion::execution::runtime_env::RuntimeEnv;
use datafusion::prelude::*;
use tokio::runtime::{Builder, Runtime};
use tokio::task::AbortHandle;

use crate::engine::plan::QueryPlan;
use providers::StrataCatalogProvider;
use query::{
    claim_snapshot_dir, discard_snapshot_dir, retire_snapshot, run_and_snapshot, CellFormat,
};
use sql::FunctionCatalog;
use strata_model::{
    Cell, ChartData, ChartQuery, ConnectionDef, Diagnostic, QueryOutput, SnapshotId, TabId,
};

/// A workspace's stable identity — the query tab that owns a run and its current
/// snapshot (`docs/SNAPSHOT_SPEC.md` §4). Wide enough that a frontend passes its
/// **native** tab id (the Freya `TabId` is a Uuid → `as_u128`) rather than
/// maintaining a parallel one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WsId(pub u128);

impl From<TabId> for WsId {
    /// A tab *is* an engine workspace — its `Uuid` widened to the `u128` key.
    fn from(tab: TabId) -> Self {
        WsId(tab.0.as_u128())
    }
}

/// One dispatched run's identity **as the UI knows it** — the per-press nonce
/// (`QuerySpec::run`), passed down so [`Engine::cancel`] can tell "still this run" from
/// "a newer run replaced it" without a parallel request-id scheme.
///
/// It is the *caller's* nonce, so it is not unique here: the same tag can legitimately be
/// dispatched twice (freya-query re-runs an entry when a subscriber remounts while it is
/// still in flight). Engine-side lifecycle therefore keys on `InFlight::dispatch`, not on
/// this — see [`Engine::query`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RunTag(pub u128);

/// What a **Run** settled to ([`Engine::run`], ED-02) — the two things a press can produce.
///
/// The split is the router's, not a mode the caller picks: a Run is one press, and whether it
/// produces rows or performs a statement is a property of what was typed.
pub enum RunOutcome {
    /// Exactly [`Engine::query`]'s answer — the snapshot handle + page 1. Byte-for-byte the
    /// path that shipped: same supersede, same retire-on-dispatch, same pins.
    Rows(QueryOutput, RecordBatch),
    /// An intercepted statement's report. **No snapshot**, and none retired: a tab that
    /// creates a table can still page the result it already had
    /// (`docs/SNAPSHOT_SPEC.md` §4 — DDL does not retire snapshots).
    Statement(StatementReport),
}

/// Process-unique id per engine (one per project window), scoping snapshot files so
/// windows never collide.
static ENGINE_SEQ: AtomicU64 = AtomicU64::new(0);

/// An in-flight **profile scan** (D4): which dispatch it is, and the handle that cancels it.
///
/// Keyed by catalog entry rather than by workspace, because a profile belongs to the *data*:
/// it is asked for from the catalog, cached per entry, and two tables profile concurrently.
/// There is no snapshot — a scan materializes one aggregate row and returns it.
struct ProfileRun {
    /// Engine-unique, monotonic — the same "am I still the latest?" check [`InFlight`] uses,
    /// for the same reason: a re-scan supersedes, and the superseded call must not tear down
    /// the entry the newer one now owns.
    dispatch: u64,
    abort: AbortHandle,
}

/// A workspace's in-flight run or explain: which dispatch it is, the snapshot it is
/// materializing (`None` for an explain), and the abort handle that cancels it.
struct InFlight {
    /// Engine-unique, monotonic — the identity every "am I still the latest run?" check
    /// compares. A [`RunTag`] can't do that job: it is the caller's nonce, and a repeat
    /// dispatch of the same tag would make the superseded call mistake the *new* entry
    /// for its own and tear down state it doesn't own.
    dispatch: u64,
    /// The caller's nonce, kept for exactly one thing: [`Engine::cancel`]'s guard.
    tag: RunTag,
    snapshot: Option<SnapshotId>,
    abort: AbortHandle,
    start: Instant,
}

/// Undoes a dispatch whose caller went away before it could settle.
///
/// A dispatch publishes its [`InFlight`] entry *before* awaiting the spawned work, so until
/// the settle path runs the workspace looks busy. That was safe while every caller was
/// freya-query, which by design never cancels an execution — but an agent's run (AA-03b) is
/// awaited inside an MCP request future, and a client cancellation, a dropped connection or
/// the agent server shutting down all drop it mid-await. Without this the entry is never
/// removed: [`publish_inflight`](Engine::publish_inflight) latches the window's in-flight flag
/// on for the engine's life, so every later close, re-root and restart asks the T2 confirm
/// about a query that finished long ago, `is_running` reports a phantom, and the snapshot the
/// detached task materialized is never retired.
///
/// Armed for the await and [`disarm`](Self::disarm)ed by the settle path, so the ordinary
/// route pays nothing. The drop repeats the settle path's own `latest` check for the reason
/// that check exists: a superseded call must not tear down the entry a newer dispatch owns.
struct DispatchGuard<'a> {
    engine: &'a Engine,
    ws: WsId,
    dispatch: u64,
    armed: bool,
}

impl<'a> DispatchGuard<'a> {
    fn arm(engine: &'a Engine, ws: WsId, dispatch: u64) -> Self {
        Self {
            engine,
            ws,
            dispatch,
            armed: true,
        }
    }

    /// The dispatch settled on its own terms; leave the entry to the settle path.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // `lock()` rather than `unwrap()` on the guard: this runs during a drop, which may
        // itself be an unwind, and a panic there aborts the process.
        let Ok(mut lc) = self.engine.lifecycle.lock() else {
            return;
        };
        if lc.inflight.get(&self.ws).map(|f| f.dispatch) != Some(self.dispatch) {
            return;
        }
        if let Some(f) = lc.inflight.remove(&self.ws) {
            // Aborts the detached task and retires whatever it managed to materialize —
            // the same teardown `cleanup_ws` performs, for the same reason.
            self.engine.abort_inflight(f);
        }
        self.engine.publish_inflight(&lc);
    }
}

/// The engine's lifecycle bookkeeping, all under one lock (never held across an await):
/// which run is in flight per workspace, which snapshot each workspace currently owns, and
/// which catalog entries are being profiled.
#[derive(Default)]
struct Lifecycle {
    inflight: HashMap<WsId, InFlight>,
    current: HashMap<WsId, SnapshotId>,
    /// In-flight profile scans by entry identity ([`fold_ident`] of the name — tables and
    /// views share one namespace).
    profiles: HashMap<String, ProfileRun>,
    /// How many pieces of **background** work are in flight — an export writing a file, a
    /// drop deleting a table's data (ED-05). A **count, not a map**: nothing addresses one of
    /// these — no cancel, no supersede, no per-item state to look up. All it has to do is keep
    /// [`publish_inflight`](Engine::publish_inflight) true while something is half-done, so the
    /// close-while-running confirm asks before the window takes the runtime away.
    ///
    /// Not per-kind, because the question every reader asks is the same one: is anything the
    /// user would rather finish still going? A second counter would be a second answer to it.
    background: usize,
    /// Snapshots a caller is **holding open**, and how many holds each has
    /// ([`Engine::pin_snapshot`]). A pinned snapshot survives its workspace re-running.
    pins: HashMap<SnapshotId, usize>,
    /// Snapshots whose retire arrived while they were pinned. They are retired for real
    /// when the last pin releases — deferred, never skipped, so nothing leaks.
    deferred: HashSet<SnapshotId>,
    /// What each live snapshot's write pass observed ([`query::SnapshotStats`]) — today the
    /// exact per-column null counts a partitioned export has to check.
    ///
    /// Here rather than in the file because a snapshot never outlives its process, so this has
    /// exactly its lifetime: inserted when it materializes, dropped when it retires. The Arrow
    /// IPC snapshot carries no statistics of its own, and asking the file was never the point —
    /// the write pass already streams every batch.
    stats: HashMap<SnapshotId, query::SnapshotStats>,
}

/// A window's engine. Create once per project window (cheap to share as `Arc<Engine>`);
/// dropping it aborts in-flight work and removes its snapshot directory.
pub struct Engine {
    engine_id: u64,
    /// DataFusion's home: the private multi-thread runtime every call spawns onto.
    /// `Option` only so `Drop` can take it for a context-safe `shutdown_background`
    /// (a plain field drop panics when the engine is dropped inside another runtime,
    /// e.g. a `#[tokio::test]`); always `Some` while the engine lives.
    rt: Option<Runtime>,
    ctx: SessionContext,
    /// The `datafusion.*` config overrides this engine runs with (W2). Mutex'd because
    /// [`set_config`](Engine::set_config) re-points a live engine at a new set.
    overrides: Mutex<BTreeMap<String, String>>,
    /// The `datafusion.runtime.*` overrides this engine was **built** with. The `RuntimeEnv`
    /// is fixed when the `SessionContext` is built, so this — not the live `overrides` — is
    /// what [`restart_owed`](Engine::restart_owed) measures against: a user who declines the
    /// restart keeps the new values in `overrides`, and comparing the two maps to each other
    /// would then report "nothing changed" and never offer the restart again.
    built_runtime: BTreeMap<String, String>,
    /// Snapshot-id allocator — ids are per-engine unique for the process lifetime,
    /// which is what makes a snapshot immutable-by-identity.
    snap_seq: AtomicU64,
    /// Dispatch-id allocator — see [`InFlight::dispatch`].
    dispatch_seq: AtomicU64,
    /// The exclusive lock on this engine's snapshot directory, held open for the engine's
    /// whole life: it is what tells *another* process's startup purge that these
    /// snapshots are live (`query::claim_snapshot_dir`). Never read — closing it is the
    /// entire contract, so it drops with the engine, after `Drop` has cleaned up.
    _snapshot_lock: Option<File>,
    lifecycle: Mutex<Lifecycle>,
    /// Optional mirror of "this engine has work in flight", published on every lifecycle
    /// mutation for readers that can reach neither the lock nor async code — the window's
    /// winit close hook (T2), which runs outside the UI and must be `Send`. Installed once
    /// by [`Engine::watch_inflight`]; `None` until then, and for engines nobody watches.
    inflight_flag: OnceLock<Arc<AtomicBool>>,
    /// The registered SQL functions (built-ins + UDFs), enumerated once at build for
    /// the language service (S26/S7/S25).
    functions: FunctionCatalog,
    /// The project folder this engine may write internal tables into (ED-04), set at project
    /// open by whichever host owns it — see [`set_data_dir`](Engine::set_data_dir). `None` until
    /// then, and forever for an engine with no project behind it.
    data_root: Mutex<Option<PathBuf>>,
    /// Which registered tables are **internal** — see [`InternalTables`].
    internal: InternalTables,
}

/// The engine-side set of tables whose data Strata owns — [`fold_ident`]ed names (ED-04).
///
/// Derived state, rebuilt by the same registration pass that builds everything else, and
/// deliberately **not a second catalog**: it holds names and nothing else, and answers exactly
/// one engine-side question — may a write statement target this provider (ED-05). Everything
/// the UI asks about the catalog is still the store's to answer.
///
/// **Shared by handle rather than borrowed**, because the two statements that ask it —
/// `INSERT`, which gates on it, and `DROP TABLE`, which takes the origin from it — run inside
/// the task [`bookkeep`](Engine::bookkeep) spawned, and that task must not hold the engine
/// itself: the engine's `Drop` is what aborts it, so a task keeping the engine alive would
/// keep the abort from ever arriving. This holds names only, so it outlives an engine
/// harmlessly.
#[derive(Clone, Debug, Default)]
pub struct InternalTables(Arc<Mutex<HashSet<String>>>);

impl InternalTables {
    /// Whether `name` is a table whose data Strata owns. `false` for an external table, a view,
    /// and a name that is not registered at all.
    pub fn contains(&self, name: &str) -> bool {
        self.0.lock().unwrap().contains(&fold_ident(name))
    }

    /// Record what a registration (or a drop) settled about a table's origin.
    fn note(&self, name: &str, internal: bool) {
        let mut set = self.0.lock().unwrap();
        match internal {
            true => set.insert(fold_ident(name)),
            // Not `if internal` — a def that *was* internal and is now registered over the same
            // name as an external one has to stop being one.
            false => set.remove(&fold_ident(name)),
        };
    }
}

impl Engine {
    /// Build a window's engine, honouring the given `datafusion.*` `overrides` (W2).
    pub fn new(overrides: BTreeMap<String, String>) -> Engine {
        let engine_id = ENGINE_SEQ.fetch_add(1, Ordering::Relaxed);
        let rt = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name(format!("df-engine-{engine_id}"))
            .enable_all()
            .build()
            .expect("tokio runtime");
        let ctx = build_context(&overrides);
        let functions = functions::snapshot(&ctx);
        // Claim the snapshot directory before anything can write to it. Failing is
        // survivable (the directory is created on demand anyway) but means another
        // instance's startup purge can't see that we're alive — worth saying out loud,
        // with the reason, since that purge is then the thing that deletes live results.
        let snapshot_lock = match claim_snapshot_dir(engine_id) {
            Ok(lock) => Some(lock),
            Err(why) => {
                tracing::warn!(
                    "engine {engine_id}: could not claim {} ({why}); its snapshots are \
                     unprotected against another instance's startup purge",
                    query::snapshot_dir(engine_id)
                );
                None
            }
        };
        Engine {
            engine_id,
            rt: Some(rt),
            ctx,
            built_runtime: runtime_subset(&overrides),
            overrides: Mutex::new(overrides),
            snap_seq: AtomicU64::new(1),
            dispatch_seq: AtomicU64::new(1),
            _snapshot_lock: snapshot_lock,
            lifecycle: Mutex::default(),
            inflight_flag: OnceLock::new(),
            functions,
            data_root: Mutex::default(),
            internal: InternalTables::default(),
        }
    }

    /// Tell this engine which **project folder** it belongs to (ED-04).
    ///
    /// `root` is the project folder, not `.strata/tables`, because a statement that creates an
    /// internal table needs both: the absolute directory to spool into, and the project-relative
    /// path the def stores, which is what lets the def travel with `project.json`. The layout
    /// between them is [`project::tables_dir`](crate::project::tables_dir)'s, in one place.
    ///
    /// Every host that opens a project calls this — the app window and the headless server both.
    /// An engine that is never told refuses to *create* a table (politely, naming the reason) and
    /// is otherwise unaffected: a project's existing internal defs replay through the ordinary
    /// registration pass, whose source paths were already resolved against the root by the
    /// caller.
    pub fn set_data_dir(&self, root: &Path) {
        *self.data_root.lock().unwrap() = Some(root.to_path_buf());
    }

    /// Whether `name` is a table whose data Strata owns — the one question the internal-name set
    /// exists to answer (see [`InternalTables`]). `false` for an external table, a view, and a
    /// name that is not registered at all.
    pub fn is_internal(&self, name: &str) -> bool {
        self.internal.contains(name)
    }

    /// Record what a registration settled about a table's origin. Called from every path that
    /// registers one, so the set is rebuilt by the pass rather than maintained beside it.
    fn note_origin(&self, name: &str, internal: bool) {
        self.internal.note(name, internal);
    }

    /// Mirror "this engine has work in flight" into `flag` for the rest of its life — the
    /// truth behind the close-while-running confirm (T2). Call once, when the window wires
    /// its close guard.
    ///
    /// The engine is the **only** thing that knows: a run belongs to a workspace, not to a
    /// mounted view, so a UI-side derivation only ever sees the tab whose results pane is
    /// mounted and reads "idle" the moment the user switches tabs on a running query.
    /// Publishing happens inside every lifecycle mutation, under the same lock, so the flag
    /// cannot drift from `inflight`.
    pub fn watch_inflight(&self, flag: Arc<AtomicBool>) {
        // Take the lock first: installing and seeding must be atomic against a concurrent
        // dispatch/settle, or the seed below could overwrite a fresher publish.
        let lc = self.lifecycle.lock().unwrap();
        if self.inflight_flag.set(flag).is_err() {
            tracing::error!(
                "engine {}: in-flight flag already installed",
                self.engine_id
            );
            return;
        }
        self.publish_inflight(&lc);
    }

    /// Publish "this engine has work in flight" to the installed flag. Called from **every**
    /// mutation of `Lifecycle::inflight` / `Lifecycle::profiles`, with the lock held, so a
    /// reader can never see a flag that disagrees with the maps.
    ///
    /// A **profile counts** (D4): a scan is the most expensive thing the app does, and closing
    /// the window would throw it away — exactly what the confirm exists to ask about. The
    /// per-tab probe below deliberately does not, because a profile is not a tab's work.
    ///
    /// An **export counts** for a stronger reason than either: closing mid-write doesn't lose
    /// work, it leaves a truncated file (or a half-built Hive tree) on the user's disk under
    /// the name they chose. Like a profile, it is nobody's tab, so the per-tab probe ignores it.
    fn publish_inflight(&self, lc: &Lifecycle) {
        if let Some(flag) = self.inflight_flag.get() {
            flag.store(
                !lc.inflight.is_empty() || !lc.profiles.is_empty() || lc.background > 0,
                Ordering::Relaxed,
            );
        }
    }

    /// Whether workspace `ws` has a run or explain executing right now — the per-tab half
    /// of the close-while-running confirm (a tab *is* a [`WsId`]). Same reason as
    /// [`watch_inflight`](Engine::watch_inflight): a background tab's run is invisible to
    /// the UI, which mounts only the active tab's results.
    pub fn is_running(&self, ws: WsId) -> bool {
        self.lifecycle.lock().unwrap().inflight.contains_key(&ws)
    }

    /// Whether anything **other than a workspace run** is in flight: a profile scan, an export,
    /// or a drop deleting a table's data.
    ///
    /// The close confirm's gate is `watch_inflight`'s flag, which is runs ∪ profiles ∪
    /// background — so a surface deciding *whose* work is at stake cannot answer from
    /// [`is_running`](Engine::is_running) alone. Enumerating the workspaces it knows about and
    /// assuming the rest is idle is exactly the wrong answer: a scan, an export or a drop in
    /// flight is the user's own work, and a dialog that named an agent instead would ask them to
    /// destroy it under the wrong sentence.
    pub fn has_background_work(&self) -> bool {
        let lc = self.lifecycle.lock().unwrap();
        !lc.profiles.is_empty() || lc.background > 0
    }

    /// This engine's process-unique id — what makes a [`SnapshotId`] meaningful.
    ///
    /// Snapshot ids are a **per-engine** counter, so a restart mints `1` again: anything that
    /// remembers a snapshot across a possible rebuild has to remember which engine minted it,
    /// or it will read a *different* result that happens to share the number.
    pub fn id(&self) -> u64 {
        self.engine_id
    }

    /// The registered SQL functions (the editor's language catalog).
    pub fn functions(&self) -> &FunctionCatalog {
        &self.functions
    }

    /// Re-point this live engine at `overrides` (Settings ▸ Engine ▸ Properties, W2), and
    /// answer whether a restart is still owed.
    ///
    /// Every `ConfigOptions` key is written straight onto the live `SessionState`, so the
    /// next query planned sees it. A key the user **removed** is not skipped — it is set back
    /// to the built-in default from [`config::ENGINE_KEYS`], because leaving it at the value
    /// that was just deleted is the one outcome nobody asked for. That completes the mapping:
    /// the keys `ConfigOptions` accepts are exactly the ones the catalog names a default for,
    /// so every key that ever applied can also be un-applied. (An unrecognised
    /// `datafusion.*` key was already rejected by `set` at build time and stays rejected
    /// here — it never took effect, so removing it is a no-op either way.)
    ///
    /// `datafusion.runtime.*` is the exception, and the reason this returns anything: those
    /// configure the `RuntimeEnv`, which is fixed when the `SessionContext` is built. They
    /// are recorded, not applied, and `true` means the caller owes the user a restart.
    pub fn set_config(&self, overrides: BTreeMap<String, String>) -> bool {
        let mut current = self.overrides.lock().unwrap();
        if *current != overrides {
            let state = self.ctx.state_ref();
            let mut state = state.write();
            let options = state.config_mut().options_mut();
            let touched: BTreeSet<&String> = current.keys().chain(overrides.keys()).collect();
            for key in touched {
                if config::is_restart_key(key) || config::is_owned_key(key) {
                    continue;
                }
                let value = match overrides.get(key) {
                    Some(value) => value.as_str(),
                    None => match config::key_def(key) {
                        Some(def) => def.default,
                        // Never applied, so there is nothing to put back.
                        None => continue,
                    },
                };
                if let Err(e) = options.set(key, value) {
                    tracing::warn!("engine config: skipping {key}={value}: {e}");
                }
            }
            *current = overrides;
        }
        runtime_subset(&current) != self.built_runtime
    }

    /// Whether this engine's `datafusion.runtime.*` overrides have moved on from the ones its
    /// `RuntimeEnv` was built with — i.e. whether a restart would change anything. Stays true
    /// until the engine is actually rebuilt, so a declined restart can be offered again.
    pub fn restart_owed(&self) -> bool {
        runtime_subset(&self.overrides.lock().unwrap()) != self.built_runtime
    }

    /// The `datafusion.*` overrides this engine is running with.
    pub fn overrides(&self) -> BTreeMap<String, String> {
        self.overrides.lock().unwrap().clone()
    }

    /// Validate `sql` against this engine's live session (P2-18): lexical lints,
    /// managed-DDL policy, and a **dry-plan** of each statement — parse → resolve →
    /// analyze, never execute — so the diagnostics are exactly the errors a Run would
    /// hit. Total by design: faults come back as `Diagnostic`s, not an `Err`.
    pub async fn validate(&self, sql: String) -> Vec<Diagnostic> {
        let ctx = self.ctx.clone();
        let functions = self.functions.clone();
        self.rt()
            .spawn(async move { sql::validate(&ctx, &functions, &sql).await })
            .await
            .unwrap_or_default()
    }

    /// The managed-DDL policy over `sql` as a pre-dispatch gate (AA-01): the same
    /// classification [`validate`](Engine::validate) squiggles, parsed with this
    /// session's own dialect on the engine runtime. `Ok(vec![])` is a clean pass;
    /// `Ok(refusals)` names each blocked statement; `Err` means the input could not be
    /// judged (it does not parse) — the caller refuses dispatch on either non-clean
    /// answer, so the gate fails closed.
    pub async fn policy_verdicts(&self, sql: String) -> Result<Vec<PolicyRefusal>, String> {
        let ctx = self.ctx.clone();
        self.rt()
            .spawn(async move { sql::policy_verdicts(&ctx, &sql) })
            .await
            .map_err(|e| format!("policy task failed: {e}"))?
    }

    /// The engine's runtime (always present while the engine lives — see the field).
    fn rt(&self) -> &Runtime {
        self.rt.as_ref().expect("engine runtime")
    }

    // --- run / read -------------------------------------------------------

    /// **The editor's Run** (ED-02): classify `sql`, then route it.
    ///
    /// One classification in front of dispatch, and it is the same one the squiggles came from
    /// ([`sql::classify_one`], `Capability::Editor`) — so a statement the editor did not
    /// underline is a statement Run is prepared to perform, and a refusal fails the run with
    /// the words the squiggle showed rather than a DataFusion error about a rule that is ours.
    ///
    /// - `Query` delegates to [`query`](Engine::query) **byte-for-byte**. It is the only arm
    ///   that touches the snapshot lifecycle, which is what keeps "DDL does not retire
    ///   snapshots" true by construction rather than by care.
    /// - `Intercept(kind)` goes to `ddl::execute`, bracketed by
    ///   [`bookkeep`](Engine::bookkeep) so `cancel` / `is_running` / the close-while-running
    ///   confirm see it like any other work — a CTAS is a full scan, and a window closing over
    ///   one has to ask.
    /// - `Refuse(b)` never reaches DataFusion at all: classification is in front of `ctx.sql`
    ///   precisely because DDL executes *eagerly* inside it (spec §2), so anything that must
    ///   not run cannot be allowed to plan.
    ///
    /// The `SQLOptions` triple the read path carries (`query::materialize`) stays all-false and
    /// becomes defense in depth behind this: it is no longer the gate, and it never had the
    /// vocabulary to be one — it can refuse a class of plan, not name the surface that owns the
    /// capability.
    pub async fn run(
        &self,
        ws: WsId,
        tag: RunTag,
        sql: String,
        page_size: usize,
    ) -> Result<RunOutcome, String> {
        // On the engine runtime, like `policy_verdicts`: parsing is the caller's whole answer
        // here, and the caller is a UI task on the render thread.
        let (stmt, verdict) = {
            let ctx = self.ctx.clone();
            let text = sql.clone();
            self.rt()
                .spawn(async move { sql::classify_one(&ctx, &text) })
                .await
                .map_err(|e| format!("policy task failed: {e}"))??
        };
        match verdict {
            Verdict::Query => self
                .query(ws, tag, sql, page_size)
                .await
                .map(|(output, batch)| RunOutcome::Rows(output, batch)),
            Verdict::Intercept(kind) => {
                let ctx = self.ctx.clone();
                let root = self.data_root.lock().unwrap().clone();
                let internal = self.internal.clone();
                let report = self
                    .bookkeep(ws, tag, "statement", async move {
                        ddl::execute(&ctx, kind, stmt, sql, root, internal).await
                    })
                    .await?;
                self.settle_effect(report.effect.as_ref());
                Ok(RunOutcome::Statement(report))
            }
            Verdict::Refuse(blocked) => Err(blocked.editor_message()),
        }
    }

    /// What the **engine** has to learn from a statement's [`StoreEffect`], applied wherever one
    /// is produced — [`run`](Engine::run)'s interception and [`drop_table`](Engine::drop_table)'s
    /// direct call both.
    ///
    /// The engine learns from the returned value, exactly as the store does: an arm that
    /// registered a table says so in its effect, and that is where the origin comes from — never
    /// by asking DataFusion, which does not know. Held once rather than at each producer, so the
    /// catalog-surface drop and the typed one cannot leave the engine in two different states.
    /// Exhaustive on [`StoreEffect`] with no wildcard, for the reason [`ddl::execute`] is
    /// exhaustive on `StmtKind`: an effect a later task adds must be a compile error here rather
    /// than something the engine silently declines to learn from.
    fn settle_effect(&self, effect: Option<&StoreEffect>) {
        let Some(effect) = effect else { return };
        match effect {
            StoreEffect::TableUpserted { def, .. } => {
                self.note_origin(&def.name, def.origin.is_internal())
            }
            // A dropped table is no longer a write target, and a profile still scanning it is
            // now measuring files that may already be gone — cancelled here rather than inside
            // the drop for the reason [`InternalTables`] gives: the drop runs in a task that
            // cannot reach the lifecycle without holding the engine open.
            StoreEffect::TableRemoved { name, .. } => {
                self.cancel_profile(name);
                self.note_origin(name, false);
            }
            // Nothing for the engine to learn. A view's own lifecycle is `create_view` /
            // `drop_view`, which already cancel its scan; an `INSERT` moves data under a
            // registration that is unchanged; and the function catalog is not a table.
            StoreEffect::ViewUpserted { .. }
            | StoreEffect::ViewRemoved { .. }
            | StoreEffect::RescanTable { .. }
            | StoreEffect::FunctionsChanged => {}
        }
    }

    /// Drop the registered table `name` — **the one funnel both surfaces drop through** (ED-05).
    ///
    /// A typed `DROP TABLE` reaches the same body through [`run`](Engine::run)'s interception;
    /// the catalog pane's confirm reaches it here, after it has taken the def out of the store
    /// and written `project.json` (the store-first order `save_view` established — a drop the
    /// project file never heard about comes back on the next open). Two gestures, one
    /// implementation, because the difference between them is a *question asked of the user*,
    /// not a difference in what the drop does: an internal table's data directory goes with it
    /// on both paths, which is the whole reason this is not two calls.
    ///
    /// `if_exists` is the statement's clause. The pane passes `true`: the row it is dropping came
    /// out of the store, and a def whose registration failed has no provider to deregister.
    pub async fn drop_table(
        &self,
        name: String,
        if_exists: bool,
    ) -> Result<StatementOutcome, String> {
        // **Background work, so the close confirm asks about it.** An internal table's data is
        // one file per `INSERT` with no compaction, so a heavily written table is a directory of
        // thousands of files and deleting it is not instant — long enough that a window closing
        // over it would take the runtime away mid-delete. The user gets the same question a
        // running export gets, and can let it finish.
        let _deleting = BackgroundGuard::new(self);
        let ctx = self.ctx.clone();
        let root = self.data_root.lock().unwrap().clone();
        let internal = self.internal.clone();
        let outcome = self
            .rt()
            .spawn(async move { ddl::drop_table(&ctx, &root, &internal, &name, if_exists).await })
            .await
            .map_err(|e| format!("drop table task failed: {e}"))??;
        self.settle_effect(outcome.effect.as_ref());
        Ok(outcome)
    }

    /// Bracket `work` as workspace `ws`'s in-flight call — the lifecycle every dispatch that
    /// materializes **nothing** shares: [`explain`](Engine::explain), and every intercepted
    /// statement.
    ///
    /// Supersedes whatever `ws` was running (a tab runs one thing at a time, exactly as a
    /// re-press does), registers the abort handle so [`cancel`](Engine::cancel),
    /// [`is_running`](Engine::is_running) and the close-while-running flag can all see it, and
    /// removes the entry on the way out **iff** this is still the latest dispatch — by
    /// `dispatch`, never by `tag`, for the reason [`InFlight::dispatch`] exists.
    ///
    /// `Lifecycle::current` is deliberately untouched: nothing routed through here spools a
    /// snapshot, so there is none to settle and none to retire.
    ///
    /// `what` names the call in the one message a *runtime* failure can produce — a task that
    /// panicked or was dropped by the runtime, which is a different fault from the call's own
    /// `Err` and reads as one.
    async fn bookkeep<F, T>(&self, ws: WsId, tag: RunTag, what: &str, work: F) -> Result<T, String>
    where
        F: Future<Output = Result<T, String>> + Send + 'static,
        T: Send + 'static,
    {
        let dispatch = self.dispatch_seq.fetch_add(1, Ordering::Relaxed);
        let task = {
            let mut lc = self.lifecycle.lock().unwrap();
            if let Some(prev) = lc.inflight.remove(&ws) {
                self.abort_inflight(prev);
            }
            let task = self.rt().spawn(work);
            lc.inflight.insert(
                ws,
                InFlight {
                    dispatch,
                    tag,
                    snapshot: None,
                    abort: task.abort_handle(),
                    start: Instant::now(),
                },
            );
            self.publish_inflight(&lc);
            task
        };

        // Armed for the await, disarmed the moment it returns — see [`DispatchGuard`]. An
        // agent reaches this through `mode: "explain"`, and a dropped MCP request future is
        // exactly the caller that goes away mid-await.
        let mut guard = DispatchGuard::arm(self, ws, dispatch);
        let joined = task.await;
        guard.disarm();

        let mut lc = self.lifecycle.lock().unwrap();
        if lc.inflight.get(&ws).map(|f| f.dispatch) == Some(dispatch) {
            lc.inflight.remove(&ws);
        }
        self.publish_inflight(&lc);
        match joined {
            Ok(res) => res,
            // The shared vocabulary, never the prose: a stopped call must not read as a fault.
            Err(join) if join.is_cancelled() => Err(CANCELLED.into()),
            Err(join) => Err(format!("{what} task failed: {join}")),
        }
    }

    /// Run `sql` **once** for workspace `ws`: materialize a fresh immutable snapshot
    /// and return its handle + page 1 (`docs/SNAPSHOT_SPEC.md` §3). Dispatch retires
    /// the workspace's previous snapshot and aborts its in-flight run (§4); `tag` is
    /// the caller's nonce for [`cancel`](Engine::cancel).
    ///
    /// Supersede checks key on the engine's own dispatch id, never on `tag`: the UI may
    /// dispatch the same tag twice for one logical run, and comparing tags would let the
    /// first call's settle path adopt the second call's `InFlight` entry — dismantling a
    /// perfectly good run and failing *both* calls (see [`InFlight::dispatch`]).
    pub async fn query(
        &self,
        ws: WsId,
        tag: RunTag,
        sql: String,
        page_size: usize,
    ) -> Result<(QueryOutput, RecordBatch), String> {
        let snapshot = SnapshotId(self.snap_seq.fetch_add(1, Ordering::Relaxed));
        let dispatch = self.dispatch_seq.fetch_add(1, Ordering::Relaxed);
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
                // Deferred rather than immediate if something is holding it open — an export
                // window opened on that result still owes the user those rows.
                self.retire_or_defer(&mut lc, old);
            }
            let ctx = self.ctx.clone();
            let engine_id = self.engine_id;
            let task = self.rt().spawn(async move {
                run_and_snapshot(&ctx, engine_id, snapshot, &sql, page_size, &fmt).await
            });
            lc.inflight.insert(
                ws,
                InFlight {
                    dispatch,
                    tag,
                    snapshot: Some(snapshot),
                    abort: task.abort_handle(),
                    start: Instant::now(),
                },
            );
            self.publish_inflight(&lc);
            task
        };

        // Armed for the await, disarmed the moment it returns — see [`DispatchGuard`].
        let mut guard = DispatchGuard::arm(self, ws, dispatch);
        let joined = task.await;
        guard.disarm();

        let mut lc = self.lifecycle.lock().unwrap();
        // Only the still-latest dispatch may settle workspace state; a newer one has
        // already retired everything this one owned — and owns the `InFlight` entry now,
        // so a superseded call must not remove it.
        let latest = lc.inflight.get(&ws).map(|f| f.dispatch) == Some(dispatch);
        if latest {
            lc.inflight.remove(&ws);
        }
        self.publish_inflight(&lc);
        match joined {
            Ok(Ok((output, batch, stats))) => {
                if latest {
                    if let Some(snap) = output.snapshot {
                        lc.current.insert(ws, snap);
                        lc.stats.insert(snap, stats);
                    }
                    Ok((output, batch))
                } else {
                    // Finished after being superseded — its snapshot must not leak.
                    retire_snapshot(&self.ctx, self.engine_id, snapshot);
                    Err(SUPERSEDED_RUN.into())
                }
            }
            // `run_and_snapshot` cleaned its own partial on failure.
            Ok(Err(e)) => Err(e),
            Err(join) if join.is_cancelled() => {
                // Aborted. The aborter retired the partial too, but `abort()` only lands
                // at the task's next await — so the task may have gone on to finish
                // `register_arrow` *after* that retire, leaving a table registered over
                // a deleted file. Awaiting the handle is what makes this definitive: the
                // task is finished by the time we see `is_cancelled`, so retiring again
                // (idempotent, best-effort) sweeps whatever it managed to create.
                retire_snapshot(&self.ctx, self.engine_id, snapshot);
                Err(CANCELLED.into())
            }
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
        // The snapshot's ordinal column (`docs/SNAPSHOT_SPEC.md` §9), from the same register
        // `snapshot_live` reads — present for exactly the snapshots that are alive to read.
        let ord = self.ordinal(snapshot);
        self.rt()
            .spawn(async move {
                query::fetch_page(&ctx, snapshot, page, page_size, sort, ord, &fmt).await
            })
            .await
            .map_err(|e| format!("page task failed: {e}"))?
    }

    /// The name of `snapshot`'s ordinal column, if the write pass recorded one — `None` for
    /// a retired snapshot (whose read fails anyway) and for a snapshot spooled without an
    /// ordinal (an `EXPLAIN` result, or one with duplicate column names — see
    /// `query::materialize`), which reads unordered exactly as it did before ordinals.
    fn ordinal(&self, snapshot: SnapshotId) -> Option<String> {
        self.lifecycle
            .lock()
            .unwrap()
            .stats
            .get(&snapshot)
            .and_then(|s| s.ord.clone())
    }

    /// Read one immutable snapshot as a chart (Rz2, `docs/CHART_SPEC.md` §5) — the
    /// renderer-first read `q` asks for: a projected, ordinal-ordered, capped read plus a
    /// long→wide pivot (`Rows`), raw points (`Raw`), or the one computed mark
    /// (`Histogram`). No aggregation, no bucketing, no imposed order — the withdrawn
    /// pipeline's grouped reads must not come back here (AGENTS.md §2).
    ///
    /// Snapshot-scoped and side-effect free like [`fetch_page`](Engine::fetch_page). Cache
    /// identity is `(snapshot, q)` **plus the engine's display config**: axis labels render
    /// through the live `datafusion.format.*` overrides, which `set_config` changes without
    /// a restart — so a UI cache keyed on `(snapshot, q)` alone serves stale labels after a
    /// Settings change, and the chart surface must re-render (not merely re-key) when those
    /// overrides move, exactly as the grid's pages do. Deliberately no lifecycle
    /// bookkeeping and no confirm in front of it — a projected, capped read of a local
    /// snapshot is `fetch_page`-tier work, not [`profile`](Engine::profile)'s tier.
    ///
    /// The chart never re-reads the source files: it charts the result the grid is paging,
    /// which is what makes the two agree when the data underneath has since moved.
    pub async fn chart(
        self: &Arc<Self>,
        snapshot: SnapshotId,
        q: ChartQuery,
    ) -> Result<ChartData, String> {
        // A histogram is **two** reads — a range pass, then the binning one — so the call
        // holds the snapshot open across them. Without the pin a re-run in the owning tab
        // between the passes deregisters the table mid-call, and a histogram would answer
        // with the first pass's real edges and the second pass's zero counts: a chart of
        // nothing, indistinguishable from a genuine empty range. Same rule as `export`'s
        // in-call pin (AGENTS.md §2) — and the same limit: the pin lives in this future,
        // so it holds only while the caller keeps awaiting. A dropped caller drops the pin
        // while the spawned read runs on detached; the read may then fail against a retired
        // table, but its answer has no listener, so nothing wrong is ever delivered.
        let _reading = self.pin_snapshot(snapshot);
        let ctx = self.ctx.clone();
        let fmt = CellFormat::new(&self.overrides.lock().unwrap());
        let ord = self.ordinal(snapshot);
        self.rt()
            .spawn(async move { chart::run_chart(&ctx, snapshot, &q, &fmt, ord.as_deref()).await })
            .await
            .map_err(|e| format!("chart task failed: {e}"))?
    }

    /// Does `snapshot` still exist to be read?
    ///
    /// The one honest way to tell "your result was replaced" from a real read failure. A
    /// retired snapshot's table is deregistered, so [`fetch_page`](Engine::fetch_page)
    /// answers with DataFusion's own "table not found" prose — and matching that prose at a
    /// call site is exactly the copy-of-a-rule this crate keeps refusing to hand out
    /// (`stopped_on_purpose` is the same lesson). A reader that outlived its snapshot asks
    /// **after** its read fails, so the answer cannot race the dispatch that retired it.
    ///
    /// [`Lifecycle::stats`] is the register consulted because it has exactly a snapshot's
    /// lifetime by construction — inserted when the write pass settles, removed by
    /// [`retire_now`](Engine::retire_now), which every retire of a handed-out snapshot goes
    /// through. A snapshot whose retire is **deferred** behind a pin is still there to read,
    /// and still answers `true`, which is the same fact from the reader's side.
    pub fn snapshot_live(&self, snapshot: SnapshotId) -> bool {
        self.lifecycle.lock().unwrap().stats.contains_key(&snapshot)
    }

    /// Run an `EXPLAIN [ANALYZE]` statement for `ws` — a parsed plan tree, no snapshot.
    /// Supersedes the workspace's in-flight run (mutually exclusive, like a re-run) but
    /// leaves its settled snapshot alone (spec §4: explains materialize nothing).
    pub async fn explain(&self, ws: WsId, tag: RunTag, sql: String) -> Result<QueryPlan, String> {
        let ctx = self.ctx.clone();
        self.bookkeep(ws, tag, "explain", async move {
            explain::run_explain(&ctx, &sql).await
        })
        .await
    }

    /// Cancel `ws`'s in-flight run/explain **iff** it is still run `tag` (S14 — a stale
    /// cancel can't abort a just-started newer run). Returns the elapsed time when
    /// something was actually cancelled; the awaiting `query`/`explain` settles
    /// `Err("cancelled")`.
    ///
    /// The `tag` — the UI's per-press nonce — is exactly right here, and the one place it
    /// is: the caller is asking to stop *the run it can see*, so if a repeat dispatch
    /// replaced the in-flight entry under the same tag, stopping that one is what the
    /// press meant.
    pub fn cancel(&self, ws: WsId, tag: RunTag) -> Option<u128> {
        let mut lc = self.lifecycle.lock().unwrap();
        if lc.inflight.get(&ws).map(|f| f.tag) == Some(tag) {
            let f = lc.inflight.remove(&ws).unwrap();
            let elapsed = f.start.elapsed().as_millis();
            self.abort_inflight(f);
            self.publish_inflight(&lc);
            Some(elapsed)
        } else {
            None
        }
    }

    // --- profile ----------------------------------------------------------

    /// Profile the catalog entry `name` — **one full scan, one aggregate, every column at
    /// once** (D4, see [`profile`]). Works for a table or a view: a view has no footer at all,
    /// so a scan is the only way it learns anything beyond a column's type.
    ///
    /// Deliberately expensive and deliberately opt-in: distinct counts can't be merged across
    /// files, so there is no cheaper form. The UI confirms before a first scan (P3-10) and
    /// caches the result until the entry changes.
    ///
    /// Superseded-by-dispatch like [`query`](Engine::query): a re-scan aborts the scan it
    /// replaces, and the older call settles `Err("superseded by a newer scan")` rather than
    /// tearing down the entry the newer one now owns. Dedup is the *caller's* (freya-query
    /// keys the cache by the request), which is why two arrivals here mean two real requests.
    pub async fn profile(&self, name: String) -> Result<profile::CatalogProfile, String> {
        let key = fold_ident(&name);
        let dispatch = self.dispatch_seq.fetch_add(1, Ordering::Relaxed);
        let task = {
            let mut lc = self.lifecycle.lock().unwrap();
            if let Some(prev) = lc.profiles.remove(&key) {
                prev.abort.abort();
            }
            let ctx = self.ctx.clone();
            let scanned = name.clone();
            let task = self
                .rt()
                .spawn(async move { catalog::run_profile(&ctx, &scanned).await });
            lc.profiles.insert(
                key.clone(),
                ProfileRun {
                    dispatch,
                    abort: task.abort_handle(),
                },
            );
            self.publish_inflight(&lc);
            task
        };

        let joined = task.await;

        let mut lc = self.lifecycle.lock().unwrap();
        let latest = lc.profiles.get(&key).map(|p| p.dispatch) == Some(dispatch);
        if latest {
            lc.profiles.remove(&key);
        }
        self.publish_inflight(&lc);
        match joined {
            Ok(res) if latest => res,
            // Its numbers describe a scan the caller has already replaced.
            Ok(_) => Err(SUPERSEDED_SCAN.into()),
            Err(join) if join.is_cancelled() => Err(CANCELLED.into()),
            Err(join) => Err(format!("profile task failed: {join}")),
        }
    }

    /// Abort the profile scan of `name`, if one is running — `true` when something was
    /// actually cancelled. The awaiting [`profile`](Engine::profile) settles `Err("cancelled")`.
    ///
    /// Unguarded by any nonce, unlike [`cancel`](Engine::cancel): a scan is keyed by the entry,
    /// and every caller — the inspector's Cancel, and every catalog mutation that is about to
    /// make the result a lie — means "stop scanning *this entry*".
    pub fn cancel_profile(&self, name: &str) -> bool {
        let mut lc = self.lifecycle.lock().unwrap();
        let cancelled = match lc.profiles.remove(&fold_ident(name)) {
            Some(run) => {
                run.abort.abort();
                true
            }
            None => false,
        };
        self.publish_inflight(&lc);
        cancelled
    }

    // --- snapshot pins ----------------------------------------------------

    /// Hold `snapshot` open for as long as the returned [`SnapshotPin`] lives: while a pin is
    /// out, retiring the snapshot is **deferred**, not skipped.
    ///
    /// A snapshot is owned by its workspace and retired the moment that workspace dispatches
    /// another run (`docs/SNAPSHOT_SPEC.md` §4) — which is right for the grid, whose pages
    /// follow the tab, and wrong for anything that outlives one press. The export window is
    /// the first such reader: it is opened *on a result*, may sit there while the user goes
    /// back and re-runs the query, and must still write the rows that were on screen when it
    /// was opened. Without a pin a re-run deregisters the table mid-`COPY` and truncates the
    /// file, or — the quieter failure — makes a later Export report no results at all when
    /// there are plainly some on screen.
    ///
    /// **RAII rather than a pin/unpin pair**, deliberately: this is the same rule freya-query
    /// cache entries live by (lifetime is a held handle, never imperative bookkeeping), so a
    /// caller expresses "I still need this" by keeping the guard, and an early return, a
    /// panic or a dropped window all release it.
    pub fn pin_snapshot(self: &Arc<Self>, snapshot: SnapshotId) -> SnapshotPin {
        let mut lc = self.lifecycle.lock().unwrap();
        *lc.pins.entry(snapshot).or_insert(0) += 1;
        drop(lc);
        SnapshotPin {
            engine: Arc::clone(self),
            snapshot,
        }
    }

    /// Take one hold without the `Arc` a [`SnapshotPin`] needs — for [`ExportGuard`], which
    /// borrows the engine for the length of one call. Always released by that guard's `Drop`.
    fn acquire_pin(&self, snapshot: SnapshotId) {
        let mut lc = self.lifecycle.lock().unwrap();
        *lc.pins.entry(snapshot).or_insert(0) += 1;
    }

    /// Drop one hold, retiring the snapshot for real if this was the last one and a retire
    /// arrived while it was pinned.
    fn release_pin(&self, snapshot: SnapshotId) {
        let mut lc = self.lifecycle.lock().unwrap();
        match lc.pins.get_mut(&snapshot) {
            Some(holds) if *holds > 1 => *holds -= 1,
            Some(_) => {
                lc.pins.remove(&snapshot);
                if lc.deferred.remove(&snapshot) {
                    self.retire_now(&mut lc, snapshot);
                }
            }
            // Unbalanced release — a bug in a caller, and the kind that would otherwise show
            // up much later as a snapshot that never goes away.
            None => tracing::error!(
                "engine {}: released a pin on snapshot {snapshot} that was never held",
                self.engine_id
            ),
        }
    }

    /// Retire `snapshot` unless someone is holding it open, in which case remember to retire
    /// it when the last hold releases.
    ///
    /// Every retire of a snapshot a caller **has been handed** goes through here. The three
    /// inside [`query`](Engine::query)'s settle path deliberately do not: they retire a run's
    /// own partial or a superseded run's output, neither of which is ever returned, so nothing
    /// can be holding one.
    fn retire_or_defer(&self, lc: &mut Lifecycle, snapshot: SnapshotId) {
        if lc.pins.contains_key(&snapshot) {
            lc.deferred.insert(snapshot);
        } else {
            self.retire_now(lc, snapshot);
        }
    }

    /// Retire a snapshot and forget what its write pass recorded. Every path that actually
    /// deletes a live snapshot goes through here, so `Lifecycle::stats` cannot outlive the
    /// snapshots it describes.
    fn retire_now(&self, lc: &mut Lifecycle, snapshot: SnapshotId) {
        lc.stats.remove(&snapshot);
        retire_snapshot(&self.ctx, self.engine_id, snapshot);
    }

    // --- export -----------------------------------------------------------

    /// Write `snapshot` to disk per `spec` (D6) — one file, or a Hive directory when the
    /// spec carries partition columns. Returns `(path, rows_written)`.
    ///
    /// **The snapshot is the source, not the SQL.** An export never re-runs the query: it
    /// streams the very table the grid is paging, in the sort the grid is showing, so the
    /// file matches what was on screen even if the underlying data has since moved. That is
    /// the whole reason snapshots exist (`docs/SNAPSHOT_SPEC.md`), and it is why this takes a
    /// [`SnapshotId`] rather than a workspace: an export belongs to a *result*, not to a tab,
    /// and the result outlives the tab's current text.
    ///
    /// Unlike [`query`](Engine::query) there is no dispatch nonce and no supersede: two
    /// exports are two files, and neither invalidates the other. The bookkeeping is the
    /// in-flight count, which keeps the close confirm honest while a file is half-written,
    /// and a [`pin`](Engine::pin_snapshot) for the duration — so a re-run in the owning tab
    /// can't deregister the table this `COPY` is streaming. The export window holds a pin of
    /// its own for its whole life; this one makes the call correct on its own terms, for a
    /// caller that has no window.
    pub async fn export(
        &self,
        snapshot: SnapshotId,
        spec: export::ExportSpec,
    ) -> Result<(String, usize), String> {
        // **A guard, not a bracketed pair.** This method awaits, and the caller is a UI task
        // that is dropped when its scope goes — so closing the export window mid-write drops
        // this future at the await below. Decrementing after the await would then never run:
        // the in-flight flag would stay true for the engine's whole life (every later window
        // close would claim work was running) and the pin would never release, leaving a
        // snapshot that survives every re-run for the rest of the session.
        let _writing = ExportGuard::new(self, snapshot);
        // Copied out under the lock: the partitioned-export gate reads what this snapshot's write
        // pass counted, and the spawned task must not hold the lifecycle lock across an await.
        // A snapshot with no recorded stats is one nothing counted, which the gate treats as
        // "cannot vouch for" rather than as zero nulls.
        let stats = self
            .lifecycle
            .lock()
            .unwrap()
            .stats
            .get(&snapshot)
            .cloned()
            .unwrap_or_default();
        let task = {
            let ctx = self.ctx.clone();
            self.rt()
                .spawn(async move { export::run_export(&ctx, snapshot, spec, &stats).await })
        };

        // Dropping this future does *not* stop the write: the spawned task detaches and the
        // file still completes, which is the kinder outcome for a user who closed the window
        // after committing to an export. What it does stop is anyone hearing how it ended.
        let joined = task.await;

        match joined {
            Ok(res) => res,
            // The shared vocabulary, not the prose: a stopped call must never be presented as a
            // fault, and every surface asks [`stopped_on_purpose`] rather than matching a string.
            Err(join) if join.is_cancelled() => Err(CANCELLED.into()),
            Err(join) => Err(format!("export task failed: {join}")),
        }
    }

    // --- catalog ----------------------------------------------------------

    /// Register the object store one [`ConnectionDef`] describes, so tables can be
    /// registered over its bucket (W7).
    ///
    /// **Before any table that reads it.** DataFusion resolves no remote scheme on its own:
    /// without this, a source path under `s3://acme-lake` fails its registration with "No
    /// suitable object store found" no matter how well-formed the def is. That ordering is
    /// [`register_pass`](crate::register::register_pass)'s, so every replay of a project gets
    /// it.
    ///
    /// `Err` means nothing was registered, and carries what to fix — a missing region, a
    /// profile the credential chain does not answer for. See [`store::connect`].
    pub async fn connect(&self, conn: ConnectionDef) -> Result<(), String> {
        let ctx = self.ctx.clone();
        self.rt()
            .spawn(async move { store::connect(&ctx, &conn).await })
            .await
            .map_err(|e| format!("connect task failed: {e}"))?
    }

    /// Forget the object store a connection registered — the Forget gesture's engine half
    /// (W7), addressed by the same [`ConnectionDef::url`] [`connect`](Self::connect) put it in
    /// under.
    ///
    /// Synchronous, like [`deregister`](Self::deregister) and for the same reason: DataFusion
    /// just drops the entry from its registry, so there is no work to spawn and no answer to
    /// await. Nothing is reported — see [`store::disconnect`] for why neither of its no-ops is
    /// a fault.
    pub fn disconnect(&self, url: &str) {
        store::disconnect(&self.ctx, url);
    }

    /// The AWS profile names this machine's own configuration defines — what the connection
    /// editor's **Named profile** picker offers (W7 · 03). See [`store::aws_profiles`]; no
    /// profile's *contents* are read.
    ///
    /// On the engine rather than beside the surface that asks for it, for the two reasons every
    /// other method here is: `aws-config` is [`store`]'s dependency and stays there, and this
    /// reads files — so it belongs on the runtime that keeps a read off the thread drawing every
    /// window, not in a component that would have to invent one.
    pub async fn aws_profiles(&self) -> Vec<String> {
        self.rt()
            .spawn(store::aws_profiles())
            .await
            .unwrap_or_default()
    }

    /// (Re)register one external table from its spec, returning its inferred schema +
    /// free row count.
    ///
    /// Aborts the table's profile scan first: re-registration re-infers the schema from
    /// whatever is on disk *now*, so a scan in flight is computing numbers about files the
    /// register is replacing. Done here rather than left to the caller because it is engine
    /// truth — every path that re-registers gets it, including ones not yet written.
    pub async fn register(&self, spec: TableSpec) -> Result<TableMeta, String> {
        self.cancel_profile(&spec.name);
        let ctx = self.ctx.clone();
        let (name, internal) = (spec.name.clone(), spec.internal);
        let meta = self
            .rt()
            .spawn(async move { catalog::register_external(&ctx, &spec).await })
            .await
            .map_err(|e| format!("register task failed: {e}"))?;
        // On **both** arms. A def the engine refused has no provider at all — `register_external`
        // deregisters before it re-infers — so the honest answer to "may a write statement target
        // this" is no, whatever the def says. Recording only on success would leave a name that
        // *used* to be internal claiming it still is: a table re-registered as external and then
        // failing would keep answering `is_internal`, and ED-05's drop would take the origin from
        // an entry the pass had already disproved.
        self.note_origin(&name, internal && meta.is_ok());
        meta
    }

    /// What `name`'s row says **now** — its columns and free row count — read from the files
    /// without re-registering the table (ED-05).
    ///
    /// The answer an `INSERT` needs, and the reason it is not [`register`](Engine::register):
    /// re-registering deregisters the provider and builds a fresh one, and **that** is what
    /// leaves every view above it holding a stale `Arc` (D10/D11). Views survive it only
    /// because the caller then re-creates them. An append cannot make them stale — the sink
    /// schema-checks before it writes, so the shape a view captured is the shape that is still
    /// there — so re-registering after one would break the views and repair them again for
    /// nothing, and re-infer a schema that could not have moved on the way.
    ///
    /// The count is still *read*, never added up from what a statement claimed: this re-LISTs
    /// the sources and totals the footers, of which only the appended file's is uncached.
    pub async fn table_meta(&self, name: String) -> Result<TableMeta, String> {
        let ctx = self.ctx.clone();
        self.rt()
            .spawn(async move { catalog::table_meta(&ctx, &name).await })
            .await
            .map_err(|e| format!("table meta task failed: {e}"))?
    }

    /// The Hive partition keys under `paths`, outermost first — what the Configure window's
    /// Hive section offers (P4-11). Listed through the session's object store, so it answers for
    /// a bucket as readily as for a local folder.
    pub async fn detect_partitions(&self, paths: Vec<String>) -> Vec<String> {
        let ctx = self.ctx.clone();
        self.rt()
            .spawn(async move { catalog::detect_partitions(&ctx, &paths).await })
            .await
            .unwrap_or_default()
    }

    /// Drop a registered table.
    pub fn deregister(&self, table: &str) {
        self.cancel_profile(table);
        let _ = self.ctx.deregister_table(table);
        self.note_origin(table, false);
    }

    /// Create (or redefine) the SQL view `name` over `sql`, returning its columns and
    /// what it reads (D10). `CREATE OR REPLACE` — redefinition is the ⌘S-on-a-view path.
    ///
    /// `name` is whatever the user typed (it rides in `.strata/project.json`, a shared,
    /// committed file), so it goes through [`quote_ident`] rather than straight into the
    /// statement — which is the only reason a name like `Sales 2024` can be a view at all.
    /// The view's identity is then [`fold_ident(name)`](fold_ident), which is what the
    /// lookup below asks for.
    pub async fn create_view(&self, name: String, sql: String) -> Result<ViewMeta, String> {
        // Redefining the view changes what a scan of it would even mean — see `register`.
        self.cancel_profile(&name);
        let ctx = self.ctx.clone();
        self.rt()
            .spawn(async move {
                let stmt = format!(
                    "CREATE OR REPLACE VIEW {} AS {sql}",
                    quote_ident(name.as_str())
                );
                let df = ctx.sql(&stmt).await.map_err(|e| e.to_string())?;
                // The DDL only takes effect when its (empty) result is driven.
                let _ = df.collect().await;
                // The freshly-registered view's own `DataFrame` gives both the columns
                // and what it reads — the planner has already resolved it, so we never
                // parse the SQL ourselves.
                //
                // `bare(fold_ident(…))`, not the raw `&str`: `impl Into<TableReference>
                // for &str` parses, and a *quoted* name (`Sales 2024`, `say "hi"`) does
                // not survive a parse — it would be looked up under a name the DDL never
                // created. `fold_ident` is exactly what the DDL registered, and `bare`
                // then takes it verbatim instead of parsing it a second time.
                let t = ctx
                    .table(TableReference::bare(fold_ident(name.as_str())))
                    .await
                    .map_err(|e| e.to_string())?;
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

    /// Drop the SQL view `name` (idempotent — `IF EXISTS`). Quoted the same way
    /// [`create_view`](Engine::create_view) quoted it, so the drop names the same view.
    pub async fn drop_view(&self, name: String) -> Result<(), String> {
        self.cancel_profile(&name);
        let ctx = self.ctx.clone();
        self.rt()
            .spawn(async move {
                ctx.sql(&format!(
                    "DROP VIEW IF EXISTS {}",
                    quote_ident(name.as_str())
                ))
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
    ///
    /// Sync, so it can't await the aborted task the way `query` does; if the tab's
    /// `query` future is already gone, nothing does. What survives that is bounded — a
    /// `__snap_N` registered over a file we deleted (an uncached read of it fails
    /// cleanly, exactly like any retired snapshot), and at worst a stray parquet file in
    /// this engine's own directory, which `Drop` removes wholesale.
    pub fn cleanup_ws(&self, ws: WsId) {
        let mut lc = self.lifecycle.lock().unwrap();
        if let Some(f) = lc.inflight.remove(&ws) {
            self.abort_inflight(f);
        }
        if let Some(snap) = lc.current.remove(&ws) {
            self.retire_or_defer(&mut lc, snap);
        }
        self.publish_inflight(&lc);
    }

    /// Abort an in-flight run and retire whatever snapshot it was materializing.
    ///
    /// Best effort by construction: `abort()` lands at the task's next await (cooperative
    /// cancel), so the task's own error-path cleanup never runs *and* the task can still
    /// be mid-flight while we retire. The definitive sweep is the awaiter's — `query`
    /// retires again once its `JoinHandle` reports cancelled, by which point the task is
    /// finished for certain. This retire is what covers the paths with no awaiter left
    /// (`cleanup_ws`, `Drop`).
    fn abort_inflight(&self, f: InFlight) {
        f.abort.abort();
        if let Some(snap) = f.snapshot {
            // No `stats` entry can exist for this one — it is only recorded on a settle the
            // caller was handed — but the retire itself still has to happen.
            retire_snapshot(&self.ctx, self.engine_id, snap);
        }
    }
}

/// One piece of **background** engine work in flight — an export writing a file, a drop
/// deleting a table's data. Holding one is what keeps the close-while-running flag true
/// (`Lifecycle::background`), so the window asks before it takes the runtime away.
///
/// A guard rather than a matching pair of statements because every holder **awaits**, and a
/// dropped future must not be able to leak the count: a leaked increment would make every later
/// window close claim work was running for the rest of the engine's life. Borrows the engine, so
/// it cannot outlive the call.
struct BackgroundGuard<'a> {
    engine: &'a Engine,
}

impl<'a> BackgroundGuard<'a> {
    /// Constructing the guard *is* the acquire, so there is no way to hold one without having
    /// taken what it releases.
    fn new(engine: &'a Engine) -> Self {
        let mut lc = engine.lifecycle.lock().unwrap();
        lc.background += 1;
        engine.publish_inflight(&lc);
        Self { engine }
    }
}

impl Drop for BackgroundGuard<'_> {
    fn drop(&mut self) {
        let mut lc = self.engine.lifecycle.lock().unwrap();
        lc.background = lc.background.saturating_sub(1);
        self.engine.publish_inflight(&lc);
    }
}

/// One in-flight export's bookkeeping — [`BackgroundGuard`]'s count, plus a hold on the snapshot
/// being written — released together by `Drop`.
///
/// The export half is the pin: a write must read the snapshot it was opened on even if the tab
/// behind it re-runs. [`SnapshotPin`] is the owned variant, for a holder that outlives one
/// method.
struct ExportGuard<'a> {
    snapshot: SnapshotId,
    /// The engine rides here rather than beside it — one copy of the reference, so a later edit
    /// cannot have the pin and the count answer to two of them. Dropped after this struct's own
    /// `Drop` body, which is the order the lock wants (see there).
    background: BackgroundGuard<'a>,
}

impl<'a> ExportGuard<'a> {
    /// Claim both halves. Constructing the guard *is* the acquire, so there is no way to hold
    /// one without having taken what it releases.
    fn new(engine: &'a Engine, snapshot: SnapshotId) -> Self {
        engine.acquire_pin(snapshot);
        Self {
            snapshot,
            background: BackgroundGuard::new(engine),
        }
    }
}

impl Drop for ExportGuard<'_> {
    fn drop(&mut self) {
        // `release_pin` takes the lifecycle lock itself, and this mutex is not reentrant — so it
        // runs here, before the field's own `Drop` takes the lock again.
        self.background.engine.release_pin(self.snapshot);
    }
}

/// A hold on one snapshot, keeping it readable past the re-run that would otherwise retire it
/// (see [`Engine::pin_snapshot`]). Dropping it releases the hold, and retires the snapshot if
/// a retire arrived while it was pinned and this was the last hold.
///
/// Holds an `Arc<Engine>` rather than a borrow so it can be parked in UI state for a window's
/// lifetime — which is the whole point of it existing.
pub struct SnapshotPin {
    engine: Arc<Engine>,
    snapshot: SnapshotId,
}

impl SnapshotPin {
    /// The snapshot this pin is holding open.
    pub fn snapshot(&self) -> SnapshotId {
        self.snapshot
    }
}

impl Drop for SnapshotPin {
    fn drop(&mut self) {
        self.engine.release_pin(self.snapshot);
    }
}

impl Drop for Engine {
    /// The window is closing: abort everything in flight and remove this engine's
    /// snapshot directory. (`purge_snapshot_root` at the *next* process start covers an
    /// abrupt exit that skips this.)
    ///
    /// Same asynchronous-abort caveat as `cleanup_ws`, with a smaller blast radius: an
    /// aborted task that outlives us can only recreate files under a directory nobody
    /// reads any more (its `SessionContext` dies with the engine), and the next startup
    /// purge sweeps it — the claim goes with `_snapshot_lock`, which drops right after
    /// this body, so that directory is no longer defended.
    fn drop(&mut self) {
        let mut lc = self.lifecycle.lock().unwrap();
        for (_, f) in lc.inflight.drain() {
            f.abort.abort();
        }
        for (_, p) in lc.profiles.drain() {
            p.abort.abort();
        }
        lc.current.clear();
        // The window is going: whoever still holds the flag must not be told we're busy.
        self.publish_inflight(&lc);
        drop(lc);
        // Context-safe shutdown: don't block on worker threads (a plain `Runtime` drop
        // panics inside another async context); aborted tasks are dropped in the background.
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
        discard_snapshot_dir(self.engine_id);
    }
}

/// The **engine identity** of a catalog name: the string DataFusion ends up keying the
/// object under, once [`quote_ident`] has rendered `name` into a statement.
///
/// It is not a re-derivation of DataFusion's rules — it *asks* `TableReference::parse_str`,
/// the very function `ctx.register_table(&str)` and `ctx.table(&str)` resolve a plain
/// `&str` through. So a view created via [`quote_ident`] and a table registered from the
/// same def name land on the same identity by construction: a single bare word folds to
/// ASCII-lowercase (`MyView` → `myview`, `Order` → `order`), and anything the parser can't
/// read as one identifier — a space, a hyphen, a leading digit, a stray quote — is the
/// name verbatim.
///
/// A dotted name parses as *qualified*, which we deliberately don't honour: the engine owns
/// one schema and a catalog name is an opaque label, so `a.b` is the literal name `a.b`.
/// (Nothing regresses: `register_table("a.b")` resolves to schema `a`, which doesn't exist,
/// so such a table never registered either.)
pub(crate) fn fold_ident(name: &str) -> String {
    match TableReference::parse_str(name) {
        TableReference::Bare { table } => table.to_string(),
        _ => name.to_string(),
    }
}

/// Render `name` for interpolation into a statement, such that DataFusion resolves it to
/// exactly [`fold_ident(name)`](fold_ident): bare when the folded name is a plain
/// lowercase word that isn't reserved in a name position, double-quoted (any embedded `"`
/// doubled) otherwise.
///
/// **Fold-preserving is the contract**, and it is what makes this safe to add to a shipped
/// app. DataFusion lower-cases an *unquoted* identifier and takes a quoted one verbatim
/// (`datafusion-sql`'s `normalize_ident`), so a view a user named `DailySales` has been
/// registering as `dailysales` all along — as have any sibling defs and saved queries that
/// say `FROM dailysales`. Emitting `"DailySales"` would re-key it and break every one of
/// them, with no migration in sight. So a name that already worked keeps its *exact* old
/// identity: we quote nothing that can be said bare, and we fold the case ourselves rather
/// than leaving it to the parser (which also makes the identity independent of
/// `datafusion.sql_parser.enable_ident_normalization`, a key W2 lets the user set).
///
/// Quoting is therefore never a re-keying, only ever a capability gain. It fires in two
/// cases, and they are not the same:
///
/// * **Names that were genuinely broken.** `Sales 2024`, `2024`, `sales-eu`, `say "hi"` each
///   turned an interpolated name into malformed SQL, and the caller has already persisted
///   the def by then, so the row stayed Failed forever. There is no prior identity to
///   preserve here, because nothing was ever registered
///   (`the_names_quoting_added_were_malformed_sql_before`).
/// * **Reserved words, defensively.** `order` was *not* broken: under the `GenericDialect`
///   DataFusion parses with, both `CREATE VIEW Order …` and a bare `FROM order` parse today,
///   and the view registered as `order`. Quoting it is insurance against a name position or
///   dialect where that stops being true — and it stays safe precisely because the fold runs
///   **first**, so `Order` → `"order"`, the same identity the unquoted spelling had and the
///   one `register_table` gives a table of that name. A generated `FROM` therefore resolves
///   for tables and views alike ([`profile::profile_sql`]).
///
/// Both cases are pinned by `quoting_keeps_the_identity_the_unquoted_interpolation_gave_a_name`,
/// which registers each name the old way and the new way and requires the two contexts to be
/// reachable under exactly the same spellings.
///
/// The reserved-word authority is [`sql::lex::is_reserved_in_name_position`] — the same one
/// the language service uses for completion's quoting.
pub(crate) fn quote_ident(name: &str) -> String {
    let id = fold_ident(name);
    let mut rest = id.chars();
    let bare = matches!(rest.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && rest.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !sql::lex::is_reserved_in_name_position(&id);
    if bare {
        id
    } else {
        format!("\"{}\"", id.replace('"', "\"\""))
    }
}

/// Build a `SessionContext` honouring the engine config `overrides`: the
/// `ConfigOptions` keys go on the `SessionConfig`; the `datafusion.runtime.*` keys
/// build a `RuntimeEnv` (parsed via `parse_capacity_limit`). Bad values are logged
/// and skipped rather than failing the whole engine.
fn build_context(overrides: &BTreeMap<String, String>) -> SessionContext {
    // `information_schema` on by default — Strata's default, not DataFusion's, which is why
    // it is set *before* the override loop rather than in it: a user who turns it off in
    // Settings still wins, and it is not an owned key. `SHOW TABLES` and every
    // `information_schema` view need it, and they only became safe to expose with the
    // snapshot filter in `providers` (`docs/STATEMENTS_SPEC.md` §5) — without that they would
    // list every `__snap_N` spool table and its `__strata_ord` column. `ENGINE_KEYS` carries
    // the same `true`, so a *removed* override lands back here rather than on DataFusion's.
    let mut config = SessionConfig::new().with_information_schema(true);
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
    let mut config = config.with_default_catalog_and_schema(CATALOG, SCHEMA);
    // Source spans on planner errors power the validator's squiggles (P2-18) — owned,
    // like the catalog names, so an override can't silently degrade diagnostics.
    config.options_mut().sql_parser.collect_spans = true;
    let mut ctx = match build_runtime(overrides) {
        Ok(rt) => SessionContext::new_with_config_rt(config, rt),
        Err(e) => {
            tracing::warn!("engine runtime config invalid ({e}); using defaults");
            // Not `new_with_config`: that would take DataFusion's whole runtime, list-files
            // cache included, and a bad *memory limit* must not quietly turn the file listings
            // stale as well. Fall back to our own defaults with no overrides applied.
            let rt = build_runtime(&BTreeMap::new()).expect("default runtime");
            SessionContext::new_with_config_rt(config, rt)
        }
    };
    // Our own catalog + schema, in place of the `MemoryCatalogProvider` the session builder
    // just registered under the same name, and **before** anything registers a table: identity
    // (one schema, folded names) and visibility (result snapshots resolve but do not
    // enumerate) — never lifecycle, which lives in `ddl` in front of `ctx.sql`. See
    // `providers` for why the traits cannot carry more than this.
    ctx.register_catalog(CATALOG, Arc::new(StrataCatalogProvider::default()));
    // The Postgres-style JSON accessors (`json_get`, `->`, `->>`) over a Utf8 column of JSON
    // text. They belong to the **engine**, not to a table: `json_get('{"a":1}', 'a')` is valid
    // with nothing registered, so this sits beside the catalog naming rather than in `catalog`.
    //
    // The crate also registers `?` as an alias for `json_contains`, and it is **unreachable from
    // SQL under our default dialect**: `GenericDialect` omits `Token::Question` from
    // `get_next_precedence`, so `doc ? 'a'` fails to parse before the operator is ever consulted.
    // `json_contains` is the spelling that works everywhere, and it stays the one we name: WJ-04
    // surveyed the move to `postgresql` and **declined** it — that dialect makes every operator
    // character a custom-operator part, so `a>-1` tokenizes as `a >- 1`, and it cannot parse
    // `SELECT * EXCEPT (a)`, `* EXCLUDE`, or a trailing comma in a projection. A user can still
    // set the key; `sql::lex` follows it, so the whole language service moves with them.
    //
    // Warned rather than fatal because the failure cannot be silent — a registration that did
    // not happen surfaces as "Invalid function 'json_get'" on the first query that needs one,
    // which names itself better than a panic during engine construction would.
    if let Err(e) = datafusion_functions_json::register_all(&mut ctx) {
        tracing::warn!("engine: JSON functions unavailable: {e}");
    }
    ctx
}

/// The catalog + schema **we own** — see [`build_context`].
const CATALOG: &str = "strata";
const SCHEMA: &str = "public";

/// Just the `datafusion.runtime.*` entries of `overrides` — the half that
/// [`build_runtime`] reads, and so the half a restart would change.
fn runtime_subset(overrides: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    overrides
        .iter()
        .filter(|(k, _)| config::is_restart_key(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// A `RuntimeEnv` from the `datafusion.runtime.*` overrides. Sizes ("2G", "100G") parse via
/// `parse_capacity_limit`; the TTL via [`crate::util::parse_duration`], the same function
/// [`config::value_error`] validates the field with.
///
/// **Always built, never DataFusion's own**, because one of these settings is ours: the
/// list-files cache starts *off* (`ENGINE_KEYS`, where the reason is written down). DataFusion
/// 54 turns it on by default with an infinite TTL, and a cached listing makes a re-scan return
/// the previous answer — which is the one thing a re-scan exists not to do. That default has to
/// be applied even when the user has set no `runtime.*` key at all, so there is no
/// "nothing to configure" short circuit here.
///
/// **Every key [`config::ENGINE_KEYS`] catalogues as `runtime.*` is read here.** Four of them
/// were catalogued — named, described, and treated as restart-required by
/// [`is_restart_key`](config::is_restart_key) — but never consumed, which nothing noticed while
/// there was no way to set them. P4-07's properties editor is that way: it offers the key,
/// validates the value, badges it RESTART and rebuilds the engine, and the setting then did
/// nothing, with no error to say so. A catalogue entry is a promise that the key applies, so
/// adding one to `ENGINE_KEYS` means adding it here in the same change.
fn build_runtime(overrides: &BTreeMap<String, String>) -> Result<Arc<RuntimeEnv>, String> {
    use datafusion::execution::runtime_env::RuntimeEnvBuilder;
    let val = |k: &str| {
        overrides
            .get(k)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let bytes = |key: &str, raw: &str| {
        SessionContext::parse_capacity_limit(key, raw).map_err(|e| e.to_string())
    };

    let mem = val("datafusion.runtime.memory_limit");
    let max_temp = val("datafusion.runtime.max_temp_directory_size");
    let temp_dir = val("datafusion.runtime.temp_directory");
    let metadata_cache = val("datafusion.runtime.metadata_cache_limit");
    let list_cache = val("datafusion.runtime.list_files_cache_limit");
    let list_ttl = val("datafusion.runtime.list_files_cache_ttl");

    // Ours before any override, so a key the user *removed* lands back here — the same shape
    // `information_schema` uses on the `SessionConfig` side.
    let mut b = RuntimeEnvBuilder::new().with_object_list_cache_limit(bytes(
        "datafusion.runtime.list_files_cache_limit",
        config::key_def("datafusion.runtime.list_files_cache_limit")
            .expect("the catalogued key")
            .default,
    )?);
    if let Some(v) = &mem {
        b = b.with_memory_limit(bytes("datafusion.runtime.memory_limit", v)?, 1.0);
    }
    if let Some(v) = &max_temp {
        b = b.with_max_temp_directory_size(
            bytes("datafusion.runtime.max_temp_directory_size", v)? as u64
        );
    }
    if let Some(v) = &temp_dir {
        b = b.with_temp_file_path(v);
    }
    if let Some(v) = &metadata_cache {
        b = b.with_metadata_cache_limit(bytes("datafusion.runtime.metadata_cache_limit", v)?);
    }
    if let Some(v) = &list_cache {
        b = b.with_object_list_cache_limit(bytes("datafusion.runtime.list_files_cache_limit", v)?);
    }
    if let Some(v) = &list_ttl {
        let ttl = crate::util::parse_duration(v).ok_or_else(|| {
            format!("datafusion.runtime.list_files_cache_ttl: expected a duration like 30s or 2m, got {v}")
        })?;
        b = b.with_object_list_cache_ttl(Some(ttl));
    }
    b.build_arc().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::task::{Context, Waker};

    use crate::engine::sql::Blocked;
    use strata_model::{SourceFormat, StatKey};

    use super::*;

    /// Big enough that the first dispatch is still streaming when the second lands, and
    /// cheap to abort (the spool awaits per batch, so the abort takes effect at once).
    const SLOW: &str = "SELECT count(*) FROM generate_series(1, 50000000)";
    const FAST: &str = "SELECT 1 AS n";
    /// A view body whose **profile** is still counting when a test acts on it: 50M distinct
    /// values, aborted within a few dozen milliseconds, so the scan never accumulates far.
    const SLOW_ROWS: &str = "SELECT * FROM generate_series(1, 50000000)";

    /// Drive `fut` to its first await point and hand it back still owned, so a test can drop
    /// it there. One poll is all it takes: a dispatch's bookkeeping is synchronous and runs
    /// before the `await` on the spawned work, so after this the entry is published and the
    /// future is parked exactly where a cancelled caller would abandon it.
    fn dispatched<F: Future>(fut: F) -> Pin<Box<F>> {
        let mut fut = Box::pin(fut);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            fut.as_mut().poll(&mut cx).is_pending(),
            "the run settled before it could be interrupted"
        );
        fut
    }

    /// **A caller that goes away mid-run leaves the engine idle.**
    ///
    /// Every caller used to be freya-query, which never cancels an execution, so `query`
    /// could publish its in-flight entry and rely on its own settle path to clear it. An
    /// agent's run (AA-03b) is awaited inside an MCP request future instead, and a client
    /// cancellation, a dropped connection or the agent server shutting down all drop it
    /// mid-await. Without `DispatchGuard` the entry survives forever: the window's in-flight
    /// flag latches on, so every later close, re-root and engine restart raises the
    /// close-while-running confirm for a query that finished long ago.
    #[test]
    fn a_dropped_run_future_does_not_leave_the_workspace_running() {
        let engine = Engine::new(BTreeMap::new());
        let ws = WsId(1);
        let flag = Arc::new(AtomicBool::new(false));
        engine.watch_inflight(Arc::clone(&flag));

        let running = dispatched(engine.query(ws, RunTag(1), SLOW.into(), 10));
        assert!(engine.is_running(ws), "the dispatch published");
        assert!(
            flag.load(Ordering::Relaxed),
            "and the window sees work in flight"
        );

        drop(running);

        assert!(
            !engine.is_running(ws),
            "dropping the caller must retract the dispatch, not strand it"
        );
        assert!(
            !flag.load(Ordering::Relaxed),
            "and the close-while-running flag must go back down"
        );
    }

    /// The guard must not tear down an entry a **newer** dispatch owns — the same `latest`
    /// rule the settle path follows, for the same reason.
    #[test]
    fn a_dropped_superseded_run_leaves_the_newer_dispatch_alone() {
        let engine = Engine::new(BTreeMap::new());
        let ws = WsId(1);

        let first = dispatched(engine.query(ws, RunTag(1), SLOW.into(), 10));
        // A second dispatch supersedes it and now owns the workspace's entry.
        let _second = dispatched(engine.query(ws, RunTag(2), SLOW.into(), 10));

        drop(first);

        assert!(
            engine.is_running(ws),
            "the superseded caller going away must not cancel the run that replaced it"
        );
    }

    fn overrides(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// What the live session actually holds for `key` — the thing `set_config` claims to move.
    fn live(engine: &Engine, key: &str) -> String {
        engine
            .ctx
            .state()
            .config()
            .options()
            .entries()
            .into_iter()
            .find(|entry| entry.key == key)
            .and_then(|entry| entry.value)
            .unwrap_or_default()
    }

    const BATCH: &str = "datafusion.execution.batch_size";
    const MEMORY: &str = "datafusion.runtime.memory_limit";

    #[test]
    fn set_config_moves_a_live_option_and_puts_a_removed_one_back_to_its_default() {
        let engine = Engine::new(overrides(&[(BATCH, "4096")]));
        assert_eq!(live(&engine, BATCH), "4096", "built with the override");

        assert!(!engine.set_config(overrides(&[(BATCH, "1024")])));
        assert_eq!(live(&engine, BATCH), "1024", "applied without a restart");

        // The half that is easy to get wrong: dropping a key must not leave the engine on the
        // value that was just deleted.
        assert!(!engine.set_config(BTreeMap::new()));
        assert_eq!(
            live(&engine, BATCH),
            config::key_def(BATCH).expect("catalogued").default,
            "a removed override goes back to the built-in default"
        );
    }

    #[test]
    fn a_runtime_key_owes_a_restart_until_the_engine_is_rebuilt() {
        let engine = Engine::new(BTreeMap::new());
        assert!(!engine.restart_owed());

        assert!(
            engine.set_config(overrides(&[(MEMORY, "2G")])),
            "the RuntimeEnv is fixed at build, so this is owed"
        );
        // Declining the restart must not settle the debt: the map has moved on, the runtime has
        // not, and the next config write has to offer it again.
        assert!(engine.restart_owed());
        assert!(
            engine.set_config(overrides(&[(MEMORY, "2G"), (BATCH, "1024")])),
            "a second write still owes the same restart"
        );

        // Rebuilding is what settles it — which is exactly what the window's remount does.
        let restarted = Engine::new(engine.overrides());
        assert!(!restarted.restart_owed());
    }

    /// The JSON accessors reach the **function catalogue**, not merely the context — and those
    /// are the same fact, which is the whole reason registering them is the entire integration.
    /// `functions::snapshot` walks the live registry, so a UDF registered in `build_context`
    /// arrives in autocomplete, signature help and the docs panel with no per-function table to
    /// maintain and no way for the completion pool and the engine to disagree about what exists.
    /// Asserted through `functions()` rather than through `ctx.udfs()` for exactly that reason:
    /// the registry is the easy half, and it is the *snapshot* that the surfaces read.
    #[test]
    fn json_accessors_reach_the_function_catalogue() {
        let engine = Engine::new(BTreeMap::new());
        let names: Vec<&str> = engine
            .functions()
            .scalar
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        for want in [
            "json_get",
            "json_get_str",
            "json_get_int",
            "json_as_text",
            "json_contains",
            "json_length",
        ] {
            assert!(
                names.contains(&want),
                "'{want}' is registered on the context but absent from the function catalogue"
            );
        }
    }

    /// A bare `->` **used to panic the query task**, and must not.
    ///
    /// `json_get` returns a sparse `Union` (the crate's stand-in for Postgres `jsonb`, which Arrow
    /// has no equivalent of). Parquet has no union logical type and `arrow_to_parquet_schema`
    /// panics rather than erroring, so the snapshot every run materializes took the task down with
    /// `not implemented: See ARROW-8817`. `query::flatten_json_unions` projects the column to its
    /// canonical JSON text before the writer sees it.
    ///
    /// The four cases below are one per union arm that survives to a result, which is what makes
    /// this a test of the *flattening* rather than of one expression.
    #[tokio::test]
    async fn a_bare_json_arrow_yields_text_instead_of_panicking() {
        let eng = Engine::new(Default::default());
        let doc = r#"'{"s": "x", "n": 7, "b": true, "o": {"k": 1}, "a": [1,2], "z": null}'"#;

        let (out, _) = eng
            .query(
                WsId(1),
                RunTag(1),
                format!(
                    "SELECT {doc} -> 's' AS s, {doc} -> 'n' AS n, {doc} -> 'b' AS b, \
                     {doc} -> 'o' AS o, {doc} -> 'a' AS a, {doc} -> 'z' AS z"
                ),
                10,
            )
            .await
            .expect("a union column no longer reaches the parquet writer");

        let row: Vec<String> = out.rows[0].iter().map(|c| c.text.clone()).collect();
        assert_eq!(
            row,
            vec![
                r#""x""#.to_string(), // a string arm is JSON-quoted
                "7".to_string(),      // int
                "true".to_string(),   // bool
                // The object and array arms are the source's own text, passed through
                // **verbatim** — note the space after `k:`, which is how it was written. They
                // are already raw JSON inside the union, so nothing re-serializes them and the
                // user gets back exactly what the document held.
                r#"{"k": 1}"#.to_string(),
                "[1,2]".to_string(),
                "NULL".to_string(), // the JSON-null arm becomes a SQL null
            ]
        );
        assert!(
            out.rows[0][5].null,
            "the JSON null arm is a real null, not the text"
        );

        // And the schema the grid is handed says text, not union — the projection is on the
        // logical plan, so `ColumnInfo` and the snapshot cannot disagree.
        assert!(
            out.columns.iter().all(|c| !c.dtype.contains("Union")),
            "{:?}",
            out.columns.iter().map(|c| &c.dtype).collect::<Vec<_>>()
        );
    }

    /// A union **nested** inside a struct now stores as itself.
    ///
    /// It used to be refused by name, because `json_union_to_text` takes the union directly so
    /// there was nothing to wrap it with, and parquet would have panicked on it. The IPC snapshot
    /// holds it, which is the fidelity the format change bought: the type that reaches the writer
    /// is the type the query produced, and no coercion or refusal stands between them.
    #[tokio::test]
    async fn a_json_value_nested_in_a_struct_round_trips() {
        let eng = Engine::new(Default::default());
        let (out, _) = eng
            .query(
                WsId(1),
                RunTag(1),
                r#"SELECT struct('{"a":1}' -> 'a' AS v) AS wrapped"#.into(),
                10,
            )
            .await
            .expect("a nested union is storable under IPC");
        assert_eq!(out.total, 1);
        assert_eq!(out.columns.len(), 1);
        assert_eq!(out.columns[0].name, "wrapped");
    }

    /// And they evaluate. A path accessor over a JSON *string* with nothing registered is the
    /// case that says these belong to the engine rather than to a table — which is why they are
    /// built in `build_context` beside the catalog naming and not in `catalog`.
    #[tokio::test]
    async fn a_json_accessor_needs_no_table() {
        use datafusion::arrow::array::StringArray;

        let ctx = build_context(&BTreeMap::new());
        let batches = ctx
            .sql(r#"SELECT json_get_str('{"a":{"b":"deep"}}', 'a', 'b') AS v"#)
            .await
            .expect("plan")
            .collect()
            .await
            .expect("run");

        let col = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("json_get_str returns Utf8");
        assert_eq!(col.value(0), "deep");
    }

    /// A catalogue entry is a promise that the key applies. Four `runtime.*` keys were
    /// catalogued, described and badged RESTART while `build_runtime` never read them, so setting
    /// one did nothing at all — invisible until P4-07 gave them an editor. This is the guard:
    /// every `runtime.*` key the catalogue names must reach `RuntimeEnvBuilder`.
    ///
    /// It shows up as **the builder rejecting a value that key's kind cannot hold**, which is
    /// something only a key that is actually read can do. (It used to be "the builder answered
    /// with a runtime rather than `None`", which stopped meaning anything when `build_runtime`
    /// started always building one — the list-files cache default is ours, so there is no longer
    /// a "nothing to configure" answer to distinguish.) A free-text key has no invalid value and
    /// is exempted by name rather than silently skipped.
    #[test]
    fn every_catalogued_runtime_key_reaches_the_runtime_builder() {
        for entry in config::ENGINE_KEYS
            .iter()
            .filter(|e| config::is_restart_key(e.key))
        {
            // Blank means "unset" for several of these, so exercise a real value instead.
            let value = match entry.default {
                "" => match entry.kind {
                    config::Kind::Bytes => "64M",
                    config::Kind::Duration => "30s",
                    _ => "/tmp/strata-runtime-test",
                },
                default => default,
            };
            build_runtime(&overrides(&[(entry.key, value)]))
                .unwrap_or_else(|e| panic!("{} = {value} was rejected: {e}", entry.key));

            // `temp_directory` is a path: every string is a legal value, so there is nothing a
            // reader could refuse and no negative to assert.
            if entry.key == "datafusion.runtime.temp_directory" {
                continue;
            }
            assert!(
                build_runtime(&overrides(&[(entry.key, "nonsense")])).is_err(),
                "{} accepted a value its kind cannot hold — the key is catalogued but never \
                 read, so setting it does nothing and says nothing",
                entry.key
            );
        }
    }

    /// The list-files cache is **off by Strata's default**, not DataFusion's — which turns it on
    /// at 1M with an infinite TTL. Every re-scan promise depends on it: the catalog's Refresh,
    /// the Configure window's re-inference and `CREATE OR REPLACE TABLE` all mean "list the
    /// sources again", and a cached listing answers each of them with the previous file set.
    #[test]
    fn the_list_files_cache_is_off_unless_the_user_asks_for_one() {
        let default = build_runtime(&BTreeMap::new()).expect("runtime");
        assert!(
            default.cache_manager.get_list_files_cache().is_none(),
            "a fresh engine caches no listing"
        );
        // Still the user's to turn on — it is a default, not an owned key.
        let asked = build_runtime(&overrides(&[(
            "datafusion.runtime.list_files_cache_limit",
            "4M",
        )]))
        .expect("runtime");
        assert!(asked.cache_manager.get_list_files_cache().is_some());
    }

    #[test]
    fn a_runtime_ttl_is_read_the_way_the_field_validates_it() {
        // The validator and the parser are one function (`util::parse_duration`), so a field that
        // accepts `2m` cannot be read as two seconds.
        assert!(build_runtime(&overrides(&[(
            "datafusion.runtime.list_files_cache_ttl",
            "2m"
        )]))
        .is_ok());
        assert_eq!(
            crate::util::parse_duration("2m"),
            Some(std::time::Duration::from_secs(120))
        );
        assert!(build_runtime(&overrides(&[(
            "datafusion.runtime.list_files_cache_ttl",
            "nonsense"
        )]))
        .is_err());
    }

    #[test]
    fn set_config_leaves_the_catalog_names_alone() {
        let engine = Engine::new(BTreeMap::new());
        // A stale saved override naming a key the app owns must not re-point name resolution at
        // a catalog that was never created (`is_owned_key`).
        engine.set_config(overrides(&[(
            "datafusion.catalog.default_schema",
            "elsewhere",
        )]));
        assert_eq!(live(&engine, "datafusion.catalog.default_schema"), SCHEMA);
    }

    #[test]
    fn a_nameable_ident_is_emitted_bare_and_case_folded() {
        // The fold-preserving contract: a name that could already be interpolated must
        // come out with the *same* engine identity it has always had — bare, lowercased
        // exactly the way DataFusion's own `normalize_ident` lowercased it.
        for name in ["daily_sales", "_scratch", "t9", "orders2024"] {
            assert_eq!(quote_ident(name), name, "already folded — untouched");
        }
        assert_eq!(quote_ident("DailySales"), "dailysales");
        assert_eq!(quote_ident("Revenue"), "revenue");
        assert_eq!(quote_ident("ORDERS"), "orders");
    }

    #[test]
    fn only_an_unsayable_name_is_quoted_and_it_is_escaped() {
        // These four were malformed SQL before quoting existed, so there is no prior
        // identity to preserve — quoting them is pure capability.
        assert_eq!(quote_ident("Sales 2024"), "\"Sales 2024\"");
        assert_eq!(quote_ident("2024"), "\"2024\"", "can't lead with a digit");
        assert_eq!(quote_ident("sales-eu"), "\"sales-eu\"");
        assert_eq!(
            quote_ident("say \"hi\""),
            "\"say \"\"hi\"\"\"",
            "an embedded quote is doubled, not dropped"
        );
        // A reserved word is the other case, and it is *not* one of the broken ones —
        // `CREATE VIEW Order …` already parsed (see `quote_ident`'s doc). Quoting it is
        // defensive, and safe only because the fold runs first: the identity stays the
        // lowercase one, the same name `register_table("Order")` would give a table.
        assert_eq!(quote_ident("order"), "\"order\"");
        assert_eq!(quote_ident("Order"), "\"order\"");
    }

    #[test]
    fn the_folded_name_is_the_one_datafusion_resolves() {
        // `fold_ident` must agree with `TableReference::parse_str` — the path a table
        // registers through — or a generated `FROM` names a different object than the
        // catalog row it came from.
        for name in ["daily_sales", "MyView", "Order", "Sales 2024", "2024"] {
            assert_eq!(
                fold_ident(name),
                TableReference::parse_str(name).table(),
                "{name:?}"
            );
        }
        // A dotted name is a label, not a qualification (the engine owns one schema).
        assert_eq!(fold_ident("a.b"), "a.b");
    }

    #[tokio::test]
    async fn a_view_round_trips_under_the_name_it_was_given() {
        let eng = Engine::new(Default::default());
        // Names straight out of a shared `.strata/project.json`: the plain one must keep
        // working exactly as before, the awkward ones must work at all.
        for (i, name) in ["daily_sales", "Sales 2024", "say \"hi\"", "Order"]
            .iter()
            .enumerate()
        {
            let meta = eng
                .create_view((*name).into(), "SELECT 1 AS n".into())
                .await
                .unwrap_or_else(|e| panic!("create {name:?}: {e}"));
            assert_eq!(meta.columns.len(), 1, "the view's own schema came back");

            let ws = WsId(1);
            let select = format!("SELECT * FROM {}", quote_ident(name));
            let (out, _) = eng
                .query(ws, RunTag(i as u128 * 2), select.clone(), 10)
                .await
                .unwrap_or_else(|e| panic!("select from {name:?}: {e}"));
            assert_eq!(out.total, 1);

            eng.drop_view((*name).into())
                .await
                .unwrap_or_else(|e| panic!("drop {name:?}: {e}"));
            eng.query(ws, RunTag(i as u128 * 2 + 1), select, 10)
                .await
                .expect_err("the drop named the same view the create did");
        }
    }

    /// The upgrade guarantee. A `.strata/project.json` written before quoting existed can
    /// hold a view named `DailySales`, which DataFusion registered as `dailysales` — and
    /// sibling defs / saved queries say `FROM dailysales`. Quoting must not re-key it.
    #[tokio::test]
    async fn a_mixed_case_view_still_registers_under_its_folded_name() {
        let eng = Engine::new(Default::default());
        eng.create_view("DailySales".into(), "SELECT 1 AS n".into())
            .await
            .expect("create");

        // A sibling def, written against the folded spelling, still resolves — this is
        // the one that used to land Failed forever after an unmigrated re-key.
        let meta = eng
            .create_view("Derived".into(), "SELECT * FROM dailysales".into())
            .await
            .expect("a def referencing the folded name");
        assert_eq!(meta.columns.len(), 1, "…and planned against it");

        // …and so does a saved query typed in any case, because bare names still fold.
        for sql in ["SELECT * FROM dailysales", "SELECT * FROM DailySales"] {
            let (out, _) = eng
                .query(WsId(1), RunTag(1), sql.into(), 10)
                .await
                .unwrap_or_else(|e| panic!("{sql}: {e}"));
            assert_eq!(out.total, 1);
        }

        eng.drop_view("DailySales".into()).await.expect("drop");
        eng.query(WsId(1), RunTag(2), "SELECT * FROM dailysales".into(), 10)
            .await
            .expect_err("the drop named the same view the create did");
    }

    /// Which of `probes` resolve against `ctx` — "reachable as", asked of DataFusion rather
    /// than re-derived here. [`TableReference::bare`] so each probe is taken verbatim and
    /// never folded a second time on its way in.
    async fn reachable(ctx: &SessionContext, probes: &[&str]) -> Vec<String> {
        let mut hit = Vec::new();
        for p in probes {
            if ctx.table(TableReference::bare(*p)).await.is_ok() {
                hit.push((*p).to_string());
            }
        }
        hit
    }

    /// The **identities** `ctx`'s schema is keyed by, whatever spelling registered them —
    /// `StrataSchemaProvider`'s own keys, minus the result snapshots it hides.
    fn registered(ctx: &SessionContext) -> Vec<String> {
        ctx.catalog(CATALOG)
            .expect("our catalog")
            .schema(SCHEMA)
            .expect("our schema")
            .table_names()
    }

    /// **The fold-preservation oracle.** The tests above pin `quote_ident` to hardcoded
    /// expectations; this one pins it to *the code it replaced*, which is the property that
    /// actually matters — an existing `.strata/project.json`, written and registered by the
    /// shipped app, must keep working across this change.
    ///
    /// The shipped `create_view` interpolated the raw name unquoted and let the planner fold
    /// it. So: register every name both ways — the old statement into one engine, today's
    /// `create_view` into another — and require the two contexts to be reachable under
    /// *exactly* the same spellings. That covers the headline case (`MyView` must still be
    /// `myview`, never `"MyView"`) without either side asserting what the answer should be.
    #[tokio::test]
    async fn quoting_keeps_the_identity_the_unquoted_interpolation_gave_a_name() {
        // Names the old, unquoted interpolation could already handle — the only ones with a
        // prior identity to preserve. `Order` is in here deliberately: it is a reserved word,
        // but `CREATE VIEW Order …` parsed and registered as `order` under DataFusion's
        // dialect, so it has a prior identity like any other and quoting must not move it.
        const NAMES: &[&str] = &["MyView", "DailySales", "daily_sales", "ORDERS", "Order"];
        // Every spelling worth asking about for those names, folded and unfolded alike.
        const PROBES: &[&str] = &[
            "myview",
            "MyView",
            "MYVIEW",
            "dailysales",
            "DailySales",
            "daily_sales",
            "orders",
            "ORDERS",
            "order",
            "Order",
        ];

        let legacy = Engine::new(Default::default());
        let now = Engine::new(Default::default());
        for name in NAMES {
            // Verbatim the statement the shipped code built.
            let df = legacy
                .ctx
                .sql(&format!("CREATE OR REPLACE VIEW {name} AS SELECT 1 AS n"))
                .await
                .unwrap_or_else(|e| panic!("the shipped path handled {name:?}: {e}"));
            let _ = df.collect().await;
            now.create_view((*name).into(), "SELECT 1 AS n".into())
                .await
                .unwrap_or_else(|e| panic!("create_view {name:?}: {e}"));
        }

        // Sanity, asked of the **identity** each view registered under rather than of what
        // resolves. `StrataSchemaProvider` keys the namespace by `fold_ident` on both sides
        // (ED-03), so every spelling of a name now resolves and a probe list can no longer
        // show a fold happening; the stored key still can, and it is the stricter question.
        assert_eq!(
            registered(&legacy.ctx),
            ["daily_sales", "dailysales", "myview", "order", "orders"],
            "sanity: the shipped path folded each name to its lowercase spelling"
        );
        assert_eq!(
            registered(&now.ctx),
            registered(&legacy.ctx),
            "every name must keep exactly the identity it always had — a sibling def saying \
             `FROM myview` has no migration if this changes"
        );
        assert_eq!(
            reachable(&now.ctx, PROBES).await,
            reachable(&legacy.ctx, PROBES).await,
            "and must stay reachable under exactly the spellings it always was"
        );
    }

    /// The other half of the contract: the names quoting *added* really were broken before,
    /// so there is no prior identity for them to preserve and quoting them is a pure
    /// capability gain rather than a re-keying.
    ///
    /// Reserved words are pointedly **not** in this list. `CREATE VIEW Order …` parses fine
    /// under DataFusion's `GenericDialect` (as does a bare `FROM order`), so `order` has a
    /// real prior identity and is covered by the oracle above instead — quoting it is
    /// defensive, not a repair, and the doc on [`quote_ident`] says so.
    #[tokio::test]
    async fn the_names_quoting_added_were_malformed_sql_before() {
        let eng = Engine::new(Default::default());
        for name in ["Sales 2024", "sales-eu", "2024", "say \"hi\""] {
            let stmt = format!("CREATE OR REPLACE VIEW {name} AS SELECT 1 AS n");
            assert!(
                eng.ctx.sql(&stmt).await.is_err(),
                "{name:?} was malformed unquoted — nothing was ever registered under it"
            );
        }
    }

    /// Register the `regions.csv` fixture (5 rows, `country` + `region`) under `name`.
    async fn register_regions(eng: &Engine, name: &str) {
        eng.register(TableSpec {
            name: name.into(),
            paths: vec![format!(
                "{}/tests/fixtures/loadfix/regions.csv",
                env!("CARGO_MANIFEST_DIR")
            )],
            format: SourceFormat::from_name("csv"),
            partitions: Vec::new(),
            internal: false,
        })
        .await
        .expect("register");
    }

    /// The "view as query" SQL has to *resolve*, not merely read well — the button drops
    /// it into a scratch tab for the user to run. A table the user named `Regions`
    /// registers as `regions` (`register_table` takes its `&str` through
    /// `TableReference::parse_str`), so the generated `FROM` must say `regions`; the
    /// always-quote helper this replaced printed `FROM "Regions"`, which plans against
    /// nothing.
    #[tokio::test]
    async fn the_profile_sql_for_a_mixed_case_table_actually_runs() {
        let eng = Engine::new(Default::default());
        register_regions(&eng, "Regions").await;

        let profile = eng.profile("Regions".into()).await.expect("profile");
        assert!(
            profile.sql.contains("FROM regions"),
            "the folded name, bare: {}",
            profile.sql
        );
        eng.query(WsId(1), RunTag(1), profile.sql.clone(), 10)
            .await
            .unwrap_or_else(|e| panic!("re-running the printed query: {e}\n{}", profile.sql));
    }

    /// A scan through the facade: the rows it read, and the per-type facts for the columns it
    /// found. `regions.csv` is two `Utf8` columns, so each gets distinct / min / max and a
    /// null count — and *not* mean / median, which are a type error on a string and would
    /// fail the whole aggregate rather than one column (`profile::aggregates`).
    #[tokio::test]
    async fn a_scan_lands_the_per_type_facts_of_every_column() {
        let eng = Engine::new(Default::default());
        register_regions(&eng, "regions").await;

        let profile = eng.profile("regions".into()).await.expect("profile");

        assert_eq!(profile.rows, 5);
        let keys = |col: &str| {
            let mut keys: Vec<StatKey> = profile.cols[col].iter().map(|s| s.key).collect();
            keys.sort_by_key(|k| format!("{k:?}"));
            keys
        };
        for col in ["country", "region"] {
            assert_eq!(
                keys(col),
                vec![
                    StatKey::Distinct,
                    StatKey::Max,
                    StatKey::Min,
                    StatKey::Nulls
                ],
                "{col}: a string column's facts, and no mean/median"
            );
        }
        let stat = |col: &str, key: StatKey| {
            profile.cols[col]
                .iter()
                .find(|s| s.key == key)
                .map(|s| s.text.clone())
                .unwrap_or_else(|| panic!("{col} has no {key:?}"))
        };
        assert_eq!(stat("region", StatKey::Distinct), "2", "EMEA and APAC");
        assert_eq!(stat("region", StatKey::Min), "APAC");
        assert_eq!(stat("region", StatKey::Nulls), "0");
        assert!(
            profile.cols["country"].iter().all(|s| s.exact),
            "computed, not read from a truncatable footer"
        );
    }

    /// Cancel, and what the flag says while a scan runs. Both halves are the point: a scan is
    /// the most expensive thing the app does, so the window-close confirm counts it as work in
    /// flight (D4) — and the Cancel in the inspector's running state has to actually stop it.
    ///
    /// The subject is a **view** over `generate_series`, which is also the case a scan matters
    /// most for (a view has no footer at all): 50M rows of `count(distinct …)` is comfortably
    /// slow enough to observe, and aborts at the next await.
    #[tokio::test]
    async fn a_scan_is_work_in_flight_and_cancel_stops_it() {
        let eng = Engine::new(Default::default());
        let flag = Arc::new(AtomicBool::new(false));
        eng.watch_inflight(flag.clone());
        eng.create_view("slow".into(), SLOW_ROWS.into())
            .await
            .expect("create view");
        assert!(!flag.load(Ordering::Relaxed), "idle to begin with");

        let observe = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            let flagged = flag.load(Ordering::Relaxed);
            let cancelled = eng.cancel_profile("slow");
            (flagged, cancelled)
        };
        let (settled, (flagged, cancelled)) = tokio::join!(eng.profile("slow".into()), observe);

        assert!(flagged, "a running scan is work in flight");
        assert!(cancelled, "…and the cancel found it");
        assert_eq!(
            settled.as_ref().err().map(String::as_str),
            Some("cancelled")
        );
        assert!(!flag.load(Ordering::Relaxed), "cleared once it settled");
        assert!(
            !eng.cancel_profile("slow"),
            "nothing left in flight to cancel"
        );
        // A tab's own probe is untouched: a profile is not a tab's work, so the *tab*-close
        // confirm must not count it (D4).
        assert!(!eng.is_running(WsId(1)));
    }

    /// Two things at once, because they are the same bookkeeping. A **re-scan supersedes**: the
    /// older call reports no numbers, and the newer one owns the entry — which the late cancel
    /// proves, since a settle path that keyed on the *name* would have removed the newer scan's
    /// entry on its way out and left nothing to cancel (and the flag latched). And profiles are
    /// **keyed per entry**, so a scan of another table runs alongside rather than being
    /// superseded by it.
    #[tokio::test]
    async fn a_re_scan_supersedes_its_own_entry_and_nobody_elses() {
        let eng = Engine::new(Default::default());
        eng.create_view("slow".into(), SLOW_ROWS.into())
            .await
            .expect("create view");
        register_regions(&eng, "regions").await;

        let first = eng.profile("slow".into());
        let re_scan = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            eng.profile("slow".into()).await
        };
        // Dispatched with `first`, so both scans are in flight together.
        let other = eng.profile("regions".into());
        let stop = async {
            tokio::time::sleep(std::time::Duration::from_millis(75)).await;
            eng.cancel_profile("slow")
        };
        let (first, re_scan, other, stopped) = tokio::join!(first, re_scan, other, stop);

        assert!(first.is_err(), "the superseded scan reports no numbers");
        assert!(
            stopped,
            "the re-scan owns the entry, so there was one to cancel"
        );
        assert!(re_scan.is_err(), "…and that cancel landed on it");
        assert_eq!(
            other.map(|p| p.rows),
            Ok(5),
            "another entry's scan ran alongside, untouched"
        );
    }

    /// Re-registering a table aborts its scan: the numbers would describe files the register
    /// is replacing, so nothing should go on paying for them. Engine-side rather than left to
    /// the caller, so every path that re-registers gets it.
    #[tokio::test]
    async fn re_registering_a_table_aborts_its_scan() {
        let eng = Engine::new(Default::default());
        eng.create_view("slow".into(), SLOW_ROWS.into())
            .await
            .expect("create view");

        let scan = eng.profile("slow".into());
        let replace = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            // The view's own re-definition is the view-shaped half of the same rule.
            eng.create_view("slow".into(), "SELECT 1 AS n".into())
                .await
                .expect("re-create");
        };
        let (scan, ()) = tokio::join!(scan, replace);

        assert_eq!(scan.as_ref().err().map(String::as_str), Some("cancelled"));
    }

    /// The close-while-running guard's two probes (T2). Both are answered from the
    /// lifecycle map on purpose: the run under test has no UI at all here, which is
    /// exactly the background-tab case a mounted-view derivation cannot see.
    #[tokio::test]
    async fn the_inflight_flag_and_the_per_workspace_probe_track_a_run() {
        let eng = Engine::new(Default::default());
        let flag = Arc::new(AtomicBool::new(false));
        eng.watch_inflight(flag.clone());
        assert!(!flag.load(Ordering::Relaxed), "seeded from an idle engine");

        let (ws, tag) = (WsId(1), RunTag(1));
        let observe = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            let seen = (flag.load(Ordering::Relaxed), eng.is_running(ws));
            eng.cancel(ws, tag);
            seen
        };
        let (settled, seen) = tokio::join!(eng.query(ws, tag, SLOW.into(), 10), observe);

        assert_eq!(seen, (true, true), "flagged for as long as it executes");
        assert!(settled.is_err(), "the cancel landed");
        assert!(!flag.load(Ordering::Relaxed), "cleared once it settled");
        assert!(!eng.is_running(ws));
    }

    /// **Background work raises the same flag a run does** — the close confirm's whole gate is
    /// that one `AtomicBool` (`close::CloseHook::running`), so anything the user would rather
    /// finish has to be counted in it, not merely runnable.
    ///
    /// Asserted on the guard rather than by racing a real drop or export: the guard *is* the
    /// mechanism (`Engine::drop_table` and `Engine::export` each hold one for the length of
    /// their await), the count is what a leaked increment would strand true for the engine's
    /// whole life, and a test that had to make a delete slow enough to observe would be timing
    /// against a filesystem.
    #[test]
    fn background_work_raises_the_close_confirms_flag_and_releases_it() {
        let eng = Engine::new(Default::default());
        let flag = Arc::new(AtomicBool::new(false));
        eng.watch_inflight(flag.clone());
        assert!(!flag.load(Ordering::Relaxed), "seeded from an idle engine");
        assert!(!eng.has_background_work());

        {
            let _first = BackgroundGuard::new(&eng);
            assert!(flag.load(Ordering::Relaxed), "the window would now ask");
            assert!(eng.has_background_work());
            // Two at once — an export writing while a drop deletes — so the release is a
            // decrement and not a reset. A `bool` here would have the first to finish tell the
            // window the second was done too.
            let second = BackgroundGuard::new(&eng);
            drop(second);
            assert!(
                flag.load(Ordering::Relaxed),
                "still flagged while the other is going"
            );
        }

        assert!(!flag.load(Ordering::Relaxed), "cleared once both released");
        assert!(!eng.has_background_work());
    }

    #[tokio::test]
    async fn a_repeat_dispatch_of_one_tag_leaves_the_newer_run_intact() {
        let eng = Engine::new(Default::default());
        let (ws, tag) = (WsId(1), RunTag(7));
        // The UI can dispatch one logical run twice under the same tag (freya-query
        // re-runs an entry when a subscriber remounts mid-flight). The second dispatch
        // supersedes the first; while "latest" was decided by tag, the first's settle
        // path adopted the second's `InFlight` entry and *both* calls failed.
        let first = eng.query(ws, tag, SLOW.into(), 10);
        let second = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            eng.query(ws, tag, FAST.into(), 10).await
        };
        let (first, second) = tokio::join!(first, second);

        let (out, _) = second.expect("the newer dispatch settles Ok");
        let snap = out.snapshot.expect("…owning a snapshot of its own");
        eng.fetch_page(snap, 1, 10, None)
            .await
            .expect("…which the older dispatch did not retire");
        if let Err(e) = &first {
            assert_eq!(e, "cancelled", "the superseded dispatch settles cancelled");
        }
        assert!(
            eng.cancel(ws, tag).is_none(),
            "both settled — nothing left in flight"
        );
    }

    // ---- the Run router (ED-02) -----------------------------------------------------

    /// **A query through `run` is a query through `query`.** The whole promise of routing is
    /// that the read path did not move: same snapshot handle, same page 1, same totals — so a
    /// regression here is the router having grown an opinion it has no business having.
    #[tokio::test]
    async fn a_query_routed_through_run_is_the_query_path_unchanged() {
        let eng = Engine::new(Default::default());
        let sql = "SELECT * FROM (VALUES (2), (1), (3)) AS t";

        let RunOutcome::Rows(routed, _) = eng
            .run(WsId(1), RunTag(1), sql.into(), 2)
            .await
            .expect("a SELECT runs")
        else {
            panic!("a SELECT settles rows");
        };
        let (direct, _) = eng
            .query(WsId(2), RunTag(2), sql.into(), 2)
            .await
            .expect("…as it always did");

        assert_eq!(routed.total, direct.total);
        assert_eq!(routed.rows, direct.rows);
        assert_eq!(routed.columns.len(), direct.columns.len());
        // And it really materialized: the handle pages, which is the half a report cannot fake.
        let snap = routed.snapshot.expect("a snapshot handle");
        let (page2, _) = eng.fetch_page(snap, 2, 2, None).await.expect("page 2");
        assert_eq!(page2.len(), 1);
    }

    /// A refused statement fails with **the squiggle's own words** — `Blocked::editor_message`,
    /// not DataFusion's account of a rule that is ours. `CREATE DATABASE` is the refusal that
    /// stays refused: it is structurally impossible, not merely unimplemented.
    #[tokio::test]
    async fn a_refused_statement_fails_with_the_editors_message() {
        let eng = Engine::new(Default::default());
        let err = eng
            .run(WsId(1), RunTag(1), "CREATE DATABASE d".into(), 10)
            .await
            .err()
            .expect("refused");
        assert_eq!(err, Blocked::CreateDatabase.editor_message());
    }

    /// An intercepted kind whose task has not landed fails with its **stub** refusal, naming
    /// the statement. The distinction matters: it classified, so the editor drew no squiggle,
    /// and the run has to say plainly why nothing happened.
    #[tokio::test]
    async fn an_unimplemented_interception_names_the_statement() {
        let eng = Engine::new(Default::default());
        let err = eng
            .run(
                WsId(1),
                RunTag(1),
                "SET datafusion.execution.batch_size = 2".into(),
                10,
            )
            .await
            .err()
            .expect("not implemented yet");
        assert_eq!(err, "SET is not implemented yet");
    }

    /// A statement that **is** implemented still needs somewhere to put what it makes, and an
    /// engine with no project behind it says so rather than failing in DataFusion's words about
    /// a path nobody chose (ED-04).
    #[tokio::test]
    async fn creating_a_table_without_a_project_folder_says_why() {
        let eng = Engine::new(Default::default());
        let err = eng
            .run(WsId(1), RunTag(1), "CREATE TABLE t AS SELECT 1".into(), 10)
            .await
            .err()
            .expect("nowhere to store it");
        assert_eq!(
            err,
            "CREATE TABLE AS needs a project folder to store the table's data"
        );
    }

    /// **Neither refusal touches the snapshot lifecycle.** DDL does not retire a snapshot
    /// (SNAPSHOT_SPEC §4), so the workspace's settled result is still there to page after a
    /// statement runs in the same tab — which is also what makes the results pane's "previous
    /// snapshot survives" claim true rather than hopeful.
    #[tokio::test]
    async fn a_statement_leaves_the_workspaces_snapshot_alone() {
        let eng = Engine::new(Default::default());
        let (ws, sql) = (WsId(1), "SELECT * FROM (VALUES (1), (2)) AS t");

        let (out, _) = eng
            .query(ws, RunTag(1), sql.into(), 10)
            .await
            .expect("rows");
        let snap = out.snapshot.expect("a snapshot handle");

        for stmt in ["CREATE DATABASE d", "DROP TABLE t"] {
            eng.run(ws, RunTag(2), stmt.into(), 10)
                .await
                .err()
                .expect("refused or stubbed");
            assert!(eng.snapshot_live(snap), "{stmt} retired the snapshot");
        }
        eng.fetch_page(snap, 1, 10, None)
            .await
            .expect("…and it still reads");
        assert!(!eng.is_running(ws), "nothing left in flight either");
    }

    /// **One statement per Run**, refused with a policy sentence. Left to DataFusion this is
    /// its parser complaining about its own limit, which tells the user nothing about what to
    /// do next; the buffer is still validated per statement, so the squiggles are unaffected.
    #[tokio::test]
    async fn a_multi_statement_run_is_refused_as_a_batch() {
        let eng = Engine::new(Default::default());
        let err = eng
            .run(WsId(1), RunTag(1), "SELECT 1; SELECT 2".into(), 10)
            .await
            .err()
            .expect("a batch");
        assert_eq!(err, "Run executes one statement at a time");
    }

    /// **…and a terminated statement is one statement.** This is the gate's blast radius, not a
    /// corner: people end statements with `;`, and if `parse_statements` ever counted the
    /// trailing delimiter as a second (empty) statement, *every* such Run would refuse as a
    /// batch — a total regression of Run, reported as a policy message so it would read like a
    /// deliberate rule. Pinned here rather than trusted, because it is DataFusion's behaviour
    /// and not ours. A `;` inside a string literal rides along for the same reason: the split
    /// is the tokenizer's, so it must not be a text split.
    #[tokio::test]
    async fn a_terminated_statement_is_still_one_statement() {
        let eng = Engine::new(Default::default());
        for sql in [
            "SELECT 1 AS n;",
            "SELECT 1 AS n;\n",
            "SELECT 1 AS n ;;",
            "-- a note\nSELECT 1 AS n;",
            "SELECT ';' AS n;",
            "WITH t AS (SELECT 1 AS n) SELECT * FROM t;",
        ] {
            let outcome = eng.run(WsId(1), RunTag(1), sql.into(), 10).await;
            assert!(
                matches!(outcome, Ok(RunOutcome::Rows(_, _))),
                "{sql:?} must run, not refuse as a batch"
            );
        }
    }

    /// A buffer with nothing to run in it says so. Not unreachable from the app: the blank-buffer
    /// gate upstream (`press_query`) trims whitespace, and a comment is not whitespace.
    #[tokio::test]
    async fn a_buffer_of_only_comments_has_nothing_to_run() {
        let eng = Engine::new(Default::default());
        let err = eng
            .run(WsId(1), RunTag(1), "-- just thinking out loud\n".into(), 10)
            .await
            .err()
            .expect("nothing to run");
        assert_eq!(err, "Nothing to run");
    }
}

/// **Read options, end to end** — every option the Configure window offers, proved against a
/// real file rather than against DataFusion's builder signature.
///
/// This is the point of P4-11's validation pass. The bar for offering an option is that it
/// reaches the read, and the only way to hold that bar over a DataFusion upgrade is to register
/// a table whose schema or rows are *different* because the option is set. Each test here
/// therefore asserts the difference, not the call.
#[cfg(test)]
mod read_options_tests {
    use std::io::Write;
    use std::path::PathBuf;

    use strata_model::{CsvRead, FileCompression, JsonRead, JsonShape, SourceFormat};

    use super::*;

    /// A fresh directory per test, so a stale fixture can never make one pass.
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join("strata_read_options").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    fn write(dir: &PathBuf, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("fixture");
        path.to_string_lossy().into_owned()
    }

    /// The same, gzipped — a compression option can only be proved by a genuinely compressed
    /// file whose name carries the suffix.
    fn write_gz(dir: &PathBuf, name: &str, body: &str) -> String {
        let path = dir.join(name);
        let mut enc = flate2::write::GzEncoder::new(
            std::fs::File::create(&path).expect("fixture"),
            flate2::Compression::default(),
        );
        enc.write_all(body.as_bytes()).expect("compress");
        enc.finish().expect("finish");
        path.to_string_lossy().into_owned()
    }

    fn spec(name: &str, paths: Vec<String>, format: SourceFormat) -> TableSpec {
        TableSpec {
            name: name.into(),
            paths,
            format,
            partitions: Vec::new(),
            internal: false,
        }
    }

    fn names(meta: &TableMeta) -> Vec<String> {
        meta.columns.iter().map(|c| c.name.clone()).collect()
    }

    /// **Why there is no "NULL value" option**, proved against DataFusion's own DDL rather than
    /// against its source — because its `CREATE EXTERNAL TABLE` docs advertise `NULL_VALUE` in
    /// exactly this position, which is what makes the absence look like an oversight.
    ///
    /// - `format.null_value` is the **writer's** null representation. The DDL accepts it, the
    ///   reader never consults it, and the result is byte-identical to passing nothing.
    /// - `format.null_regex` is the reader's, and it is wired into schema *inference* only
    ///   (`CsvFormat::infer_schema`), not into `CsvSource`'s reader. So it re-types the column
    ///   on what it saw and then fails the scan on the very token it was told was null.
    ///
    /// Offering either would be a control that does nothing or a control that breaks the table.
    /// If a future DataFusion wires `null_regex` through to the scan, this test starts failing
    /// on the third case and the option can be added.
    #[tokio::test]
    async fn a_csv_null_value_is_the_writers_and_a_null_regex_breaks_the_scan() {
        use datafusion::prelude::SessionContext;

        let d = dir("csv_null_options");
        std::fs::write(d.join("t.csv"), "a,b\n1,NAN\n2,3\n").expect("fixture");
        let loc = d.to_string_lossy().to_string();

        let read = |opts: &'static str| {
            let loc = loc.clone();
            async move {
                let ctx = SessionContext::new();
                let ddl = format!("CREATE EXTERNAL TABLE t STORED AS csv LOCATION '{loc}/' {opts}");
                ctx.sql(&ddl)
                    .await
                    .expect("ddl")
                    .collect()
                    .await
                    .expect("ddl");
                let df = ctx.sql("SELECT b IS NULL AS n FROM t ORDER BY a").await?;
                df.collect().await
            }
        };

        // The writer's option: accepted, and read exactly as if it were absent.
        let with = read("OPTIONS('format.null_value' 'NAN')")
            .await
            .expect("scan");
        let without = read("").await.expect("scan");
        assert_eq!(
            format!("{with:?}"),
            format!("{without:?}"),
            "NULL_VALUE changes nothing a reader can see"
        );

        // The reader's option: inference types the column on it, then the scan cannot parse it.
        let err = read("OPTIONS('format.null_regex' 'NAN')")
            .await
            .expect_err("the scan cannot parse what inference called null");
        assert!(
            err.to_string().contains("Error while parsing value 'NAN'"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_delimiter_changes_how_the_columns_are_found() {
        let d = dir("delimiter");
        let path = write(&d, "s.csv", "a;b;c\n1;2;3\n");
        let eng = Engine::new(Default::default());

        // Read with the default comma the whole line is one column, header and all.
        let meta = eng
            .register(spec(
                "commas",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect("register");
        assert_eq!(names(&meta), vec!["a;b;c"]);

        let meta = eng
            .register(spec(
                "semis",
                vec![path],
                SourceFormat::Csv(CsvRead {
                    delimiter: ';',
                    ..Default::default()
                }),
            ))
            .await
            .expect("register");
        assert_eq!(names(&meta), vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn no_header_row_names_the_columns_positionally() {
        let d = dir("header");
        let path = write(&d, "s.csv", "1,2\n3,4\n");
        let eng = Engine::new(Default::default());

        let meta = eng
            .register(spec(
                "t",
                vec![path],
                SourceFormat::Csv(CsvRead {
                    header: false,
                    ..Default::default()
                }),
            ))
            .await
            .expect("register");
        assert_eq!(names(&meta), vec!["column_1", "column_2"]);
    }

    #[tokio::test]
    async fn a_comment_character_keeps_the_commented_line_out_of_the_schema() {
        let d = dir("comment");
        let path = write(&d, "s.csv", "# generated\na,b\n1,2\n");
        let eng = Engine::new(Default::default());

        // Without it the comment line is read as the header — a one-column table whose next
        // row has two, which is a register that fails rather than a schema that is merely odd.
        let err = eng
            .register(spec(
                "raw",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect_err("the comment line is taken as the header");
        assert!(err.contains("unequal lengths"), "{err}");

        let meta = eng
            .register(spec(
                "commented",
                vec![path],
                SourceFormat::Csv(CsvRead {
                    comment: Some('#'),
                    ..Default::default()
                }),
            ))
            .await
            .expect("register");
        assert_eq!(names(&meta), vec!["a", "b"]);
    }

    /// The option that earns its place *because* a table is many files — and the one whose
    /// absence is worst, because it does not fail the register.
    ///
    /// Schema inference merges the files happily: the table comes back with the union of the
    /// columns and looks perfectly registered. It is the **scan** that then fails, on every
    /// query, for the files that are short of a column. So this asserts the schema is the same
    /// either way and the *read* is not.
    #[tokio::test]
    async fn truncated_rows_is_what_makes_differently_shaped_files_readable_not_merely_registrable()
    {
        let d = dir("truncated");
        write(&d, "a.csv", "x,y\n1,2\n");
        write(&d, "b.csv", "x,y,z\n3,4,5\n");
        let path = d.to_string_lossy().into_owned();
        let eng = Engine::new(Default::default());

        let meta = eng
            .register(spec(
                "strict",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect("registration succeeds — which is the trap");
        assert_eq!(names(&meta), vec!["x", "y", "z"]);
        let err = eng
            .query(WsId(1), RunTag(1), "SELECT * FROM strict".into(), 100)
            .await
            .expect_err("the short file cannot be read against the merged schema");
        assert!(
            err.to_string().contains("incorrect number of fields"),
            "{err}"
        );

        let meta = eng
            .register(spec(
                "union",
                vec![path],
                SourceFormat::Csv(CsvRead {
                    truncated_rows: true,
                    ..Default::default()
                }),
            ))
            .await
            .expect("register");
        assert_eq!(names(&meta), vec!["x", "y", "z"]);
        eng.query(WsId(2), RunTag(2), "SELECT * FROM union".into(), 100)
            .await
            .expect("the missing column is padded with nulls");
    }

    /// The same option, within one file: a row short of a column fails the register outright.
    #[tokio::test]
    async fn truncated_rows_also_covers_a_ragged_row_inside_one_file() {
        let d = dir("ragged");
        let path = write(&d, "r.csv", "x,y\n1,2\n3\n");
        let eng = Engine::new(Default::default());

        assert!(eng
            .register(spec(
                "strict",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .is_err());

        let meta = eng
            .register(spec(
                "ragged",
                vec![path],
                SourceFormat::Csv(CsvRead {
                    truncated_rows: true,
                    ..Default::default()
                }),
            ))
            .await
            .expect("register");
        assert_eq!(names(&meta), vec!["x", "y"]);
        eng.query(WsId(3), RunTag(3), "SELECT * FROM ragged".into(), 100)
            .await
            .expect("the short row is padded");
    }

    /// Infer-rows at 0 is DataFusion's own "read everything as text" — the one claim the
    /// canvas's hint makes about this field, so it is the one asserted.
    #[tokio::test]
    async fn zero_infer_rows_reads_every_csv_column_as_text() {
        let d = dir("infer");
        let path = write(&d, "s.csv", "n\n1\n2\n");
        let eng = Engine::new(Default::default());

        let meta = eng
            .register(spec(
                "typed",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect("register");
        assert_eq!(meta.columns[0].dtype, "Int64");

        let meta = eng
            .register(spec(
                "text",
                vec![path],
                SourceFormat::Csv(CsvRead {
                    infer_rows: Some(0),
                    ..Default::default()
                }),
            ))
            .await
            .expect("register");
        assert_eq!(meta.columns[0].dtype, "Utf8");
    }

    /// Compression is two things, and the second is the one that bit: the listing filters on
    /// the file **extension**, so a gzipped CSV is only found when the filter says `.csv.gz`.
    #[tokio::test]
    async fn a_compressed_csv_is_both_found_and_decoded() {
        let d = dir("gzip");
        let path = write_gz(&d, "s.csv.gz", "a,b\n1,2\n");
        let eng = Engine::new(Default::default());

        // Uncompressed, the listing's `.csv` filter does not match `s.csv.gz` at all.
        let err = eng
            .register(spec(
                "plain",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect_err("the extension filter excludes it");
        assert!(err.contains("not one"), "{err}");

        let meta = eng
            .register(spec(
                "gz",
                vec![path],
                SourceFormat::Csv(CsvRead {
                    compression: FileCompression::Gzip,
                    ..Default::default()
                }),
            ))
            .await
            .expect("register");
        assert_eq!(names(&meta), vec!["a", "b"]);
    }

    /// A **type-discriminated union** registers and queries, instead of failing schema inference.
    ///
    /// This is the acceptance for `engine::json_poly` at the level that actually matters — through
    /// `Engine::register` and a real `SELECT`, not just the inference unit tests. Arrow's reader
    /// fails this file outright with `Expected object json type, found: Array(…)`, naming neither
    /// the key nor the source. The shape is `sample/config.json`'s, reduced: one key that is a
    /// string, an object, an array and a bool across records.
    #[tokio::test]
    async fn a_polymorphic_json_field_registers_as_text_and_queries() {
        let d = dir("json_poly");
        let path = write(
            &d,
            "c.json",
            concat!(
                r#"{"id": 1, "content": "plain"}"#,
                "\n",
                r#"{"id": 2, "content": {"kind": "block"}}"#,
                "\n",
                r#"{"id": 3, "content": ["a", true]}"#,
                "\n",
                r#"{"id": 4, "content": false}"#,
                "\n",
            ),
        );
        let eng = Engine::new(Default::default());

        let meta = eng
            .register(spec(
                "poly",
                vec![path],
                SourceFormat::Json(JsonRead::default()),
            ))
            .await
            .expect("a conflicted field registers");
        assert_eq!(names(&meta), vec!["content", "id"]);
        let content = meta
            .columns
            .iter()
            .find(|c| c.name == "content")
            .expect("content column");
        assert_eq!(content.dtype, "Utf8", "the conflicted field is text");

        // The values are each record's own JSON, which is what makes the column worth having —
        // `json_get` reads straight into it.
        let (out, _) = eng
            .query(
                WsId(1),
                RunTag(1),
                "SELECT content FROM poly ORDER BY id".into(),
                10,
            )
            .await
            .expect("query");
        let cells: Vec<String> = out.rows.iter().map(|r| r[0].text.clone()).collect();
        assert_eq!(
            cells,
            vec![
                // Quoted: the column holds JSON, so every row of it parses — which is what lets
                // `json_get` read all of them and stops a string containing JSON from reading
                // back as the object it resembles.
                r#""plain""#.to_string(),
                r#"{"kind":"block"}"#.to_string(),
                r#"["a",true]"#.to_string(),
                "false".to_string(),
            ]
        );
    }

    /// An empty JSON object stays an empty struct, end to end.
    ///
    /// It was briefly coerced to text, because parquet cannot write a zero-field struct
    /// (`Parquet does not support writing empty structs`) and `sample/config.json` has 19,159 of
    /// them — a storage workaround wearing an inference rule's clothes. The snapshot is Arrow IPC
    /// now, which stores one, so the reader can say what the source actually contains.
    #[tokio::test]
    async fn an_empty_json_object_stays_an_empty_struct() {
        let d = dir("json_poly_empty_obj");
        let path = write(&d, "t.json", "{\"id\": 1, \"tags\": {}}\n");
        let eng = Engine::new(Default::default());

        let meta = eng
            .register(spec(
                "t",
                vec![path],
                SourceFormat::Json(JsonRead::default()),
            ))
            .await
            .expect("register");
        let tags = meta
            .columns
            .iter()
            .find(|c| c.name == "tags")
            .expect("tags column");
        assert!(
            tags.dtype.starts_with("Struct"),
            "an empty object is an empty struct, not text: {}",
            tags.dtype
        );

        // And it survives the snapshot, which is the half parquet could not do.
        let (out, _) = eng
            .query(WsId(1), RunTag(1), "SELECT * FROM t".into(), 10)
            .await
            .expect("an empty struct reaches the grid");
        assert_eq!(out.total, 1);
    }

    /// A conflict that spans **files** is the same conflict. It used to fail registration:
    /// `infer` ran per file and the results were folded with arrow's `Schema::try_merge`, whose
    /// `Field::try_merge` hard-errors on Struct-vs-Utf8 — so the feature worked inside one file
    /// and not across two, and the single-file test above could not see it.
    #[tokio::test]
    async fn a_conflict_across_files_registers_too() {
        let d = dir("json_poly_multifile");
        write(&d, "a.json", "{\"id\": 1, \"content\": \"text\"}\n");
        write(&d, "b.json", "{\"id\": 2, \"content\": {\"k\": 1}}\n");
        let eng = Engine::new(Default::default());

        let meta = eng
            .register(spec(
                "shards",
                vec![format!("{}/", d.to_string_lossy())],
                SourceFormat::Json(JsonRead::default()),
            ))
            .await
            .expect("a conflict across files registers");
        let content = meta
            .columns
            .iter()
            .find(|c| c.name == "content")
            .expect("content column");
        assert_eq!(content.dtype, "Utf8");
    }

    /// `infer_rows` of 0 is not a sample size, it is a schema with no columns. Left to run it
    /// registered the table **successfully** with zero columns — a green catalog row whose every
    /// query fails with "No field named". The Configure window floors it at 1; a hand-edited
    /// `project.json` reaches the engine directly.
    #[tokio::test]
    async fn zero_infer_rows_is_refused_rather_than_registering_no_columns() {
        let d = dir("json_poly_zero_infer");
        let path = write(&d, "t.json", "{\"a\": 1}\n");
        let eng = Engine::new(Default::default());

        let err = eng
            .register(spec(
                "t",
                vec![path],
                SourceFormat::Json(JsonRead {
                    infer_rows: Some(0),
                    ..Default::default()
                }),
            ))
            .await
            .expect_err("zero is not a sample size");
        assert!(err.contains("at least 1"), "{err}");
    }

    /// A small `datafusion.execution.batch_size` must not lose rows.
    ///
    /// It did: the opener fed records into a decoder and discarded `Decoder::decode`'s returned
    /// byte count, but arrow stops consuming once `batch_size` objects are buffered and expects
    /// the tail to be re-fed. Measured before the fix — 50 rows in, **4 rows out**, no error.
    /// `batch_size` is a catalogued, user-settable Engine key whose own description invites
    /// lowering it.
    #[tokio::test]
    async fn a_small_batch_size_does_not_drop_rows() {
        let d = dir("json_poly_batch");
        let body: String = (0..50).map(|i| format!("{{\"i\": {i}}}\n")).collect();
        let path = write(&d, "rows.json", &body);

        for size in ["8192", "4"] {
            let mut ov = BTreeMap::new();
            ov.insert(
                "datafusion.execution.batch_size".to_string(),
                size.to_string(),
            );
            let eng = Engine::new(ov);
            eng.register(spec(
                "rows",
                vec![path.clone()],
                SourceFormat::Json(JsonRead::default()),
            ))
            .await
            .expect("register");

            let (out, _) = eng
                .query(
                    WsId(1),
                    RunTag(1),
                    "SELECT count(*) AS n FROM rows".into(),
                    10,
                )
                .await
                .expect("query");
            assert_eq!(out.rows[0][0].text, "50", "batch_size={size} lost rows");
        }
    }

    /// The option the canvas never had: a whole-document JSON array is readable, and the
    /// failure message for reading it in the wrong shape now names the setting that fixes it.
    #[tokio::test]
    async fn a_json_array_document_reads_in_array_shape_and_explains_itself_in_the_other() {
        let d = dir("json_shape");
        let path = write(&d, "s.json", "[{\"a\": 1}, {\"a\": 2}]\n");
        let eng = Engine::new(Default::default());

        let err = eng
            .register(spec(
                "ndjson",
                vec![path.clone()],
                SourceFormat::Json(JsonRead::default()),
            ))
            .await
            .expect_err("an array is not newline-delimited");
        assert!(err.contains("Set the JSON shape to array"), "{err}");

        let meta = eng
            .register(spec(
                "array",
                vec![path],
                SourceFormat::Json(JsonRead {
                    shape: JsonShape::Array,
                    ..Default::default()
                }),
            ))
            .await
            .expect("register");
        assert_eq!(names(&meta), vec!["a"]);
    }

    /// A def naming a reader this build does not have fails **by name**. The arm this
    /// replaces was a fallthrough onto parquet, so such a table registered as parquet and
    /// said nothing — the register's one job is to be the check.
    #[tokio::test]
    async fn an_unreadable_format_fails_by_name_rather_than_being_read_as_parquet() {
        let d = dir("unknown");
        let path = write(&d, "s.avro", "not really avro");
        let eng = Engine::new(Default::default());

        let err = eng
            .register(spec(
                "legacy",
                vec![path],
                SourceFormat::Unknown("avro".into()),
            ))
            .await
            .expect_err("no reader");
        assert_eq!(
            err,
            "Table 'legacy' is defined as 'avro', which Strata cannot read."
        );
    }

    /// A multi-byte delimiter is reported, never truncated to its first byte — which would
    /// split fields in the middle of the next character.
    #[tokio::test]
    async fn a_multi_byte_delimiter_is_refused_by_name() {
        let d = dir("wide_delimiter");
        let path = write(&d, "s.csv", "a,b\n1,2\n");
        let eng = Engine::new(Default::default());

        let err = eng
            .register(spec(
                "t",
                vec![path],
                SourceFormat::Csv(CsvRead {
                    delimiter: '→',
                    ..Default::default()
                }),
            ))
            .await
            .expect_err("not a byte");
        assert!(err.contains("single-byte character"), "{err}");

        // The half of the range that used to pass: 'é' is U+00E9, so `c as u32` fits a byte —
        // but the file holds 0xC3 0xA9, and 0xE9 is a UTF-8 lead byte the reader would split on.
        let err = eng
            .register(spec(
                "latin1",
                vec![write(&d, "s2.csv", "a,b\n1,2\n")],
                SourceFormat::Csv(CsvRead {
                    delimiter: 'é',
                    ..Default::default()
                }),
            ))
            .await
            .expect_err("not one byte in UTF-8");
        assert!(err.contains("single-byte character"), "{err}");
    }
}
