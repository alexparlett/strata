//! One workspace's runs: dispatch, supersede, cancel, tear down.

use strata_arrow::plan::QueryPlan;

use crate::policy::{Capability, Principal};
use crate::query::ReadPolicy;
use crate::statements::arms;
use crate::statements::ctx::StmtCtx;
use crate::statements::pipeline::{accept, Admitted, Pipeline};
use crate::{explain, Engine, EngineError, RunOutcome, RunRows, RunTag, WsId};

/// One workspace's runs, from [`Engine::ws`].
///
/// A workspace runs one thing at a time: every dispatch supersedes whatever it was running. The
/// exception is the window in front of dispatch, where a statement is still being classified —
/// that registers without superseding, so [`cancel`](Self::cancel) reaches it and a refusal
/// leaves a running query alone.
#[derive(Clone, Copy)]
pub struct Workspace<'a> {
    pub(super) engine: &'a Engine,
    pub(super) ws: WsId,
}

impl Workspace<'_> {
    /// Runs `sql`, as a query or as a statement the engine performs itself.
    ///
    /// One pipeline in front of dispatch, the same one [`Lang::validate`](crate::Lang::validate)
    /// reports from, so a statement the editor did not underline is one this is prepared to
    /// perform.
    ///
    /// - `Query` delegates to [`query`](Self::query)'s body **byte-for-byte**, carrying only
    ///   the one thing the pipeline knows and the read path cannot: the [`ReadPolicy`] an
    ///   `EXECUTE` needs. It is the only arm that touches the snapshot lifecycle, which is what
    ///   keeps "DDL does not retire snapshots" true by construction rather than by care.
    /// - `Statement(kind)` goes to `arms::execute`, bracketed by `Engine::bookkeep` so
    ///   [`cancel`](Self::cancel) / [`is_running`](Self::is_running) / the close-while-running
    ///   confirm see it like any other work — a CTAS is a full scan, and a window closing over
    ///   one has to ask.
    /// - A refusal never reaches DataFusion at all: the pipeline is in front of `ctx.sql`
    ///   precisely because DDL executes *eagerly* inside it (spec §3), so anything that must
    ///   not run cannot be allowed to plan.
    ///
    /// The `SQLOptions` triple the read path carries (`query::materialize`) stays defense in
    /// depth behind this: it is no longer the gate, and it never had the vocabulary to be one —
    /// it can refuse a class of plan, not name the surface that owns the capability.
    ///
    /// The classification itself is bracketed too — `Engine::classify_bracket`, which registers
    /// **without** superseding, so a Cancel that lands in that window stops something and a
    /// refusal still leaves the workspace's running query alone (`Classifying`).
    pub async fn run(
        self,
        tag: RunTag,
        sql: String,
        page_size: usize,
    ) -> Result<RunOutcome, EngineError> {
        let ws = self.ws;
        let engine = self.engine;
        let who = Principal::new(Capability::full()).in_session(ws);
        let admitted = {
            let ctx = engine.ctx.clone();
            let policy = engine.policy.clone();
            let who = who.clone();
            let sql = sql.clone();
            engine
                .classify_bracket(ws, tag, async move {
                    let pipeline = Pipeline::new(&ctx);
                    accept(&pipeline, &sql, policy.as_ref(), &who)
                        .await
                        .map_err(EngineError::Refused)
                })
                .await?
        };
        match admitted {
            Admitted::Query { stmt, policy, .. } => engine
                .read(ws, tag, stmt.into_statement(), page_size, policy)
                .await
                .map(RunOutcome::Rows),
            Admitted::Statement { kind, stmt, .. } => {
                let root = engine.data_root.lock().unwrap().clone();
                let cx = StmtCtx {
                    ctx: engine.ctx.clone(),
                    sql,
                    root,
                    internal: engine.internal.clone(),
                    tables: engine.tables.clone(),
                    connections: engine.connections.clone(),
                    sources: engine.live.clone(),
                    formats: engine.formats.clone(),
                    scope: engine.session.clone(),
                    functions: engine.functions.clone(),
                    baseline: engine.overrides(),
                    policy: engine.policy.clone(),
                };
                let report = engine
                    .bookkeep(ws, tag, "statement", async move {
                        arms::execute(kind, stmt, &who, cx).await
                    })
                    .await?;
                engine.settle_effect(report.effect.as_ref());
                Ok(RunOutcome::Statement(report))
            }
        }
    }

    /// Run `sql` **once**: materialize a fresh immutable snapshot
    /// and return its handle + page 1 (`docs/SNAPSHOT_SPEC.md` §3). Dispatch retires
    /// the workspace's previous snapshot and aborts its in-flight run (§4); `tag` is
    /// the caller's nonce for [`cancel`](Self::cancel).
    ///
    /// Supersede checks key on the engine's own dispatch id, never on `tag`: the UI may
    /// dispatch the same tag twice for one logical run, and comparing tags would let the
    /// first call's settle path adopt the second call's `InFlight` entry — dismantling a
    /// perfectly good run and failing *both* calls (see `InFlight::dispatch`).
    pub async fn query(
        self,
        tag: RunTag,
        sql: String,
        page_size: usize,
    ) -> Result<RunRows, EngineError> {
        let stmt = self.engine.parse_one(&sql)?;
        self.engine
            .read(self.ws, tag, stmt, page_size, ReadPolicy::default())
            .await
    }

    /// Run an `EXPLAIN [ANALYZE]` statement — a parsed plan tree, no snapshot.
    /// Supersedes the workspace's in-flight run (mutually exclusive, like a re-run) but
    /// leaves its settled snapshot alone (spec §4: explains materialize nothing).
    pub async fn explain(self, tag: RunTag, sql: String) -> Result<QueryPlan, EngineError> {
        let stmt = self.engine.parse_one(&sql)?;
        let ctx = self.engine.ctx.clone();
        self.engine
            .bookkeep(self.ws, tag, "explain", async move {
                explain::run_explain(&ctx, stmt).await
            })
            .await
    }

    /// Cancel the in-flight run/explain **iff** it is still run `tag` (a stale
    /// cancel can't abort a just-started newer run). Returns the elapsed time when
    /// something was actually cancelled; the awaiting [`query`](Self::query) /
    /// [`explain`](Self::explain) settles [`StopReason::Cancelled`](crate::StopReason::Cancelled).
    ///
    /// The `tag` — the UI's per-press nonce — is exactly right here, and the one place it
    /// is: the caller is asking to stop *the run it can see*, so if a repeat dispatch
    /// replaced the in-flight entry under the same tag, stopping that one is what the
    /// press meant.
    pub fn cancel(self, tag: RunTag) -> Option<u128> {
        let mut lc = self.engine.lifecycle.lock().unwrap();
        if lc.inflight.get(&self.ws).map(|f| f.tag) == Some(tag) {
            let f = lc.inflight.remove(&self.ws).unwrap();
            let elapsed = f.start.elapsed().as_millis();
            self.engine.abort_inflight(f);
            self.engine.publish_inflight(&lc);
            return Some(elapsed);
        }
        // A press can also land while the statement is still being **classified**, before
        // anything is dispatched — see `Classifying`. Nothing has been registered and no
        // snapshot minted, so stopping it is dropping the entry and aborting its task; the
        // awaiting `run` settles `StopReason::Cancelled` exactly as a dispatched one does.
        if lc.classifying.get(&self.ws).map(|c| c.tag) == Some(tag) {
            let c = lc.classifying.remove(&self.ws).unwrap();
            let elapsed = c.start.elapsed().as_millis();
            c.abort.abort();
            self.engine.publish_inflight(&lc);
            return Some(elapsed);
        }
        None
    }

    /// Tear the workspace down (tab close): abort its in-flight run — or the statement it is
    /// still classifying — and retire its current snapshot (spec §4).
    ///
    /// Sync, so it can't await the aborted task the way [`query`](Self::query) does; if the
    /// tab's `query` future is already gone, nothing does. What survives that is bounded — a
    /// `__snap_N` registered over a file we deleted (an uncached read of it fails
    /// cleanly, exactly like any retired snapshot), and at worst a stray parquet file in
    /// this engine's own directory, which `Drop` removes wholesale.
    pub fn cleanup(self) {
        let mut lc = self.engine.lifecycle.lock().unwrap();
        if let Some(f) = lc.inflight.remove(&self.ws) {
            self.engine.abort_inflight(f);
        }
        if let Some(c) = lc.classifying.remove(&self.ws) {
            c.abort.abort();
        }
        if let Some(snap) = lc.current.remove(&self.ws) {
            self.engine.retire_or_defer(&mut lc, snap);
        }
        self.engine.publish_inflight(&lc);
    }

    /// Whether this workspace has a run or explain executing right now — the per-tab half
    /// of the close-while-running confirm (a tab *is* a [`WsId`]). Same reason as
    /// [`Work::flag`](crate::Work::flag): a background tab's run is invisible to
    /// the UI, which mounts only the active tab's results.
    ///
    /// A statement still being **classified** counts, because the user pressed Run and something
    /// is happening (`Classifying`).
    pub fn is_running(self) -> bool {
        let lc = self.engine.lifecycle.lock().unwrap();
        lc.inflight.contains_key(&self.ws) || lc.classifying.contains_key(&self.ws)
    }
}
