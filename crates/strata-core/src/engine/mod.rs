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

mod catalog;
pub mod config;
mod explain;
pub mod export;
mod functions;
pub mod json_poly;
pub mod plan;
pub mod profile;
mod query;
pub mod serialize;
pub mod sql;

pub use catalog::{TableMeta, TableSpec, ViewMeta};
pub use query::purge_snapshot_root;

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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use datafusion::common::TableReference;
use datafusion::execution::runtime_env::RuntimeEnv;
use datafusion::prelude::*;
use tokio::runtime::{Builder, Runtime};
use tokio::task::AbortHandle;

use crate::engine::plan::QueryPlan;
use query::{
    claim_snapshot_dir, discard_snapshot_dir, retire_snapshot, run_and_snapshot, CellFormat,
};
use sql::FunctionCatalog;
use strata_model::{Cell, Diagnostic, QueryOutput, SnapshotId, TabId};

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
    /// How many exports are writing right now. A **count, not a map**: nothing addresses one
    /// export — there is no cancel, no supersede and no per-export state to look up. All it
    /// has to do is keep [`publish_inflight`](Engine::publish_inflight) true while a file is
    /// half-written.
    exports: usize,
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
        }
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
                !lc.inflight.is_empty() || !lc.profiles.is_empty() || lc.exports > 0,
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

    /// The engine's runtime (always present while the engine lives — see the field).
    fn rt(&self) -> &Runtime {
        self.rt.as_ref().expect("engine runtime")
    }

    // --- run / read -------------------------------------------------------

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

        let joined = task.await;

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
        self.rt()
            .spawn(
                async move { query::fetch_page(&ctx, snapshot, page, page_size, sort, &fmt).await },
            )
            .await
            .map_err(|e| format!("page task failed: {e}"))?
    }

    /// Run an `EXPLAIN [ANALYZE]` statement for `ws` — a parsed plan tree, no snapshot.
    /// Supersedes the workspace's in-flight run (mutually exclusive, like a re-run) but
    /// leaves its settled snapshot alone (spec §4: explains materialize nothing).
    pub async fn explain(&self, ws: WsId, tag: RunTag, sql: String) -> Result<QueryPlan, String> {
        let dispatch = self.dispatch_seq.fetch_add(1, Ordering::Relaxed);
        let task = {
            let mut lc = self.lifecycle.lock().unwrap();
            if let Some(prev) = lc.inflight.remove(&ws) {
                self.abort_inflight(prev);
            }
            let ctx = self.ctx.clone();
            let task = self
                .rt()
                .spawn(async move { explain::run_explain(&ctx, &sql).await });
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

        let joined = task.await;

        let mut lc = self.lifecycle.lock().unwrap();
        // By dispatch, not by tag — a repeat dispatch of the same tag owns the entry now
        // (see `query`). An explain materializes nothing, so there is no snapshot to
        // settle either way.
        if lc.inflight.get(&ws).map(|f| f.dispatch) == Some(dispatch) {
            lc.inflight.remove(&ws);
        }
        self.publish_inflight(&lc);
        match joined {
            Ok(res) => res,
            Err(join) if join.is_cancelled() => Err(CANCELLED.into()),
            Err(join) => Err(format!("explain task failed: {join}")),
        }
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
        self.rt()
            .spawn(async move { catalog::register_external(&ctx, &spec).await })
            .await
            .map_err(|e| format!("register task failed: {e}"))?
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

/// One in-flight export's bookkeeping — the count the close confirm reads, and a hold on the
/// snapshot being written — released together by `Drop`.
///
/// A guard rather than a matching pair of statements because [`Engine::export`] awaits, and a
/// dropped future must not be able to leak either. Borrows the engine, so it cannot outlive the
/// call; [`SnapshotPin`] is the owned variant, for a holder that outlives one method.
struct ExportGuard<'a> {
    engine: &'a Engine,
    snapshot: SnapshotId,
}

impl<'a> ExportGuard<'a> {
    /// Claim both halves. Constructing the guard *is* the acquire, so there is no way to hold
    /// one without having taken what it releases.
    fn new(engine: &'a Engine, snapshot: SnapshotId) -> Self {
        engine.acquire_pin(snapshot);
        {
            let mut lc = engine.lifecycle.lock().unwrap();
            lc.exports += 1;
            engine.publish_inflight(&lc);
        }
        Self { engine, snapshot }
    }
}

impl Drop for ExportGuard<'_> {
    fn drop(&mut self) {
        {
            let mut lc = self.engine.lifecycle.lock().unwrap();
            lc.exports = lc.exports.saturating_sub(1);
            self.engine.publish_inflight(&lc);
        }
        // Outside the lock above: `release_pin` takes it itself, and this mutex is not
        // reentrant.
        self.engine.release_pin(self.snapshot);
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
    let mut config = config.with_default_catalog_and_schema(CATALOG, SCHEMA);
    // Source spans on planner errors power the validator's squiggles (P2-18) — owned,
    // like the catalog names, so an override can't silently degrade diagnostics.
    config.options_mut().sql_parser.collect_spans = true;
    let mut ctx = match build_runtime(overrides) {
        Ok(Some(rt)) => SessionContext::new_with_config_rt(config, rt),
        Ok(None) => SessionContext::new_with_config(config),
        Err(e) => {
            tracing::warn!("engine runtime config invalid ({e}); using defaults");
            SessionContext::new_with_config(config)
        }
    };
    // The Postgres-style JSON accessors (`json_get`, `->`, `->>`, `?`) over a Utf8 column of JSON
    // text. They belong to the **engine**, not to a table: `json_get('{"a":1}', 'a')` is valid
    // with nothing registered, so this sits beside the catalog naming rather than in `catalog`.
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

/// A `RuntimeEnv` from the `datafusion.runtime.*` overrides, or `None` when none are
/// set (default runtime). Sizes ("2G", "100G") parse via `parse_capacity_limit`; the TTL via
/// [`crate::util::parse_duration`], the same function [`config::value_error`] validates the
/// field with.
///
/// **Every key [`config::ENGINE_KEYS`] catalogues as `runtime.*` is read here.** Four of them
/// were catalogued — named, described, and treated as restart-required by
/// [`is_restart_key`](config::is_restart_key) — but never consumed, which nothing noticed while
/// there was no way to set them. P4-07's properties editor is that way: it offers the key,
/// validates the value, badges it RESTART and rebuilds the engine, and the setting then did
/// nothing, with no error to say so. A catalogue entry is a promise that the key applies, so
/// adding one to `ENGINE_KEYS` means adding it here in the same change.
fn build_runtime(overrides: &BTreeMap<String, String>) -> Result<Option<Arc<RuntimeEnv>>, String> {
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
    // Nothing to configure — a default `RuntimeEnv` rather than a hand-built one that happens to
    // match it.
    if [
        &mem,
        &max_temp,
        &temp_dir,
        &metadata_cache,
        &list_cache,
        &list_ttl,
    ]
    .iter()
    .all(|v| v.is_none())
    {
        return Ok(None);
    }

    let mut b = RuntimeEnvBuilder::new();
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
    b.build_arc().map(Some).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use strata_model::{SourceFormat, StatKey};

    use super::*;

    /// Big enough that the first dispatch is still streaming when the second lands, and
    /// cheap to abort (the spool awaits per batch, so the abort takes effect at once).
    const SLOW: &str = "SELECT count(*) FROM generate_series(1, 50000000)";
    const FAST: &str = "SELECT 1 AS n";
    /// A view body whose **profile** is still counting when a test acts on it: 50M distinct
    /// values, aborted within a few dozen milliseconds, so the scan never accumulates far.
    const SLOW_ROWS: &str = "SELECT * FROM generate_series(1, 50000000)";

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
    /// every `runtime.*` key the catalogue names must reach `RuntimeEnvBuilder`, which shows up
    /// here as "the builder accepts its documented default without erroring".
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
            let one = overrides(&[(entry.key, value)]);
            let built = build_runtime(&one)
                .unwrap_or_else(|e| panic!("{} = {value} was rejected: {e}", entry.key));
            assert!(
                built.is_some(),
                "{} = {value} built a default runtime — the key is catalogued but never read, so \
                 setting it does nothing and says nothing",
                entry.key
            );
        }
    }

    #[test]
    fn a_runtime_ttl_is_read_the_way_the_field_validates_it() {
        // The validator and the parser are one function (`util::parse_duration`), so a field that
        // accepts `2m` cannot be read as two seconds.
        assert!(build_runtime(&overrides(&[(
            "datafusion.runtime.list_files_cache_ttl",
            "2m"
        )]))
        .is_ok_and(|rt| rt.is_some()));
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

        let before = reachable(&legacy.ctx, PROBES).await;
        assert_eq!(
            before,
            ["myview", "dailysales", "daily_sales", "orders", "order"],
            "sanity: the shipped path folded each name to its lowercase spelling"
        );
        assert_eq!(
            reachable(&now.ctx, PROBES).await,
            before,
            "every name must stay reachable under exactly the spellings it always was — a \
             sibling def saying `FROM myview` has no migration if this changes"
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
