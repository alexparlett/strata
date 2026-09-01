//! What the engine has in flight, and the guards that keep the record true.
//!
//! [`Lifecycle`] is the bookkeeping itself — one lock, never held across an await — and the rest
//! of the file is the RAII around it: every entry that is published before an await is undone by
//! a guard, because a dropped future must not be able to leave a workspace looking busy for the
//! engine's life. [`SnapshotPin`] is the one that is handed **out**, so a caller can hold a
//! result open past the re-run that would retire it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Instant;

use tokio::task::AbortHandle;

use strata_model::SnapshotId;

use crate::snapshots::SnapshotStats;
use crate::{Engine, RunTag, WsId};

/// An in-flight **profile scan**: which dispatch it is, and the handle that cancels it.
///
/// Keyed by catalog entry rather than by workspace, because a profile belongs to the *data*:
/// it is asked for from the catalog, cached per entry, and two tables profile concurrently.
/// There is no snapshot — a scan materializes one aggregate row and returns it.
pub(crate) struct ProfileRun {
    /// Engine-unique, monotonic — the same "am I still the latest?" check [`InFlight`] uses,
    /// for the same reason: a re-scan supersedes, and the superseded call must not tear down
    /// the entry the newer one now owns.
    pub(crate) dispatch: u64,
    pub(crate) abort: AbortHandle,
}

/// A workspace's in-flight run or explain: which dispatch it is, the snapshot it is
/// materializing (`None` for an explain), and the abort handle that cancels it.
pub(crate) struct InFlight {
    /// Engine-unique, monotonic — the identity every "am I still the latest run?" check
    /// compares. A [`RunTag`] can't do that job: it is the caller's nonce, and a repeat
    /// dispatch of the same tag would make the superseded call mistake the *new* entry
    /// for its own and tear down state it doesn't own.
    pub(crate) dispatch: u64,
    /// The caller's nonce, kept for exactly one thing: [`Workspace::cancel`]'s guard.
    pub(crate) tag: RunTag,
    pub(crate) snapshot: Option<SnapshotId>,
    pub(crate) abort: AbortHandle,
    pub(crate) start: Instant,
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
pub(crate) struct Classifying {
    /// Engine-unique, monotonic — the same "am I still the latest?" identity [`InFlight`] uses.
    /// A second `run` on the same workspace replaces the entry rather than aborting it, so the
    /// first one's settle path has to know the entry is no longer its own.
    pub(crate) dispatch: u64,
    pub(crate) tag: RunTag,
    pub(crate) abort: AbortHandle,
    pub(crate) start: Instant,
}

/// Undoes a dispatch whose caller went away before it could settle.
///
/// A dispatch publishes its [`InFlight`] entry *before* awaiting the spawned work, so until
/// the settle path runs the workspace looks busy. That was safe while every caller was
/// freya-query, which by design never cancels an execution — but an agent's run is
/// awaited inside an MCP request future, and a client cancellation, a dropped data source or
/// the agent server shutting down all drop it mid-await. Without this the entry is never
/// removed: [`publish_inflight`](Engine::publish_inflight) latches the window's in-flight flag
/// on for the engine's life, so every later close, re-root and restart asks the T2 confirm
/// about a query that finished long ago, `is_running` reports a phantom, and the snapshot the
/// detached task materialized is never retired.
///
/// Armed for the await and [`disarm`](Self::disarm)ed by the settle path, so the ordinary
/// route pays nothing. The drop repeats the settle path's own `latest` check for the reason
/// that check exists: a superseded call must not tear down the entry a newer dispatch owns.
pub(crate) struct DispatchGuard<'a> {
    engine: &'a Engine,
    ws: WsId,
    dispatch: u64,
    armed: bool,
}

impl<'a> DispatchGuard<'a> {
    pub(crate) fn arm(engine: &'a Engine, ws: WsId, dispatch: u64) -> Self {
        Self {
            engine,
            ws,
            dispatch,
            armed: true,
        }
    }

    /// The dispatch settled on its own terms; leave the entry to the settle path.
    pub(crate) fn disarm(&mut self) {
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
pub(crate) struct ClassifyGuard<'a> {
    engine: &'a Engine,
    ws: WsId,
    dispatch: u64,
    armed: bool,
}

impl<'a> ClassifyGuard<'a> {
    pub(crate) fn arm(engine: &'a Engine, ws: WsId, dispatch: u64) -> Self {
        Self {
            engine,
            ws,
            dispatch,
            armed: true,
        }
    }

    /// The classification settled on its own terms; leave the entry to `classify_bracket`.
    pub(crate) fn disarm(&mut self) {
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
pub(crate) struct Lifecycle {
    pub(crate) inflight: HashMap<WsId, InFlight>,
    /// Statements being classified — see [`Classifying`]. Read by `cancel`, `is_running` and
    /// `publish_inflight` beside `inflight`, and by supersede by nobody.
    pub(crate) classifying: HashMap<WsId, Classifying>,
    pub(crate) current: HashMap<WsId, SnapshotId>,
    /// In-flight profile scans by entry identity ([`fold_ident`] of the name — tables and
    /// views share one namespace).
    pub(crate) profiles: HashMap<String, ProfileRun>,
    /// How many pieces of **background** work are in flight — an export writing a file, a
    /// drop deleting a table's data. A **count, not a map**: nothing addresses one of
    /// these — no cancel, no supersede, no per-item state to look up. All it has to do is keep
    /// [`publish_inflight`](Engine::publish_inflight) true while something is half-done, so the
    /// close-while-running confirm asks before the window takes the runtime away.
    ///
    /// Not per-kind, because the question every reader asks is the same one: is anything the
    /// user would rather finish still going? A second counter would be a second answer to it.
    pub(crate) background: usize,
    /// Snapshots a caller is **holding open**, and how many holds each has
    /// ([`SnapshotReads::pin`](crate::SnapshotReads::pin)). A pinned snapshot survives its workspace re-running.
    pub(crate) pins: HashMap<SnapshotId, usize>,
    /// Snapshots whose retire arrived while they were pinned. They are retired for real
    /// when the last pin releases — deferred, never skipped, so nothing leaks.
    pub(crate) deferred: HashSet<SnapshotId>,
    /// What each live snapshot's write pass observed ([`SnapshotStats`]) — today the
    /// exact per-column null counts a partitioned export has to check.
    ///
    /// Here rather than in the file because a snapshot never outlives its process, so this has
    /// exactly its lifetime: inserted when it materializes, dropped when it retires. The Arrow
    /// IPC snapshot carries no statistics of its own, and asking the file was never the point —
    /// the write pass already streams every batch.
    pub(crate) stats: HashMap<SnapshotId, SnapshotStats>,
}

/// One piece of **background** engine work in flight — an export writing a file, a drop
/// deleting a table's data. Holding one is what keeps the close-while-running flag true
/// (`Lifecycle::background`), so the window asks before it takes the runtime away.
///
/// A guard rather than a matching pair of statements because every holder **awaits**, and a
/// dropped future must not be able to leak the count: a leaked increment would make every later
/// window close claim work was running for the rest of the engine's life. Borrows the engine, so
/// it cannot outlive the call.
pub(crate) struct BackgroundGuard<'a> {
    engine: &'a Engine,
}

impl<'a> BackgroundGuard<'a> {
    /// Constructing the guard *is* the acquire, so there is no way to hold one without having
    /// taken what it releases.
    pub(crate) fn new(engine: &'a Engine) -> Self {
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
pub(crate) struct ExportHold {
    pub(crate) snapshot: SnapshotId,
    /// **Weak, and that is load-bearing.** This hold rides on a task running on the runtime the
    /// engine owns, so a strong `Arc` here would close a cycle — engine owns runtime owns task
    /// owns hold owns engine — and the engine would never drop. The write does not need it
    /// either: `run_export` holds its own clone of the `SessionContext`. An engine that has gone
    /// has no bookkeeping left to correct, so a failed upgrade is the whole handling.
    pub(crate) engine: Weak<Engine>,
}

impl ExportHold {
    /// Claim both halves. Constructing the hold *is* the acquire, so there is no way to hold one
    /// without having taken what it releases.
    pub(crate) fn new(engine: &Engine, snapshot: SnapshotId) -> Self {
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
/// (see [`SnapshotReads::pin`](crate::SnapshotReads::pin)). Dropping it releases the hold, and retires the snapshot if
/// a retire arrived while it was pinned and this was the last hold.
///
/// Holds an `Arc<Engine>` rather than a borrow so it can be parked in UI state for a window's
/// lifetime — which is the whole point of it existing.
pub struct SnapshotPin {
    pub(crate) engine: Arc<Engine>,
    pub(crate) snapshot: SnapshotId,
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
