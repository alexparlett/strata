//! The DataFusion engine — a **direct-call async facade** over a runtime it owns.
//!
//! This crate is the workspace's **one DataFusion boundary**: nothing else names DataFusion
//! (`strata-freya` carries a dev-dependency so a test can build a fixture, and that is all). It
//! sits on [`strata_arrow`], whose Arrow-level vocabulary it hands back — [`RecordBatch`],
//! [`column_info`](strata_arrow::column_info), the [`plan`](strata_arrow::plan) model an EXPLAIN
//! is read into — and on `strata-core`, whose services it reads: `util`, `project` and `secret`.
//! Neither points back.
//!
//! Beside the facade live [`sql`] (the language service: validation, completion, symbols),
//! [`profile`] (a catalog entry's column statistics) and [`register`] (the project registration
//! pass: connect, register tables, create views).
//!
//! [`Engine`] holds the `SessionContext` plus a private multi-thread Tokio runtime: every call
//! spawns its work onto that runtime and awaits the `JoinHandle`, which is executor-agnostic — so
//! Freya's non-Tokio UI executor awaits engine calls directly while DataFusion's own parallelism
//! runs on the engine's threads and never on the render thread.
//!
//! **Pagination is bounded memory.** Each query executes **once** and its full result spools to an
//! immutable on-disk **Arrow IPC snapshot** keyed by [`SnapshotId`] (`docs/SNAPSHOT_SPEC.md`); every
//! page is a bounded `LIMIT/OFFSET` read of it, so RAM holds one page and no query is recomputed.
//! The engine owns the snapshot **lifecycle** too: a re-run for the same workspace retires the
//! previous snapshot at dispatch, cancel and cleanup retire partials, and dropping the engine
//! clears its whole snapshot directory.
//!
//! Profiling ([`Catalog::profile`]) is the third thing tracked beside runs and snapshots: one full
//! scan per catalog entry, keyed by the entry rather than by a workspace, because a profile is a
//! property of the *data* and not of any tab.
//!
//! The underlying logic lives in the sibling modules (`query`, `explain`, `catalog`, `export`,
//! `profile`) as plain async functions over `&SessionContext`.

pub mod arrow_stats;
#[cfg(test)]
mod boundaries;
pub mod builder;
mod catalog;
mod chart;
/// The all-or-nothing contract a connection registers under, shared by [`store`] and [`sources`].
mod connect;
mod explain;
pub mod export;
mod facade;
pub mod formats;
mod functions;
mod generation;
pub mod json_poly;
pub mod policy;
pub mod profile;
mod providers;
mod query;
pub mod register;
pub mod secrets;
mod sink;
pub mod sources;
pub mod sql;
pub mod statements;
mod store;
pub mod udf_package;
pub mod udfs;

pub use catalog::{TableMeta, TableSpec, ViewMeta};
pub use facade::{Catalog, Lang, SnapshotReads, Sources, Work, Workspace};
pub use generation::CatalogGen;
pub use policy::{
    Admit, Capability, CapabilityPolicyProvider, DenyCode, Grant, GrantFamily, Grants, Locality,
    PolicyProvider, Principal, RemoteScope, RemoteSel, TargetFacts,
};
pub use query::{purge_snapshot_root, ReadPolicy};
pub use sources::source::{
    ConnectionKey, DataSource, Field, Located, SourceCatalog, SourceInfo, SourceKind, SourceMode,
    Sourced,
};
pub use sources::RemoteRelation;
pub use statements::arms::{drop_intent, duplicate_column, SessionScope};
pub use statements::{
    Fault, Form, Mechanism, PolicyRefusal, Reason, Remote, StatementReport, StmtKind, StoreEffect,
    Target,
};

pub use builder::EngineBuilder;
pub use udf_package::UdfPackage;

use secrets::SecretProvider;

use strata_arrow::{config, RecordBatch};

/// A call the caller (or the app on their behalf) **stopped**: [`Workspace::cancel`] aborted it, or
/// [`Catalog::cancel_profile`] did.
pub const CANCELLED: &str = "cancelled";
/// A run that finished but was no longer the latest dispatch for its workspace — a newer press
/// replaced it, so its result is discarded and its snapshot retired ([`Workspace::query`]).
pub const SUPERSEDED_RUN: &str = "superseded by a newer run";
/// The scan equivalent: a re-scan replaced this one ([`Catalog::profile`]).
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
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use datafusion::common::TableReference;
use datafusion::execution::memory_pool::MemoryPool;
use datafusion::execution::runtime_env::RuntimeEnv;
use datafusion::execution::{FunctionRegistry, SessionState, SessionStateBuilder};
use datafusion::logical_expr::ScalarUDF;
use datafusion::prelude::*;
use datafusion::sql::parser::Statement as DFStatement;
use datafusion_federation::FederatedQueryPlanner;
use tokio::runtime::Runtime;
use tokio::task::AbortHandle;

use formats::Formats;
use functions::Functions;
use generation::GenClock;
use providers::{StrataCatalogList, StrataCatalogProvider};
use query::{discard_snapshot_dir, retire_snapshot, run_and_snapshot, CellFormat};
use sources::source::Sources as SourceRegistry;
use sources::Live;
use statements::arms::StrataFunctionFactory;
use strata_model::{ConnectionDef, QueryOutput, SnapshotId, TabId};

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
/// (`QuerySpec::run`), passed down so [`Workspace::cancel`] can tell "still this run" from
/// "a newer run replaced it" without a parallel request-id scheme.
///
/// It is the *caller's* nonce, so it is not unique here: the same tag can legitimately be
/// dispatched twice (freya-query re-runs an entry when a subscriber remounts while it is
/// still in flight). Engine-side lifecycle therefore keys on `InFlight::dispatch`, not on
/// this — see [`Workspace::query`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RunTag(pub u128);

/// What a **Run** settled to ([`Workspace::run`]) — the two things a press can produce.
///
/// The split is the router's, not a mode the caller picks: a Run is one press, and whether it
/// produces rows or performs a statement is a property of what was typed.
pub enum RunOutcome {
    /// Exactly [`Workspace::query`]'s answer — the snapshot handle + page 1. Byte-for-byte the
    /// path that shipped: same supersede, same retire-on-dispatch, same pins.
    Rows(QueryOutput, RecordBatch),
    /// An intercepted statement's report. **No snapshot**, and none retired: a tab that
    /// creates a table can still page the result it already had
    /// (`docs/SNAPSHOT_SPEC.md` §4 — DDL does not retire snapshots).
    Statement(StatementReport),
}

/// An in-flight **profile scan**: which dispatch it is, and the handle that cancels it.
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
    /// The caller's nonce, kept for exactly one thing: [`Workspace::cancel`]'s guard.
    tag: RunTag,
    snapshot: Option<SnapshotId>,
    abort: AbortHandle,
    start: Instant,
}

/// A statement being **classified** — the window in front of dispatch.
///
/// Registered *without* superseding, which is the whole reason it is a second map rather than an
/// `InFlight` with no snapshot: everywhere else registration **is** supersede
/// ([`bookkeep`](Engine::bookkeep) aborts what the workspace was running before it registers), and
/// a Run press must not destroy a scan that is minutes in before anyone knows the typed statement
/// is even valid. So a classification is visible to [`Workspace::cancel`],
/// [`Workspace::is_running`] and the close-while-running flag, and invisible to supersede.
///
/// It matters because classification can **await**: an embedder's [`PolicyProvider`] may be a
/// service, and without this the whole of its round trip is a stretch in which a tab looks idle
/// and Cancel does nothing.
struct Classifying {
    /// Engine-unique, monotonic — the same "am I still the latest?" identity [`InFlight`] uses.
    /// A second `run` on the same workspace replaces the entry rather than aborting it, so the
    /// first one's settle path has to know the entry is no longer its own.
    dispatch: u64,
    tag: RunTag,
    abort: AbortHandle,
    start: Instant,
}

/// Undoes a dispatch whose caller went away before it could settle.
///
/// A dispatch publishes its [`InFlight`] entry *before* awaiting the spawned work, so until
/// the settle path runs the workspace looks busy. That was safe while every caller was
/// freya-query, which by design never cancels an execution — but an agent's run is
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
        let Ok(mut lc) = self.engine.lifecycle.lock() else {
            return;
        };
        if lc.inflight.get(&self.ws).map(|f| f.dispatch) != Some(self.dispatch) {
            return;
        }
        if let Some(f) = lc.inflight.remove(&self.ws) {
            self.engine.abort_inflight(f);
        }
        self.engine.publish_inflight(&lc);
    }
}

/// Removes a classification whose caller went away before it settled — [`DispatchGuard`]'s job
/// for the window in front of dispatch, and for the same reason: a latched entry keeps the
/// in-flight flag on for the engine's life, so every later close asks about a statement nobody
/// is classifying any more.
struct ClassifyGuard<'a> {
    engine: &'a Engine,
    ws: WsId,
    dispatch: u64,
    armed: bool,
}

impl<'a> ClassifyGuard<'a> {
    fn arm(engine: &'a Engine, ws: WsId, dispatch: u64) -> Self {
        Self {
            engine,
            ws,
            dispatch,
            armed: true,
        }
    }

    /// The classification settled on its own terms; leave the entry to `classify_bracket`.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ClassifyGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(mut lc) = self.engine.lifecycle.lock() else {
            return;
        };
        if lc.classifying.get(&self.ws).map(|c| c.dispatch) != Some(self.dispatch) {
            return;
        }
        if let Some(c) = lc.classifying.remove(&self.ws) {
            c.abort.abort();
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
    /// Statements being classified — see [`Classifying`]. Read by `cancel`, `is_running` and
    /// `publish_inflight` beside `inflight`, and by supersede by nobody.
    classifying: HashMap<WsId, Classifying>,
    current: HashMap<WsId, SnapshotId>,
    /// In-flight profile scans by entry identity ([`fold_ident`] of the name — tables and
    /// views share one namespace).
    profiles: HashMap<String, ProfileRun>,
    /// How many pieces of **background** work are in flight — an export writing a file, a
    /// drop deleting a table's data. A **count, not a map**: nothing addresses one of
    /// these — no cancel, no supersede, no per-item state to look up. All it has to do is keep
    /// [`publish_inflight`](Engine::publish_inflight) true while something is half-done, so the
    /// close-while-running confirm asks before the window takes the runtime away.
    ///
    /// Not per-kind, because the question every reader asks is the same one: is anything the
    /// user would rather finish still going? A second counter would be a second answer to it.
    background: usize,
    /// Snapshots a caller is **holding open**, and how many holds each has
    /// ([`SnapshotReads::pin`]). A pinned snapshot survives its workspace re-running.
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
///
/// # The surface
///
/// Every public call is reached through one of six borrowed **group handles**, each named for
/// what its calls are about and carrying that identity:
///
/// | Handle | Calls about |
/// |---|---|
/// | [`ws(id)`](Self::ws) | one workspace's runs |
/// | [`snapshot(id)`](Self::snapshot) | one immutable result |
/// | [`catalog()`](Self::catalog) | the tables and views this engine holds |
/// | [`sources()`](Self::sources) | the connections behind them |
/// | [`lang()`](Self::lang) | what a buffer means to this session |
/// | [`work()`](Self::work) | what is in flight |
///
/// Beside them is the short root set — [`builder`](Self::builder), [`id`](Self::id),
/// [`set_data_dir`](Self::set_data_dir) and the config trio — for the calls that are about the
/// engine itself. The mapping is total: nothing public sits outside it.
///
/// ```no_run
/// # use strata_engine::{Engine, RunTag, WsId};
/// # async fn read(engine: &Engine, ws: WsId, tag: RunTag) -> Result<(), String> {
/// let (output, _page_1) = engine.ws(ws).query(tag, "SELECT 1".into(), 100).await?;
/// let snapshot = output.snapshot.expect("a query settles a snapshot");
/// let (_rows, _batch) = engine.snapshot(snapshot).page(2, 100, None).await?;
/// # Ok(())
/// # }
/// ```
///
/// # The embedder kernel
///
/// An embedder with no window of its own — a CLI, a server, a test harness — needs five things,
/// and everything else on this facade is a surface built for one:
///
/// - [`Engine::builder`], which is where every decision an embedder may make is made;
/// - [`Catalog::sync`] over its own defs ([`CatalogSpec::of_project`](register::CatalogSpec::of_project)
///   builds the spec), or [`Catalog::register`] / [`Catalog::create_view`] one at a time;
/// - [`Workspace::run`] (or [`Workspace::explain`]) to perform a statement;
/// - [`SnapshotReads::page`], [`SnapshotReads::export_to`] and [`SnapshotReads::live`] to read
///   what one settled;
/// - a [`PolicyProvider`](policy::PolicyProvider) — [`EngineBuilder::with_policy`] — to say what
///   a caller may perform, which is the only thing standing between "this is my process" and
///   "this is somebody's request".
///
/// The headless MCP host (`strata-agent`) is exactly this kernel and nothing else, which is what
/// keeps it honest as the second deployment.
pub struct Engine {
    engine_id: u64,
    /// This engine's own handle, from [`Arc::new_cyclic`]. It is what lets a method handing out an
    /// owning guard still take `&self`, and so still be reachable through a `Deref`. Weak, because
    /// a strong handle here would be a cycle.
    self_ref: Weak<Engine>,
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
    /// "This engine has work in flight", published on every lifecycle mutation for readers
    /// that can reach neither the lock nor async code — the window's winit close hook (T2),
    /// which runs outside the UI and must be `Send`.
    ///
    /// **Owned from birth**, and handed out by [`Work::flag`] rather than installed by a
    /// caller: an install had an ordering to get right (a flag arriving after the first run
    /// started published nothing about it) and a second-install arm that could only ever be a
    /// bug reported to a log. An engine nobody asks publishes into this all the same, which is
    /// what makes the answer true the instant somebody does.
    inflight_flag: Arc<AtomicBool>,
    /// The registered SQL functions (built-ins + UDFs) for the language service,
    /// walked at build and re-walked by a statement that moves the registry.
    functions: Functions,
    /// The project folder this engine may write internal tables into, set at project
    /// open by whichever host owns it — see [`set_data_dir`](Engine::set_data_dir), or
    /// [`with_data_dir`](EngineBuilder::with_data_dir), which says it at construction. `None`
    /// until then, and forever for an engine with no project behind it.
    data_root: Mutex<Option<PathBuf>>,
    /// Which registered tables are **internal** — see [`InternalTables`].
    internal: InternalTables,
    /// What each registered name reads — see [`Dependencies`].
    dependencies: Dependencies,
    /// Which connections this engine has been told about — see [`Connections`].
    connections: Connections,
    /// What generation of the catalog this engine is at — see [`CatalogGen`].
    generation: GenClock,
    /// The source connections that are **live**: their handles and the catalogs they registered
    /// — see [`Live`]. A field on the engine rather than something a task holds, because a pool
    /// owns its driver tasks and the engine's `Drop` has to be what ends them.
    live: Live,
    /// Which data sources this engine can serve a connection with
    /// ([`EngineBuilder::with_source`]) — a def's kind is looked up here, and a kind nothing
    /// answers to is a failed row naming the fix.
    sources: SourceRegistry,
    /// Which file formats this engine can read ([`EngineBuilder::with_format`]) — a def's format
    /// is looked up here, and a format nothing answers to is a failed row naming the fix.
    formats: Formats,
    /// The `SET` overlay and the prepared-statement mirror — see [`SessionScope`].
    /// Default on a fresh engine, which is what makes a restart clear the session.
    session: SessionScope,
    /// Where a secret this engine needs comes from ([`EngineBuilder::with_secrets`]).
    secrets: Arc<dyn SecretProvider>,
    /// Who may perform what.
    ///
    /// Asked by the three entries that classify a statement: [`run`](Workspace::run),
    /// [`validate`](Lang::validate) and [`policy_verdicts`](Lang::policy_verdicts). The read
    /// entries take a statement rather than a caller and are limited to reading by the read path's
    /// own `SQLOptions` instead.
    policy: Arc<dyn PolicyProvider>,
}

/// The engine-side set of tables whose data Strata owns — [`fold_ident`]ed names.
///
/// Derived state, rebuilt by the same registration pass that builds everything else, and
/// deliberately **not a second catalog**: it holds names and nothing else, and answers exactly
/// one engine-side question — may a write statement target this provider. Everything
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
            false => set.remove(&fold_ident(name)),
        };
    }
}

/// What each registered name reads: a table's connection, or a view's scans.
///
/// The [`InternalTables`] shape, with the same limits, and it answers one question —
/// [`Sources::dependents`]. It is not a second catalog: what a host's row says about a name is
/// the host's, and none of it is here.
///
/// Registration is a reconciliation, so this is too. Every funnel that registers a name notes
/// what it reads and every funnel that takes one out forgets it, and [`sync`](register::sync)
/// prunes to the names its `CatalogSpec` holds. That last step is what keeps a table whose
/// registration **failed** answerable — it is noted from the spec, and no deregistration will
/// ever report it — without its entry outliving the def.
///
/// Bounded by what the last pass established: a def no pass has reached is not here, and a view
/// the engine could not create has no scans to record.
#[derive(Clone, Debug, Default)]
pub struct Dependencies(Arc<Mutex<BTreeMap<String, Scanned>>>);

/// What a connection is holding up — [`Sources::dependents`]'s answer.
///
/// Two lists, because a caller counting them counts two different things. Both are alphabetical
/// and name each thing once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dependents {
    /// Workspace tables whose def reads its files through this connection. Always empty for a
    /// connection that registers a **catalog**: no def can name one.
    pub tables: Vec<String>,
    /// The views left invalid — those over [`tables`](Self::tables) for an object store, and
    /// those scanning its catalog for a source.
    pub views: Vec<String>,
}

/// One registered name, in its own spelling, and what it reads.
///
/// The spelling is carried because the map is keyed by [`fold_ident`], names being matched the way
/// SQL matches them, while a caller renders the name as it was written.
#[derive(Clone, Debug)]
struct Scanned {
    name: String,
    scans: Scans,
}

/// What one name reads. Two arms and no third: a saved query registers nothing.
#[derive(Clone, Debug)]
enum Scans {
    /// A table, and the connection its files are read through — `None` over local files.
    Table(Option<String>),
    /// A view, and the two lists [`ViewMeta`] records: workspace scans bare, everything else
    /// qualified whole.
    View {
        tables: Vec<String>,
        remote: Vec<String>,
    },
}

impl Dependencies {
    /// The tables read through the connection called `name`, alphabetically.
    ///
    /// Case-insensitive, because a connection's name is a SQL identifier and
    /// [`Connections::resolve`] answers that way — which is also what decides, one level down,
    /// whether the table registered over that store at all.
    fn over(&self, name: &str) -> Vec<String> {
        self.named(|scans| match scans {
            Scans::Table(Some(held)) => held.eq_ignore_ascii_case(name),
            _ => false,
        })
    }

    /// The views scanning any of `tables`, alphabetically and each named once.
    ///
    /// Flat rather than transitive on purpose, and still complete: DataFusion inlines a view it
    /// reads, so a view over a view records the *base* tables of both.
    fn above(&self, tables: &[String]) -> Vec<String> {
        let wanted: BTreeSet<String> = tables.iter().map(|t| fold_ident(t)).collect();
        self.named(|scans| match scans {
            Scans::View { tables, .. } => tables.iter().any(|t| wanted.contains(&fold_ident(t))),
            Scans::Table(_) => false,
        })
    }

    /// The views scanning through the catalog `catalog`, alphabetically and each named once.
    ///
    /// Matched on the qualified name's **first part**, folded: that part is the catalog, which is
    /// what [`ViewMeta`] keeps its two lists apart for.
    fn reading(&self, catalog: &str) -> Vec<String> {
        let wanted = fold_ident(catalog);
        self.named(|scans| match scans {
            Scans::View { remote, .. } => remote
                .iter()
                .filter_map(|dep| dep.split('.').next())
                .any(|part| fold_ident(part) == wanted),
            Scans::Table(_) => false,
        })
    }

    /// Every held name whose scans `wanted` accepts, in its own spelling, alphabetically.
    fn named(&self, wanted: impl Fn(&Scans) -> bool) -> Vec<String> {
        let held = self.0.lock().unwrap();
        let mut found: Vec<String> = held
            .values()
            .filter(|held| wanted(&held.scans))
            .map(|held| held.name.clone())
            .collect();
        found.sort();
        found
    }

    /// Record what registering `name` established about what it reads.
    fn note(&self, name: &str, scans: Scans) {
        self.0.lock().unwrap().insert(
            fold_ident(name),
            Scanned {
                name: name.to_string(),
                scans,
            },
        );
    }

    /// Forget `name` — every funnel that deregisters one.
    fn forget(&self, name: &str) {
        self.0.lock().unwrap().remove(&fold_ident(name));
    }

    /// Keep only the names `wanted` holds — [`sync`](register::sync)'s reconciliation, and the
    /// only thing that can retire an entry no deregistration will ever report.
    pub(crate) fn retain(&self, wanted: &BTreeSet<String>) {
        self.0
            .lock()
            .unwrap()
            .retain(|held, _| wanted.contains(held));
    }
}

/// The connections this engine has been told about: the last def handed to
/// [`Sources::connect`] for each name, keyed by that name.
///
/// It answers two questions from the one map — may a typed `CREATE EXTERNAL TABLE` name this
/// bucket, and what does this engine hold a connection for ([`Sources::listing`]). The def rather
/// than the identity alone is what makes the second answerable without asking the host: an engine
/// told about a connection can say what kind serves it and what it registers, live or not.
///
/// It is not a second copy of the catalog. What a host's row says about a connection — whether it
/// is waiting, the sentence a failure left — is the host's, and nothing here records it.
///
/// **Membership, not connectivity.** [`Sources::connect`] notes the def whether what it describes
/// went in or not, because a connection that cannot resolve a credential today is still a
/// connection this project has: the def a statement writes is durable and the fix (`aws sso
/// login`, a region typed into the editor, ↻) happens afterwards. Asking DataFusion's object-store
/// registry instead would have answered *no* for exactly those, in a sentence — "not a connection
/// in this project" — that would then be false. (What the *session* holds right now is a different
/// question, and [`Sources::listing`] answers it as `live`.)
///
/// Rebuilt by the pass, like the origin set: the registration pass's first phase calls `connect` for
/// every def, and [`Sources::disconnect`] — the Forget gesture and the edit that moves a
/// connection's identity — is the one removal.
#[derive(Clone, Debug, Default)]
pub struct Connections(Arc<Mutex<BTreeMap<String, ConnectionDef>>>);

impl Connections {
    /// The connection `name` addresses, **in the connection's own spelling** — `None` when this
    /// project has none.
    ///
    /// Answering with the stored string rather than a bool is what keeps a def's `connection`
    /// field equal to the name everything else addresses it by: the store's picker, the table
    /// spec's path composition and the Forget confirm all match on that exact string.
    ///
    /// The fallback compares **case-insensitively**, because a connection's name is a SQL
    /// identifier and queries fold one. The exact hit is tried first so the ordinary case costs
    /// one lookup.
    pub fn resolve(&self, name: &str) -> Option<String> {
        let held = self.0.lock().unwrap();
        if held.contains_key(name) {
            return Some(name.to_string());
        }
        held.keys()
            .find(|held| held.eq_ignore_ascii_case(name))
            .cloned()
    }

    /// What the connection called `name` **is** — the `(kind, address)` pair, for the one thing
    /// that still needs it: composing the URL its object store is registered under.
    fn identity(&self, name: &str) -> Option<String> {
        self.def(name).map(|def| def.identity())
    }

    /// The def this engine was last handed for the connection called `name`, matched the way
    /// [`resolve`](Self::resolve) matches.
    fn def(&self, name: &str) -> Option<ConnectionDef> {
        let held = self.0.lock().unwrap();
        held.get(name).cloned().or_else(|| {
            held.iter()
                .find(|(held, _)| held.eq_ignore_ascii_case(name))
                .map(|(_, def)| def.clone())
        })
    }

    /// Every connection this engine has been told about, in name order — what
    /// [`Sources::listing`] walks.
    ///
    /// **Membership, not liveness**, exactly as the rest of this type is: a connection whose
    /// credentials this machine cannot resolve today is still one the project has, and the
    /// listing says so by answering `live: false` rather than by leaving it out.
    fn all(&self) -> Vec<ConnectionDef> {
        self.0.lock().unwrap().values().cloned().collect()
    }

    /// The connection whose `(kind, address)` is `identity` — for the one caller that arrives
    /// with a written location rather than with a name: a typed `CREATE EXTERNAL TABLE … LOCATION
    /// 's3://acme-lake/events/'`, which has to be matched against what the project holds.
    pub fn named(&self, identity: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|(_, held)| held.identity().eq_ignore_ascii_case(identity))
            .map(|(name, _)| name.clone())
    }

    /// The connections a set of defs describes, for a caller that holds defs rather than a live
    /// engine.
    ///
    /// The registration pass composes its table specs **before** its first phase registers
    /// anything, so at that moment no engine can answer what a table's connection is; the defs in
    /// hand are the only thing that can. Building the same type from them rather than reading the
    /// defs directly is what keeps one lookup rule — including the case-insensitive fallback,
    /// which a hand-rolled `find` over the defs would quietly drop.
    pub fn of(defs: &[ConnectionDef]) -> Self {
        let held = Self::default();
        for def in defs {
            held.note(def);
        }
        held
    }

    /// Every connection this engine has been told about, as `(name, identity)` — what
    /// [`sync`](crate::register::sync) diffs a desired set against.
    ///
    /// Both halves, because a def whose bucket or provider was edited keeps its name and changes
    /// the URL its object store went in under: a diff by name alone leaves that URL registered
    /// with nothing addressing it.
    pub(crate) fn held(&self) -> Vec<(String, String)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(name, def)| (name.clone(), def.identity()))
            .collect()
    }

    fn note(&self, def: &ConnectionDef) {
        self.0.lock().unwrap().insert(def.named(), def.clone());
    }

    fn forget(&self, name: &str) {
        self.0.lock().unwrap().remove(name);
    }
}

impl Engine {
    /// Creates an [`EngineBuilder`] to configure an `Engine`.
    ///
    /// This is the same as [`EngineBuilder::new`].
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    /// This engine's own `Arc`, for a guard that has to keep it reachable past the call that made
    /// it. Upgrading cannot fail: reaching a method at all means the engine is alive.
    fn owned(&self) -> Arc<Engine> {
        self.self_ref.upgrade().expect("the engine's own handle")
    }

    /// Tell this engine which **project folder** it belongs to.
    ///
    /// `root` is the project folder, not `.strata/tables`, because a statement that creates an
    /// internal table needs both: the absolute directory to spool into, and the project-relative
    /// path the def stores, which is what lets the def travel with `project.json`. The layout
    /// between them is [`project::tables_dir`](strata_core::project::tables_dir)'s, in one place.
    ///
    /// Every host that opens a project calls this — the app window and the headless server both.
    /// An engine that is never told refuses to *create* a table (politely, naming the reason) and
    /// is otherwise unaffected: a project's existing internal defs replay through the ordinary
    /// registration pass, whose source paths were already resolved against the root by the
    /// caller.
    pub fn set_data_dir(&self, root: &Path) {
        *self.data_root.lock().unwrap() = Some(root.to_path_buf());
    }

    /// Record what a registration settled about a table's origin. Called from every path that
    /// registers one, so the set is rebuilt by the pass rather than maintained beside it.
    fn note_origin(&self, name: &str, internal: bool) {
        self.internal.note(name, internal);
    }

    /// Record what a registration established about what `name` reads, or forget it — called from
    /// every path that registers or takes out a table or a view, so the map is rebuilt by the
    /// pass rather than maintained beside it.
    fn note_scans(&self, name: &str, scans: Option<Scans>) {
        match scans {
            Some(scans) => self.dependencies.note(name, scans),
            None => self.dependencies.forget(name),
        }
    }

    /// Publish "this engine has work in flight". Called from **every**
    /// mutation of `Lifecycle::inflight` / `Lifecycle::profiles`, with the lock held, so a
    /// reader can never see a flag that disagrees with the maps.
    ///
    /// A **profile counts**: a scan is the most expensive thing the app does, and closing
    /// the window would throw it away — exactly what the confirm exists to ask about. The
    /// per-tab probe below deliberately does not, because a profile is not a tab's work.
    ///
    /// An **export counts** for a stronger reason than either: closing mid-write doesn't lose
    /// work, it leaves a truncated file (or a half-built Hive tree) on the user's disk under
    /// the name they chose. Like a profile, it is nobody's tab, so the per-tab probe ignores it.
    fn publish_inflight(&self, lc: &Lifecycle) {
        self.inflight_flag.store(
            !lc.inflight.is_empty()
                || !lc.classifying.is_empty()
                || !lc.profiles.is_empty()
                || lc.background > 0,
            Ordering::Relaxed,
        );
    }

    /// This engine's process-unique id — what makes a [`SnapshotId`] meaningful.
    ///
    /// Snapshot ids are a **per-engine** counter, so a restart mints `1` again: anything that
    /// remembers a snapshot across a possible rebuild has to remember which engine minted it,
    /// or it will read a *different* result that happens to share the number.
    pub fn id(&self) -> u64 {
        self.engine_id
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
    ///
    /// A key the **session overlay** holds is recorded and not applied either: a typed
    /// `SET` wins for its key until `RESET` or restart, so the new value becomes the baseline a
    /// `RESET` will land on rather than something that quietly overwrites what the user just
    /// typed. That is the whole precedence rule, and it lives here because this is the only place
    /// the two writers meet.
    pub fn set_config(&self, overrides: BTreeMap<String, String>) -> bool {
        let mut current = self.overrides.lock().unwrap();
        if *current != overrides {
            let state = self.ctx.state_ref();
            let mut state = state.write();
            let options = state.config_mut().options_mut();
            let touched: BTreeSet<&String> = current.keys().chain(overrides.keys()).collect();
            for key in touched {
                if config::is_restart_key(key)
                    || config::is_owned_key(key)
                    || self.session.overlaid(key)
                {
                    continue;
                }
                let value = match overrides.get(key) {
                    Some(value) => value.as_str(),
                    None => match config::key_def(key) {
                        Some(def) => def.default,
                        None => continue,
                    },
                };
                if let Err(e) = options.set(key, value) {
                    tracing::warn!("engine config: skipping {key}={value}: {e}");
                }
            }
            refresh_config_dependent_udfs(&mut state);
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

    /// The file formats this engine was built with, in registration order.
    ///
    /// About the engine itself rather than about its catalog: a format is something an embedder
    /// said at construction ([`EngineBuilder::with_format`]), and this is the one read every
    /// surface that offers a format shares — the `STORED AS` completion, the export format list,
    /// and the agent's export.
    pub fn formats(&self) -> Vec<formats::FormatInfo> {
        self.formats.registrants()
    }

    /// The `datafusion.*` overrides this engine is running with.
    pub fn overrides(&self) -> BTreeMap<String, String> {
        self.overrides.lock().unwrap().clone()
    }

    /// The engine's runtime (always present while the engine lives — see the field).
    fn rt(&self) -> &Runtime {
        self.rt.as_ref().expect("engine runtime")
    }

    /// What the **engine** has to learn from a statement's [`StoreEffect`], applied wherever one
    /// is produced — [`run`](Workspace::run)'s interception and [`drop_table`](Catalog::drop_table)'s
    /// direct call both.
    ///
    /// The engine learns from the returned value, exactly as the store does: an arm that
    /// registered a table says so in its effect, and that is where the origin comes from — never
    /// by asking DataFusion, which does not know. Held once rather than at each producer, so the
    /// catalog-surface drop and the typed one cannot leave the engine in two different states.
    /// Exhaustive on [`StoreEffect`] with no wildcard, for the reason [`statements::arms::execute`] is
    /// exhaustive on `StmtKind`: an effect a later task adds must be a compile error here rather
    /// than something the engine silently declines to learn from.
    ///
    /// Where a statement moves the [`CatalogGen`], on every arm but
    /// [`RescanTable`](StoreEffect::RescanTable): re-reading a row's counts cannot change what
    /// any name resolves to, since the sink schema-checks before it writes.
    fn settle_effect(&self, effect: Option<&StoreEffect>) {
        let Some(effect) = effect else { return };
        match effect {
            StoreEffect::TableUpserted { def, .. } => {
                self.note_origin(&def.name, def.origin.is_internal());
                self.note_scans(&def.name, Some(Scans::Table(def.connection.clone())));
                self.generation.bump();
            }
            StoreEffect::TableRemoved { name, .. } => {
                self.catalog().cancel_profile(name);
                self.note_origin(name, false);
                self.note_scans(name, None);
                self.generation.bump();
            }
            StoreEffect::ViewUpserted { def, meta } => {
                self.catalog().cancel_profile(&def.name);
                self.note_scans(
                    &def.name,
                    Some(Scans::View {
                        tables: meta.tables.clone(),
                        remote: meta.remote.clone(),
                    }),
                );
                self.generation.bump();
            }
            StoreEffect::ViewRemoved { name } => {
                self.catalog().cancel_profile(name);
                self.note_scans(name, None);
                self.generation.bump();
            }
            StoreEffect::FunctionsChanged
            | StoreEffect::PreparedChanged
            | StoreEffect::RemoteRelationsChanged => {
                self.generation.bump();
            }
            StoreEffect::RescanTable { .. } => {}
        }
    }

    /// Bracket `work` as workspace `ws`'s **classification** — the window in front of dispatch,
    /// where a statement is being parsed, qualified and put to the policy provider.
    ///
    /// Registers so [`cancel`](Workspace::cancel), [`is_running`](Workspace::is_running) and the
    /// close-while-running flag can all see it, and **deliberately does not supersede** — see
    /// [`Classifying`], and `a_refused_statement_leaves_the_workspaces_run_alone`, which pins
    /// exactly that. Nothing is registered, planned or spooled in this window, so there is no
    /// snapshot to retire and nothing to abort but the classification itself.
    async fn classify_bracket<F, T>(&self, ws: WsId, tag: RunTag, work: F) -> Result<T, String>
    where
        F: Future<Output = Result<T, String>> + Send + 'static,
        T: Send + 'static,
    {
        let dispatch = self.dispatch_seq.fetch_add(1, Ordering::Relaxed);
        let task = {
            let mut lc = self.lifecycle.lock().unwrap();
            let task = self.rt().spawn(work);
            lc.classifying.insert(
                ws,
                Classifying {
                    dispatch,
                    tag,
                    abort: task.abort_handle(),
                    start: Instant::now(),
                },
            );
            self.publish_inflight(&lc);
            task
        };

        let mut guard = ClassifyGuard::arm(self, ws, dispatch);
        let joined = task.await;
        guard.disarm();

        let mut lc = self.lifecycle.lock().unwrap();
        if lc.classifying.get(&ws).map(|c| c.dispatch) == Some(dispatch) {
            lc.classifying.remove(&ws);
        }
        self.publish_inflight(&lc);
        drop(lc);
        match joined {
            Ok(res) => res,
            Err(join) if join.is_cancelled() => Err(CANCELLED.into()),
            Err(join) => Err(format!("policy task failed: {join}")),
        }
    }

    /// Bracket `work` as workspace `ws`'s in-flight call — the lifecycle every dispatch that
    /// materializes **nothing** shares: [`explain`](Workspace::explain), and every intercepted
    /// statement.
    ///
    /// Supersedes whatever `ws` was running (a tab runs one thing at a time, exactly as a
    /// re-press does), registers the abort handle so [`cancel`](Workspace::cancel),
    /// [`is_running`](Workspace::is_running) and the close-while-running flag can all see it, and
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
            Err(join) if join.is_cancelled() => Err(CANCELLED.into()),
            Err(join) => Err(format!("{what} task failed: {join}")),
        }
    }

    /// `sql` as one parsed statement with its bare names resolved.
    ///
    /// The entry every read arriving as text goes through; [`run`](Workspace::run) does not, its
    /// classification having already produced the statement.
    ///
    /// **Not spawned onto the runtime**, unlike every call that touches the context to *do*
    /// something: it has to land before the first await, or `query` stops publishing its in-flight
    /// entry on the first poll and `DispatchGuard` has nothing to retract.
    fn parse_one(&self, sql: &str) -> Result<DFStatement, String> {
        statements::pipeline::resolved_one(&self.ctx, sql)
    }

    /// [`query`](Workspace::query)'s body, plus the [`ReadPolicy`] the statement is planned under.
    ///
    /// Private, and `query` is the read-only entry every other caller keeps: the widening is only
    /// ever sound for a statement [`sql::read_policy`] judged, so the ability to ask for it does
    /// not belong on the facade (spec §1). One body either way — the lifecycle is identical, and a
    /// second copy of it is what the whole snapshot discipline exists to avoid.
    async fn read(
        &self,
        ws: WsId,
        tag: RunTag,
        stmt: DFStatement,
        page_size: usize,
        policy: ReadPolicy,
    ) -> Result<(QueryOutput, RecordBatch), String> {
        let snapshot = SnapshotId(self.snap_seq.fetch_add(1, Ordering::Relaxed));
        let dispatch = self.dispatch_seq.fetch_add(1, Ordering::Relaxed);
        let fmt = CellFormat::new(&self.overrides.lock().unwrap());
        let task = {
            let mut lc = self.lifecycle.lock().unwrap();
            if let Some(prev) = lc.inflight.remove(&ws) {
                self.abort_inflight(prev);
            }
            if let Some(old) = lc.current.remove(&ws) {
                self.retire_or_defer(&mut lc, old);
            }
            let ctx = self.ctx.clone();
            let engine_id = self.engine_id;
            let task = self.rt().spawn(async move {
                run_and_snapshot(&ctx, engine_id, snapshot, stmt, page_size, &fmt, policy).await
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

        let mut guard = DispatchGuard::arm(self, ws, dispatch);
        let joined = task.await;
        guard.disarm();

        let mut lc = self.lifecycle.lock().unwrap();
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
                    retire_snapshot(&self.ctx, self.engine_id, snapshot);
                    Err(SUPERSEDED_RUN.into())
                }
            }
            Ok(Err(e)) => Err(e),
            Err(join) if join.is_cancelled() => {
                retire_snapshot(&self.ctx, self.engine_id, snapshot);
                Err(CANCELLED.into())
            }
            Err(join) => {
                retire_snapshot(&self.ctx, self.engine_id, snapshot);
                Err(format!("query task failed: {join}"))
            }
        }
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
    /// inside [`query`](Workspace::query)'s settle path deliberately do not: they retire a run's
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
/// behind it re-runs. **Owned**, unlike [`BackgroundGuard`], because the thing it protects is the
/// spawned write and not the call that started it — see [`SnapshotReads::export`] for what a
/// caller-scoped guard let happen.
struct ExportHold {
    snapshot: SnapshotId,
    /// **Weak, and that is load-bearing.** This hold rides on a task running on the runtime the
    /// engine owns, so a strong `Arc` here would close a cycle — engine owns runtime owns task
    /// owns hold owns engine — and the engine would never drop. The write does not need it
    /// either: `run_export` holds its own clone of the `SessionContext`. An engine that has gone
    /// has no bookkeeping left to correct, so a failed upgrade is the whole handling.
    engine: Weak<Engine>,
}

impl ExportHold {
    /// Claim both halves. Constructing the hold *is* the acquire, so there is no way to hold one
    /// without having taken what it releases.
    fn new(engine: &Engine, snapshot: SnapshotId) -> Self {
        let mut lc = engine.lifecycle.lock().unwrap();
        *lc.pins.entry(snapshot).or_insert(0) += 1;
        lc.background += 1;
        engine.publish_inflight(&lc);
        drop(lc);
        Self {
            snapshot,
            engine: engine.self_ref.clone(),
        }
    }
}

impl Drop for ExportHold {
    fn drop(&mut self) {
        let Some(engine) = self.engine.upgrade() else {
            return;
        };
        engine.release_pin(self.snapshot);
        let mut lc = engine.lifecycle.lock().unwrap();
        lc.background = lc.background.saturating_sub(1);
        engine.publish_inflight(&lc);
    }
}

/// A hold on one snapshot, keeping it readable past the re-run that would otherwise retire it
/// (see [`SnapshotReads::pin`]). Dropping it releases the hold, and retires the snapshot if
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
        for (_, c) in lc.classifying.drain() {
            c.abort.abort();
        }
        for (_, p) in lc.profiles.drain() {
            p.abort.abort();
        }
        lc.current.clear();
        self.publish_inflight(&lc);
        drop(lc);
        self.live.shutdown(&self.ctx);
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
/// `pub` because the empty-table panel asks the same question of its column rows: two
/// rows collide exactly when the create arm's own fold says they do, and a form approximating
/// that with a case-insensitive compare would refuse pairs the engine accepts.
pub fn fold_ident(name: &str) -> String {
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
/// **Fold-preserving is the contract**, and it is what makes this safe to add to a shipped app.
/// DataFusion lower-cases an unquoted identifier and takes a quoted one verbatim, so a view named
/// `DailySales` has been registering as `dailysales` all along; emitting `"DailySales"` would
/// re-key it and break every sibling def that says `FROM dailysales`. So a name that already worked
/// keeps its exact old identity — nothing sayable bare is quoted, and the fold runs here rather
/// than in the parser, which also makes the identity independent of
/// `datafusion.sql_parser.enable_ident_normalization`.
///
/// Quoting is therefore never a re-keying, only a capability gain, and it fires in two cases:
/// names that were genuinely broken (`Sales 2024`, `2024`, `sales-eu`) where nothing was ever
/// registered to preserve, and reserved words defensively — `Order` folds to `"order"` first, the
/// same identity the unquoted spelling had.
///
/// The reserved-word authority is [`sql::lex::is_reserved_in_name_position`], the same one
/// completion's quoting uses — but the two renderers are **not** interchangeable, and
/// [`sql::quote_verbatim`] states the difference: that one preserves the spelling, for a name
/// whose identity belongs to a server. `pub` because a surface composing a statement about a
/// *workspace* def has to say the name that def will be keyed under (Pin as view).
pub fn quote_ident(name: &str) -> String {
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
///
/// `packages` supply the engine's SQL functions; `pool`, when given, is the memory pool
/// DataFusion executes against.
fn build_context(
    overrides: &BTreeMap<String, String>,
    packages: &[Arc<dyn UdfPackage>],
    formats: &Formats,
    pool: Option<Arc<dyn MemoryPool>>,
) -> SessionContext {
    let mut config = SessionConfig::new().with_information_schema(true);
    for (key, value) in overrides {
        if key.starts_with("datafusion.runtime.") {
            continue;
        }
        if config::is_owned_key(key) {
            continue;
        }
        if let Err(e) = config.options_mut().set(key, value) {
            tracing::warn!("engine config: skipping {key}={value}: {e}");
        }
    }
    let mut config = config.with_default_catalog_and_schema(CATALOG, SCHEMA);
    config.options_mut().sql_parser.collect_spans = true;
    let rt = match build_runtime(overrides, pool.clone()) {
        Ok(rt) => rt,
        Err(e) => {
            tracing::warn!("engine runtime config invalid ({e}); using defaults");
            build_runtime(&BTreeMap::new(), pool).expect("default runtime")
        }
    };
    let state = SessionStateBuilder::new()
        .with_config(config)
        .with_runtime_env(rt)
        .with_default_features()
        .with_catalog_list(Arc::new(StrataCatalogList::default()))
        .with_optimizer_rules(sources::sql::optimizer_rules())
        .with_query_planner(Arc::new(FederatedQueryPlanner::new()))
        .build();
    let mut ctx = SessionContext::new_with_state(state);
    ctx.register_catalog(CATALOG, Arc::new(StrataCatalogProvider::default()));
    if let Err(e) = datafusion_functions_json::register_all(&mut ctx) {
        tracing::warn!("engine: JSON functions unavailable: {e}");
    }
    udf_package::register_packages(&ctx, packages);
    formats.register_writers(&ctx);
    ctx.with_function_factory(Arc::new(StrataFunctionFactory))
}

/// Whether the engine has a function called `name` (already folded), in any registry.
///
/// **All five, because `DROP FUNCTION` clears all five.** DataFusion's `drop_function` deregisters
/// the scalar, aggregate, window, table and higher-order registries in one go, so "is this name
/// taken" has to be the same question the drop would answer — otherwise a name this predicate does
/// not see can be taken by a `CREATE FUNCTION` and then destroyed for the session by the matching
/// `DROP`, which is exactly the loss the built-in fence exists to prevent. Asking three was wrong
/// for the higher-order set in particular: `array_filter`, `array_transform` and `array_any_match`
/// are registered **only** there, so they read as free names.
///
/// The table functions are asked through the state's own map rather than a registry method, since
/// `FunctionRegistry` has none for them; `state_ref` rather than `state()`, which clones.
fn registered_function(ctx: &SessionContext, name: &str) -> bool {
    ctx.udf(name).is_ok()
        || ctx.udaf(name).is_ok()
        || ctx.udwf(name).is_ok()
        || ctx.higher_order_function(name).is_ok()
        || ctx.state_ref().read().table_functions().contains_key(name)
}

/// The catalog + schema **we own** — see [`build_context`].
///
/// The catalog's name is `strata-model`'s, because a connection's own catalog name may not be it
/// ([`strata_model::check_catalog`]) and a name written down twice is a name that can disagree.
const CATALOG: &str = strata_model::WORKSPACE_CATALOG;
const SCHEMA: &str = "public";

/// Re-initialise the UDFs that read `ConfigOptions` **when they were registered**, after a write
/// to a live session's options. Call from every path that moves an option: a Settings Apply
/// ([`Engine::set_config`]) and a typed `SET` / `RESET` (`statements::arms::session`).
///
/// Writing `ConfigOptions` is not the whole of applying a setting, and the gap is silent rather
/// than loud. `NowFunc` captures `execution.time_zone` in `new_with_config` and bakes it into the
/// literal its `simplify` returns, and the `to_timestamp` family does the same — so an option
/// written without this reports success, moves `SHOW`, and leaves `now()` / `current_timestamp`
/// answering in the zone the engine was *built* with until a restart. DataFusion's own
/// `set_variable` and `reset_variable` do this immediately after the same `options.set` call
/// (`context/mod.rs`); the `SessionStateBuilder` does it at construction, which is why a launch
/// override always worked and only a live change did not.
///
/// `with_updated_config` returning `None` is the overwhelmingly common answer (the trait's own
/// default), so this walks the registry and re-registers the handful that opt in.
fn refresh_config_dependent_udfs(state: &mut SessionState) {
    let options = state.config().options();
    let updated: Vec<Arc<ScalarUDF>> = state
        .scalar_functions()
        .values()
        .filter_map(|udf| udf.inner().with_updated_config(options).map(Arc::new))
        .collect();
    for udf in updated {
        if let Err(e) = state.register_udf(udf) {
            tracing::warn!("engine config: could not re-register a config-dependent function: {e}");
        }
    }
}

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
/// `parse_capacity_limit`; the TTL via [`strata_core::util::parse_duration`], the same function
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
/// there was no way to set them. The properties editor is that way: it offers the key,
/// validates the value, badges it RESTART and rebuilds the engine, and the setting then did
/// nothing, with no error to say so. A catalogue entry is a promise that the key applies, so
/// adding one to `ENGINE_KEYS` means adding it here in the same change.
///
/// A `pool` given here takes precedence over `memory_limit`, which otherwise builds one. The limit
/// is still parsed either way, so a malformed value is reported whether or not it is used.
fn build_runtime(
    overrides: &BTreeMap<String, String>,
    pool: Option<Arc<dyn MemoryPool>>,
) -> Result<Arc<RuntimeEnv>, String> {
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

    let mut b = RuntimeEnvBuilder::new().with_object_list_cache_limit(bytes(
        "datafusion.runtime.list_files_cache_limit",
        config::key_def("datafusion.runtime.list_files_cache_limit")
            .expect("the catalogued key")
            .default,
    )?);
    let mem = mem
        .as_deref()
        .map(|v| bytes("datafusion.runtime.memory_limit", v))
        .transpose()?;
    match (pool, mem) {
        (Some(pool), _) => b = b.with_memory_pool(pool),
        (None, Some(limit)) => b = b.with_memory_limit(limit, 1.0),
        (None, None) => {}
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
        let ttl = strata_core::util::parse_duration(v).ok_or_else(|| {
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

    use crate::builder::test_context;
    use crate::statements::Fault;
    use strata_model::{SourceFormat, StatKey};

    use super::*;

    /// Big enough that the first dispatch is still streaming when the second lands, and
    /// cheap to abort (the spool awaits per batch, so the abort takes effect at once).
    const SLOW: &str = "SELECT count(*) FROM generate_series(1, 50000000)";
    const FAST: &str = "SELECT 1 AS n";
    /// A view body whose **profile** is still counting when a test acts on it: 50M distinct
    /// values, aborted within a few dozen milliseconds, so the scan never accumulates far.
    const SLOW_ROWS: &str = "SELECT * FROM generate_series(1, 50000000)";

    /// A decision point that never answers — a stand-in for a policy service that is thinking,
    /// or hung. Nothing waits on it: the tests that install it cancel or drop instead.
    struct Pending;

    #[async_trait::async_trait]
    impl PolicyProvider for Pending {
        async fn admit(&self, _: &Principal, _: GrantFamily) -> Result<Admit, String> {
            std::future::pending().await
        }

        async fn permit(
            &self,
            _: &Principal,
            _: GrantFamily,
            _: &TargetFacts,
        ) -> Result<Admit, String> {
            std::future::pending().await
        }
    }

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
    /// agent's run is awaited inside an MCP request future instead, and a client
    /// cancellation, a dropped connection or the agent server shutting down all drop it
    /// mid-await. Without `DispatchGuard` the entry survives forever: the window's in-flight
    /// flag latches on, so every later close, re-root and engine restart raises the
    /// close-while-running confirm for a query that finished long ago.
    #[test]
    fn a_dropped_run_future_does_not_leave_the_workspace_running() {
        let engine = Engine::builder().build();
        let ws = WsId(1);
        let flag = engine.work().flag();

        let running = dispatched(engine.ws(ws).query(RunTag(1), SLOW.into(), 10));
        assert!(engine.ws(ws).is_running(), "the dispatch published");
        assert!(
            flag.load(Ordering::Relaxed),
            "and the window sees work in flight"
        );

        drop(running);

        assert!(
            !engine.ws(ws).is_running(),
            "dropping the caller must retract the dispatch, not strand it"
        );
        assert!(
            !flag.load(Ordering::Relaxed),
            "and the close-while-running flag must go back down"
        );
    }

    /// **A refusal must not supersede.** Classification runs before `Workspace::run` brackets
    /// anything, and that ordering is load-bearing: a Run press that turns out to be a statement
    /// the engine will not perform leaves the workspace's in-flight run alone.
    ///
    /// Bracketing the classification would let `cancel` reach a slow policy check, since nothing
    /// is registered while a statement classifies. This is what that costs: `bookkeep` aborts
    /// whatever the workspace was running *before* it registers, so a typo would destroy a scan
    /// that is minutes in. Anything that reaches a classifying statement has to register without
    /// superseding.
    #[tokio::test]
    async fn a_refused_statement_leaves_the_workspaces_run_alone() {
        let engine = Engine::builder().build();
        let ws = WsId(1);
        let _running = dispatched(engine.ws(ws).query(RunTag(1), SLOW.into(), 10));
        assert!(engine.ws(ws).is_running(), "the scan is in flight");

        let refused = engine
            .ws(ws)
            .run(RunTag(2), "CREATE DATABASE d".into(), 10)
            .await
            .err()
            .expect("refused");
        assert_eq!(refused, Fault::CreateDatabase.message());
        assert!(
            engine.ws(ws).is_running(),
            "a statement the engine refuses must not take the scan with it"
        );
    }

    /// **A statement being classified is cancellable, and a caller that goes away leaves the
    /// workspace idle** — the window in front of dispatch (`Classifying`).
    ///
    /// Classification is parse + qualify + the [`PolicyProvider`], and that last one is a seam an
    /// embedder fills: a provider backed by a service makes the window as long as its round trip.
    /// Before the classification was bracketed, `cancel` found nothing there and `is_running`
    /// said no, so a tab looked idle for the whole of it and Stop did nothing.
    #[tokio::test]
    async fn a_classifying_statement_is_cancellable() {
        let engine = Engine::builder().with_policy(Pending).build();
        let flag = engine.work().flag();
        let (ws, tag) = (WsId(1), RunTag(1));

        let observe = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            let seen = (flag.load(Ordering::Relaxed), engine.ws(ws).is_running());
            (seen, engine.ws(ws).cancel(tag))
        };
        let (settled, (seen, stopped)) =
            tokio::join!(engine.ws(ws).run(tag, FAST.into(), 10), observe);

        assert_eq!(
            seen,
            (true, true),
            "the press is visible while it classifies"
        );
        assert!(stopped.is_some(), "and the cancel found it");
        assert_eq!(settled.err().as_deref(), Some(CANCELLED));
        assert!(!engine.ws(ws).is_running());
        assert!(!flag.load(Ordering::Relaxed), "cleared once it settled");
    }

    /// A caller that drops mid-classification must not strand the entry either — `ClassifyGuard`
    /// is `DispatchGuard`'s counterpart, and a latched entry would keep the close-while-running
    /// confirm asking about a press that is long gone.
    #[test]
    fn a_dropped_classification_does_not_leave_the_workspace_running() {
        let engine = Engine::builder().with_policy(Pending).build();
        let flag = engine.work().flag();
        let ws = WsId(1);

        let pressing = dispatched(engine.ws(ws).run(RunTag(1), FAST.into(), 10));
        assert!(engine.ws(ws).is_running(), "the classification published");
        assert!(flag.load(Ordering::Relaxed));

        drop(pressing);

        assert!(!engine.ws(ws).is_running(), "and dropping it retracts");
        assert!(!flag.load(Ordering::Relaxed));
    }

    /// The guard must not tear down an entry a **newer** dispatch owns — the same `latest`
    /// rule the settle path follows, for the same reason.
    #[test]
    fn a_dropped_superseded_run_leaves_the_newer_dispatch_alone() {
        let engine = Engine::builder().build();
        let ws = WsId(1);

        let first = dispatched(engine.ws(ws).query(RunTag(1), SLOW.into(), 10));
        let _second = dispatched(engine.ws(ws).query(RunTag(2), SLOW.into(), 10));

        drop(first);

        assert!(
            engine.ws(ws).is_running(),
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
        let engine = Engine::builder()
            .with_config(overrides(&[(BATCH, "4096")]))
            .build();
        assert_eq!(live(&engine, BATCH), "4096", "built with the override");

        assert!(!engine.set_config(overrides(&[(BATCH, "1024")])));
        assert_eq!(live(&engine, BATCH), "1024", "applied without a restart");

        assert!(!engine.set_config(BTreeMap::new()));
        assert_eq!(
            live(&engine, BATCH),
            config::key_def(BATCH).expect("catalogued").default,
            "a removed override goes back to the built-in default"
        );
    }

    #[test]
    fn a_runtime_key_owes_a_restart_until_the_engine_is_rebuilt() {
        let engine = Engine::builder().build();
        assert!(!engine.restart_owed());

        assert!(
            engine.set_config(overrides(&[(MEMORY, "2G")])),
            "the RuntimeEnv is fixed at build, so this is owed"
        );
        assert!(engine.restart_owed());
        assert!(
            engine.set_config(overrides(&[(MEMORY, "2G"), (BATCH, "1024")])),
            "a second write still owes the same restart"
        );

        let restarted = Engine::builder().with_config(engine.overrides()).build();
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
        let engine = Engine::builder().build();
        let functions = engine.lang().functions();
        let names: Vec<&str> = functions.scalar.iter().map(|f| f.name.as_str()).collect();
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
        let eng = Engine::builder().build();
        let doc = r#"'{"s": "x", "n": 7, "b": true, "o": {"k": 1}, "a": [1,2], "z": null}'"#;

        let (out, _) = eng
            .ws(WsId(1))
            .query(
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
                r#""x""#.to_string(),
                "7".to_string(),
                "true".to_string(),
                r#"{"k": 1}"#.to_string(),
                "[1,2]".to_string(),
                "NULL".to_string(),
            ]
        );
        assert!(
            out.rows[0][5].null,
            "the JSON null arm is a real null, not the text"
        );

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
        let eng = Engine::builder().build();
        let (out, _) = eng
            .ws(WsId(1))
            .query(
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

        let ctx = test_context(&BTreeMap::new());
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
    /// one did nothing at all — invisible until they had an editor. This is the guard:
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
            let value = match entry.default {
                "" => match entry.kind {
                    config::Kind::Bytes => "64M",
                    config::Kind::Duration => "30s",
                    _ => "/tmp/strata-runtime-test",
                },
                default => default,
            };
            build_runtime(&overrides(&[(entry.key, value)]), None)
                .unwrap_or_else(|e| panic!("{} = {value} was rejected: {e}", entry.key));

            if entry.key == "datafusion.runtime.temp_directory" {
                continue;
            }
            assert!(
                build_runtime(&overrides(&[(entry.key, "nonsense")]), None).is_err(),
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
        let default = build_runtime(&BTreeMap::new(), None).expect("runtime");
        assert!(
            default.cache_manager.get_list_files_cache().is_none(),
            "a fresh engine caches no listing"
        );
        let asked = build_runtime(
            &overrides(&[("datafusion.runtime.list_files_cache_limit", "4M")]),
            None,
        )
        .expect("runtime");
        assert!(asked.cache_manager.get_list_files_cache().is_some());
    }

    #[test]
    fn a_runtime_ttl_is_read_the_way_the_field_validates_it() {
        assert!(build_runtime(
            &overrides(&[("datafusion.runtime.list_files_cache_ttl", "2m")]),
            None
        )
        .is_ok());
        assert_eq!(
            strata_core::util::parse_duration("2m"),
            Some(std::time::Duration::from_secs(120))
        );
        assert!(build_runtime(
            &overrides(&[("datafusion.runtime.list_files_cache_ttl", "nonsense")]),
            None
        )
        .is_err());
    }

    #[test]
    fn set_config_leaves_the_catalog_names_alone() {
        let engine = Engine::builder().build();
        engine.set_config(overrides(&[(
            "datafusion.catalog.default_schema",
            "elsewhere",
        )]));
        assert_eq!(live(&engine, "datafusion.catalog.default_schema"), SCHEMA);
    }

    #[test]
    fn a_nameable_ident_is_emitted_bare_and_case_folded() {
        for name in ["daily_sales", "_scratch", "t9", "orders2024"] {
            assert_eq!(quote_ident(name), name, "already folded — untouched");
        }
        assert_eq!(quote_ident("DailySales"), "dailysales");
        assert_eq!(quote_ident("Revenue"), "revenue");
        assert_eq!(quote_ident("ORDERS"), "orders");
    }

    #[test]
    fn only_an_unsayable_name_is_quoted_and_it_is_escaped() {
        assert_eq!(quote_ident("Sales 2024"), "\"Sales 2024\"");
        assert_eq!(quote_ident("2024"), "\"2024\"", "can't lead with a digit");
        assert_eq!(quote_ident("sales-eu"), "\"sales-eu\"");
        assert_eq!(
            quote_ident("say \"hi\""),
            "\"say \"\"hi\"\"\"",
            "an embedded quote is doubled, not dropped"
        );
        assert_eq!(quote_ident("order"), "\"order\"");
        assert_eq!(quote_ident("Order"), "\"order\"");
    }

    #[test]
    fn the_folded_name_is_the_one_datafusion_resolves() {
        for name in ["daily_sales", "MyView", "Order", "Sales 2024", "2024"] {
            assert_eq!(
                fold_ident(name),
                TableReference::parse_str(name).table(),
                "{name:?}"
            );
        }
        assert_eq!(fold_ident("a.b"), "a.b");
    }

    #[tokio::test]
    async fn a_view_round_trips_under_the_name_it_was_given() {
        let eng = Engine::builder().build();
        for (i, name) in ["daily_sales", "Sales 2024", "say \"hi\"", "Order"]
            .iter()
            .enumerate()
        {
            let meta = eng
                .catalog()
                .create_view((*name).into(), "SELECT 1 AS n".into())
                .await
                .unwrap_or_else(|e| panic!("create {name:?}: {e}"));
            assert_eq!(meta.columns.len(), 1, "the view's own schema came back");

            let ws = WsId(1);
            let select = format!("SELECT * FROM {}", quote_ident(name));
            let (out, _) = eng
                .ws(ws)
                .query(RunTag(i as u128 * 2), select.clone(), 10)
                .await
                .unwrap_or_else(|e| panic!("select from {name:?}: {e}"));
            assert_eq!(out.total, 1);

            eng.catalog()
                .drop_view((*name).into())
                .await
                .unwrap_or_else(|e| panic!("drop {name:?}: {e}"));
            eng.ws(ws)
                .query(RunTag(i as u128 * 2 + 1), select, 10)
                .await
                .expect_err("the drop named the same view the create did");
        }
    }

    /// The upgrade guarantee. A `.strata/project.json` written before quoting existed can
    /// hold a view named `DailySales`, which DataFusion registered as `dailysales` — and
    /// sibling defs / saved queries say `FROM dailysales`. Quoting must not re-key it.
    #[tokio::test]
    async fn a_mixed_case_view_still_registers_under_its_folded_name() {
        let eng = Engine::builder().build();
        eng.catalog()
            .create_view("DailySales".into(), "SELECT 1 AS n".into())
            .await
            .expect("create");

        let meta = eng
            .catalog()
            .create_view("Derived".into(), "SELECT * FROM dailysales".into())
            .await
            .expect("a def referencing the folded name");
        assert_eq!(meta.columns.len(), 1, "…and planned against it");

        for sql in ["SELECT * FROM dailysales", "SELECT * FROM DailySales"] {
            let (out, _) = eng
                .ws(WsId(1))
                .query(RunTag(1), sql.into(), 10)
                .await
                .unwrap_or_else(|e| panic!("{sql}: {e}"));
            assert_eq!(out.total, 1);
        }

        eng.catalog()
            .drop_view("DailySales".into())
            .await
            .expect("drop");
        eng.ws(WsId(1))
            .query(RunTag(2), "SELECT * FROM dailysales".into(), 10)
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
        const NAMES: &[&str] = &["MyView", "DailySales", "daily_sales", "ORDERS", "Order"];
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

        let legacy = Engine::builder().build();
        let now = Engine::builder().build();
        for name in NAMES {
            let df = legacy
                .ctx
                .sql(&format!("CREATE OR REPLACE VIEW {name} AS SELECT 1 AS n"))
                .await
                .unwrap_or_else(|e| panic!("the shipped path handled {name:?}: {e}"));
            let _ = df.collect().await;
            now.catalog()
                .create_view((*name).into(), "SELECT 1 AS n".into())
                .await
                .unwrap_or_else(|e| panic!("create_view {name:?}: {e}"));
        }

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
        let eng = Engine::builder().build();
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
        eng.catalog()
            .register(TableSpec {
                name: name.into(),
                paths: vec![format!(
                    "{}/tests/fixtures/loadfix/regions.csv",
                    env!("CARGO_MANIFEST_DIR")
                )],
                format: SourceFormat::from_name("csv"),
                partitions: Vec::new(),
                connection: None,
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
        let eng = Engine::builder().build();
        register_regions(&eng, "Regions").await;

        let profile = eng
            .catalog()
            .profile("Regions".into())
            .await
            .expect("profile");
        assert!(
            profile.sql.contains("FROM regions"),
            "the folded name, bare: {}",
            profile.sql
        );
        eng.ws(WsId(1))
            .query(RunTag(1), profile.sql.clone(), 10)
            .await
            .unwrap_or_else(|e| panic!("re-running the printed query: {e}\n{}", profile.sql));
    }

    /// A scan through the facade: the rows it read, and the per-type facts for the columns it
    /// found. `regions.csv` is two `Utf8` columns, so each gets distinct / min / max and a
    /// null count — and *not* mean / median, which are a type error on a string and would
    /// fail the whole aggregate rather than one column (`profile::aggregates`).
    #[tokio::test]
    async fn a_scan_lands_the_per_type_facts_of_every_column() {
        let eng = Engine::builder().build();
        register_regions(&eng, "regions").await;

        let profile = eng
            .catalog()
            .profile("regions".into())
            .await
            .expect("profile");

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
    /// flight — and the Cancel in the inspector's running state has to actually stop it.
    ///
    /// The subject is a **view** over `generate_series`, which is also the case a scan matters
    /// most for (a view has no footer at all): 50M rows of `count(distinct …)` is comfortably
    /// slow enough to observe, and aborts at the next await.
    #[tokio::test]
    async fn a_scan_is_work_in_flight_and_cancel_stops_it() {
        let eng = Engine::builder().build();
        let flag = eng.work().flag();
        eng.catalog()
            .create_view("slow".into(), SLOW_ROWS.into())
            .await
            .expect("create view");
        assert!(!flag.load(Ordering::Relaxed), "idle to begin with");

        let observe = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            let flagged = flag.load(Ordering::Relaxed);
            let cancelled = eng.catalog().cancel_profile("slow");
            (flagged, cancelled)
        };
        let (settled, (flagged, cancelled)) =
            tokio::join!(eng.catalog().profile("slow".into()), observe);

        assert!(flagged, "a running scan is work in flight");
        assert!(cancelled, "…and the cancel found it");
        assert_eq!(
            settled.as_ref().err().map(String::as_str),
            Some("cancelled")
        );
        assert!(!flag.load(Ordering::Relaxed), "cleared once it settled");
        assert!(
            !eng.catalog().cancel_profile("slow"),
            "nothing left in flight to cancel"
        );
        assert!(!eng.ws(WsId(1)).is_running());
    }

    /// Two things at once, because they are the same bookkeeping. A **re-scan supersedes**: the
    /// older call reports no numbers, and the newer one owns the entry — which the late cancel
    /// proves, since a settle path that keyed on the *name* would have removed the newer scan's
    /// entry on its way out and left nothing to cancel (and the flag latched). And profiles are
    /// **keyed per entry**, so a scan of another table runs alongside rather than being
    /// superseded by it.
    #[tokio::test]
    async fn a_re_scan_supersedes_its_own_entry_and_nobody_elses() {
        let eng = Engine::builder().build();
        eng.catalog()
            .create_view("slow".into(), SLOW_ROWS.into())
            .await
            .expect("create view");
        register_regions(&eng, "regions").await;

        let first = eng.catalog().profile("slow".into());
        let re_scan = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            eng.catalog().profile("slow".into()).await
        };
        let other = eng.catalog().profile("regions".into());
        let stop = async {
            tokio::time::sleep(std::time::Duration::from_millis(75)).await;
            eng.catalog().cancel_profile("slow")
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
        let eng = Engine::builder().build();
        eng.catalog()
            .create_view("slow".into(), SLOW_ROWS.into())
            .await
            .expect("create view");

        let scan = eng.catalog().profile("slow".into());
        let replace = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            eng.catalog()
                .create_view("slow".into(), "SELECT 1 AS n".into())
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
        let eng = Engine::builder().build();
        let flag = eng.work().flag();
        assert!(!flag.load(Ordering::Relaxed), "seeded from an idle engine");

        let (ws, tag) = (WsId(1), RunTag(1));
        let observe = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            let seen = (flag.load(Ordering::Relaxed), eng.ws(ws).is_running());
            eng.ws(ws).cancel(tag);
            seen
        };
        let (settled, seen) = tokio::join!(eng.ws(ws).query(tag, SLOW.into(), 10), observe);

        assert_eq!(seen, (true, true), "flagged for as long as it executes");
        assert!(settled.is_err(), "the cancel landed");
        assert!(!flag.load(Ordering::Relaxed), "cleared once it settled");
        assert!(!eng.ws(ws).is_running());
    }

    /// **Background work raises the same flag a run does** — the close confirm's whole gate is
    /// that one `AtomicBool` (`close::CloseHook::running`), so anything the user would rather
    /// finish has to be counted in it, not merely runnable.
    ///
    /// Asserted on the guard rather than by racing a real drop or export: the guard *is* the
    /// mechanism (`Catalog::drop_table` and `SnapshotReads::export` each hold one for the length of
    /// their await), the count is what a leaked increment would strand true for the engine's
    /// whole life, and a test that had to make a delete slow enough to observe would be timing
    /// against a filesystem.
    #[test]
    fn background_work_raises_the_close_confirms_flag_and_releases_it() {
        let eng = Engine::builder().build();
        let flag = eng.work().flag();
        assert!(!flag.load(Ordering::Relaxed), "seeded from an idle engine");
        assert!(!eng.work().background());

        {
            let _first = BackgroundGuard::new(&eng);
            assert!(flag.load(Ordering::Relaxed), "the window would now ask");
            assert!(eng.work().background());
            let second = BackgroundGuard::new(&eng);
            drop(second);
            assert!(
                flag.load(Ordering::Relaxed),
                "still flagged while the other is going"
            );
        }

        assert!(!flag.load(Ordering::Relaxed), "cleared once both released");
        assert!(!eng.work().background());
    }

    #[tokio::test]
    async fn a_repeat_dispatch_of_one_tag_leaves_the_newer_run_intact() {
        let eng = Engine::builder().build();
        let (ws, tag) = (WsId(1), RunTag(7));
        let first = eng.ws(ws).query(tag, SLOW.into(), 10);
        let second = async {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            eng.ws(ws).query(tag, FAST.into(), 10).await
        };
        let (first, second) = tokio::join!(first, second);

        let (out, _) = second.expect("the newer dispatch settles Ok");
        let snap = out.snapshot.expect("…owning a snapshot of its own");
        eng.snapshot(snap)
            .page(1, 10, None)
            .await
            .expect("…which the older dispatch did not retire");
        if let Err(e) = &first {
            assert_eq!(e, "cancelled", "the superseded dispatch settles cancelled");
        }
        assert!(
            eng.ws(ws).cancel(tag).is_none(),
            "both settled — nothing left in flight"
        );
    }

    /// **A query through `run` is a query through `query`.** The whole promise of routing is
    /// that the read path did not move: same snapshot handle, same page 1, same totals — so a
    /// regression here is the router having grown an opinion it has no business having.
    #[tokio::test]
    async fn a_query_routed_through_run_is_the_query_path_unchanged() {
        let eng = Engine::builder().build();
        let sql = "SELECT * FROM (VALUES (2), (1), (3)) AS t";

        let RunOutcome::Rows(routed, _) = eng
            .ws(WsId(1))
            .run(RunTag(1), sql.into(), 2)
            .await
            .expect("a SELECT runs")
        else {
            panic!("a SELECT settles rows");
        };
        let (direct, _) = eng
            .ws(WsId(2))
            .query(RunTag(2), sql.into(), 2)
            .await
            .expect("…as it always did");

        assert_eq!(routed.total, direct.total);
        assert_eq!(routed.rows, direct.rows);
        assert_eq!(routed.columns.len(), direct.columns.len());
        let snap = routed.snapshot.expect("a snapshot handle");
        let (page2, _) = eng.snapshot(snap).page(2, 2, None).await.expect("page 2");
        assert_eq!(page2.len(), 1);
    }

    /// A refused statement fails with **the squiggle's own words** — the classifier's own table,
    /// not DataFusion's account of a rule that is ours. `CREATE DATABASE` is the refusal that
    /// stays refused: it is structurally impossible, not merely unimplemented.
    #[tokio::test]
    async fn a_refused_statement_fails_with_the_editors_message() {
        let eng = Engine::builder().build();
        let err = eng
            .ws(WsId(1))
            .run(RunTag(1), "CREATE DATABASE d".into(), 10)
            .await
            .err()
            .expect("refused");
        assert_eq!(err, Fault::CreateDatabase.message());
    }

    /// A statement that creates something still needs somewhere to put it, and an engine with no
    /// project behind it says so rather than failing in DataFusion's words about a path nobody
    /// chose. Both creating statements, because what is missing differs: a `CREATE TABLE`
    /// has nowhere to write the **data**, and a typed registration has nowhere to write the
    /// **def**, which is the durable half of one.
    ///
    /// Every interception has a real arm, so there is no stub refusal to assert.
    #[tokio::test]
    async fn creating_a_table_without_a_project_folder_says_why() {
        let eng = Engine::builder().build();
        for (sql, expected) in [
            (
                "CREATE TABLE t AS SELECT 1",
                "CREATE TABLE AS needs a project folder to store the table's data",
            ),
            (
                "CREATE EXTERNAL TABLE t STORED AS CSV LOCATION 'x.csv'",
                "CREATE EXTERNAL TABLE needs a project folder to store the table",
            ),
        ] {
            let err = eng
                .ws(WsId(1))
                .run(RunTag(1), sql.into(), 10)
                .await
                .err()
                .expect("nowhere to store it");
            assert_eq!(err, expected);
        }
    }

    /// **Neither refusal touches the snapshot lifecycle.** DDL does not retire a snapshot
    /// (`SNAPSHOT_SPEC` §4), so the workspace's settled result is still there to page after a
    /// statement runs in the same tab — which is also what makes the results pane's "previous
    /// snapshot survives" claim true rather than hopeful.
    #[tokio::test]
    async fn a_statement_leaves_the_workspaces_snapshot_alone() {
        let eng = Engine::builder().build();
        let (ws, sql) = (WsId(1), "SELECT * FROM (VALUES (1), (2)) AS t");

        let (out, _) = eng
            .ws(ws)
            .query(RunTag(1), sql.into(), 10)
            .await
            .expect("rows");
        let snap = out.snapshot.expect("a snapshot handle");

        for stmt in ["CREATE DATABASE d", "DROP TABLE t"] {
            eng.ws(ws)
                .run(RunTag(2), stmt.into(), 10)
                .await
                .err()
                .expect("refused or stubbed");
            assert!(eng.snapshot(snap).live(), "{stmt} retired the snapshot");
        }
        eng.snapshot(snap)
            .page(1, 10, None)
            .await
            .expect("…and it still reads");
        assert!(!eng.ws(ws).is_running(), "nothing left in flight either");
    }

    /// **One statement per Run**, refused with a policy sentence. Left to DataFusion this is
    /// its parser complaining about its own limit, which tells the user nothing about what to
    /// do next; the buffer is still validated per statement, so the squiggles are unaffected.
    #[tokio::test]
    async fn a_multi_statement_run_is_refused_as_a_batch() {
        let eng = Engine::builder().build();
        let err = eng
            .ws(WsId(1))
            .run(RunTag(1), "SELECT 1; SELECT 2".into(), 10)
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
        let eng = Engine::builder().build();
        for sql in [
            "SELECT 1 AS n;",
            "SELECT 1 AS n;\n",
            "SELECT 1 AS n ;;",
            "-- a note\nSELECT 1 AS n;",
            "SELECT ';' AS n;",
            "WITH t AS (SELECT 1 AS n) SELECT * FROM t;",
        ] {
            let outcome = eng.ws(WsId(1)).run(RunTag(1), sql.into(), 10).await;
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
        let eng = Engine::builder().build();
        let err = eng
            .ws(WsId(1))
            .run(RunTag(1), "-- just thinking out loud\n".into(), 10)
            .await
            .err()
            .expect("nothing to run");
        assert_eq!(err, "Nothing to run");
    }
}

/// **Read options, end to end** — every option the Configure window offers, proved against a
/// real file rather than against DataFusion's builder signature.
///
/// This is the point of the validation pass. The bar for offering an option is that it
/// reaches the read, and the only way to hold that bar over a DataFusion upgrade is to register
/// a table whose schema or rows are *different* because the option is set. Each test here
/// therefore asserts the difference, not the call.
#[cfg(test)]
mod read_options_tests {
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::process;

    use strata_model::{CsvRead, FileCompression, JsonRead, JsonShape, SourceFormat};

    use super::*;

    /// A fresh directory per test, so a stale fixture can never make one pass — and **per
    /// process**, so a second test run cannot be the thing that makes it stale. The wipe on
    /// entry is what makes a shared path dangerous rather than merely untidy: it deletes
    /// fixtures another run may be asserting over.
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata_read_options_{}_{name}", process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("temp dir");
        d
    }

    fn write(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        fs::write(&path, body).expect("fixture");
        path.to_string_lossy().into_owned()
    }

    /// The same, gzipped — a compression option can only be proved by a genuinely compressed
    /// file whose name carries the suffix.
    fn write_gz(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        let mut enc = flate2::write::GzEncoder::new(
            File::create(&path).expect("fixture"),
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
            connection: None,
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
        fs::write(d.join("t.csv"), "a,b\n1,NAN\n2,3\n").expect("fixture");
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

        let with = read("OPTIONS('format.null_value' 'NAN')")
            .await
            .expect("scan");
        let without = read("").await.expect("scan");
        assert_eq!(
            format!("{with:?}"),
            format!("{without:?}"),
            "NULL_VALUE changes nothing a reader can see"
        );

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
        let eng = Engine::builder().build();

        let meta = eng
            .catalog()
            .register(spec(
                "commas",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect("register");
        assert_eq!(names(&meta), vec!["a;b;c"]);

        let meta = eng
            .catalog()
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
        let eng = Engine::builder().build();

        let meta = eng
            .catalog()
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
        let eng = Engine::builder().build();

        let err = eng
            .catalog()
            .register(spec(
                "raw",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect_err("the comment line is taken as the header");
        assert!(err.contains("unequal lengths"), "{err}");

        let meta = eng
            .catalog()
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
        let eng = Engine::builder().build();

        let meta = eng
            .catalog()
            .register(spec(
                "strict",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect("registration succeeds — which is the trap");
        assert_eq!(names(&meta), vec!["x", "y", "z"]);
        let err = eng
            .ws(WsId(1))
            .query(RunTag(1), "SELECT * FROM strict".into(), 100)
            .await
            .expect_err("the short file cannot be read against the merged schema");
        assert!(err.contains("incorrect number of fields"), "{err}");

        let meta = eng
            .catalog()
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
        eng.ws(WsId(2))
            .query(RunTag(2), "SELECT * FROM union".into(), 100)
            .await
            .expect("the missing column is padded with nulls");
    }

    /// The same option, within one file: a row short of a column fails the register outright.
    #[tokio::test]
    async fn truncated_rows_also_covers_a_ragged_row_inside_one_file() {
        let d = dir("ragged");
        let path = write(&d, "r.csv", "x,y\n1,2\n3\n");
        let eng = Engine::builder().build();

        assert!(eng
            .catalog()
            .register(spec(
                "strict",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .is_err());

        let meta = eng
            .catalog()
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
        eng.ws(WsId(3))
            .query(RunTag(3), "SELECT * FROM ragged".into(), 100)
            .await
            .expect("the short row is padded");
    }

    /// Infer-rows at 0 is DataFusion's own "read everything as text" — the one claim the
    /// canvas's hint makes about this field, so it is the one asserted.
    #[tokio::test]
    async fn zero_infer_rows_reads_every_csv_column_as_text() {
        let d = dir("infer");
        let path = write(&d, "s.csv", "n\n1\n2\n");
        let eng = Engine::builder().build();

        let meta = eng
            .catalog()
            .register(spec(
                "typed",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect("register");
        assert_eq!(meta.columns[0].dtype, "Int64");

        let meta = eng
            .catalog()
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
        let eng = Engine::builder().build();

        let err = eng
            .catalog()
            .register(spec(
                "plain",
                vec![path.clone()],
                SourceFormat::Csv(CsvRead::default()),
            ))
            .await
            .expect_err("the extension filter excludes it");
        assert!(err.contains("not one"), "{err}");

        let meta = eng
            .catalog()
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
    /// `Catalog::register` and a real `SELECT`, not just the inference unit tests. Arrow's reader
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
        let eng = Engine::builder().build();

        let meta = eng
            .catalog()
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

        let (out, _) = eng
            .ws(WsId(1))
            .query(RunTag(1), "SELECT content FROM poly ORDER BY id".into(), 10)
            .await
            .expect("query");
        let cells: Vec<String> = out.rows.iter().map(|r| r[0].text.clone()).collect();
        assert_eq!(
            cells,
            vec![
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
        let eng = Engine::builder().build();

        let meta = eng
            .catalog()
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

        let (out, _) = eng
            .ws(WsId(1))
            .query(RunTag(1), "SELECT * FROM t".into(), 10)
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
        let eng = Engine::builder().build();

        let meta = eng
            .catalog()
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
        let eng = Engine::builder().build();

        let err = eng
            .catalog()
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
            let eng = Engine::builder().with_config(ov).build();
            eng.catalog()
                .register(spec(
                    "rows",
                    vec![path.clone()],
                    SourceFormat::Json(JsonRead::default()),
                ))
                .await
                .expect("register");

            let (out, _) = eng
                .ws(WsId(1))
                .query(RunTag(1), "SELECT count(*) AS n FROM rows".into(), 10)
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
        let eng = Engine::builder().build();

        let err = eng
            .catalog()
            .register(spec(
                "ndjson",
                vec![path.clone()],
                SourceFormat::Json(JsonRead::default()),
            ))
            .await
            .expect_err("an array is not newline-delimited");
        assert!(err.contains("Set the JSON shape to array"), "{err}");

        let meta = eng
            .catalog()
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

    /// A def naming a format nothing is registered for fails **by name**, and names the fix.
    /// The arm this replaces was a fallthrough onto parquet, so such a table registered as
    /// parquet and said nothing — the register's one job is to be the check.
    #[tokio::test]
    async fn a_format_with_no_registrant_fails_by_name_rather_than_being_read_as_parquet() {
        let d = dir("unknown");
        let path = write(&d, "s.avro", "not really avro");
        let eng = Engine::builder().build();

        let err = eng
            .catalog()
            .register(spec("legacy", vec![path], SourceFormat::from_name("avro")))
            .await
            .expect_err("no reader");
        assert_eq!(
            err,
            "Table 'legacy' is defined as 'avro', which no reader is registered for. Register \
             one with EngineBuilder::with_format, or change the table's format."
        );
    }

    /// A multi-byte delimiter is reported, never truncated to its first byte — which would
    /// split fields in the middle of the next character.
    #[tokio::test]
    async fn a_multi_byte_delimiter_is_refused_by_name() {
        let d = dir("wide_delimiter");
        let path = write(&d, "s.csv", "a,b\n1,2\n");
        let eng = Engine::builder().build();

        let err = eng
            .catalog()
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

        let err = eng
            .catalog()
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

/// **What the engine can say about a database connection's catalog** — the two reads
/// the agent vocabulary needs so it stops answering "not found" about relations it can query.
#[cfg(test)]
mod remote_catalog_tests {
    use super::*;
    use crate::providers::fake_source;
    use crate::sources::fake::{fake_def, TestDoc};

    /// **The workspace is not a database, by construction.** The catalogs an agent is told about
    /// are the *connections* this engine holds, so the project's own catalog cannot appear among
    /// them however it is registered — and neither can a bucket, which holds files.
    #[tokio::test]
    async fn the_workspace_catalog_is_not_a_database() {
        let engine = Engine::builder()
            .with_source(TestDoc::holding("fixture", &["orders"]))
            .build();
        assert!(
            engine.sources().listing().catalog_names().is_empty(),
            "a project with no connection has no database catalogs"
        );

        engine
            .sources()
            .connect(fake_def::<TestDoc>("Sales", "fixture"))
            .await
            .expect("the source registers its catalog");

        assert_eq!(
            engine.sources().listing().catalog_names(),
            vec!["Sales".to_string()],
            "in the spelling it was registered under, and without the workspace's own"
        );
    }

    /// A qualified remote name describes; everything else is an **expected absence**
    /// (`Ok(None)`) and leaves the store to say what it knows — a bare name is a def's, and a
    /// def is not this method's business.
    ///
    /// The split that matters is absence against fault: every name below is an `Ok`, because
    /// none of them is a failure. `Err` is reserved for a relation the connection lists whose
    /// introspection then fails, which no fake catalog can produce — that arm is the real
    /// server's (`tests/postgres_federation.rs`).
    ///
    /// Names are folded like every other in the session, and the answer carries the connection's
    /// and the server's own spellings rather than the caller's. The absent set covers the four
    /// ways a name is not this method's business: not in the connection, not a database catalog
    /// (`strata`, and `STRATA`, which the catalog list resolves by folding), and not qualified
    /// into one at all, which is a def's name for the store to answer.
    #[tokio::test]
    async fn describe_remote_answers_for_a_relation_and_nothing_else() {
        let engine = Engine::builder().build();
        fake_source(&engine.ctx, "pg", &["orders"]);

        let described = engine
            .sources()
            .describe_remote("pg.public.orders".into())
            .await
            .expect("no fault")
            .expect("a relation the connection has");
        assert_eq!(described.connection, "pg");
        assert_eq!(described.relation, "public.orders");
        assert!(!described.view);
        assert_eq!(
            described
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "total"]
        );

        let folded = engine
            .sources()
            .describe_remote("PG.PUBLIC.ORDERS".into())
            .await
            .expect("no fault")
            .expect("folded");
        assert_eq!(folded.connection, "pg");
        assert_eq!(folded.relation, "public.orders");

        for name in [
            "pg.public.gone",
            "strata.public.orders",
            "STRATA.public.orders",
            "orders",
            "public.orders",
        ] {
            assert_eq!(
                engine
                    .sources()
                    .describe_remote(name.into())
                    .await
                    .expect("no fault"),
                None,
                "{name}"
            );
        }
    }
}
