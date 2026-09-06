//! One workspace's runs: dispatch, supersede, cancel, tear down.

use strata_arrow::plan::QueryPlan;

use crate::lifecycle::ClassifyGuard;
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

impl<'a> Workspace<'a> {
    /// Runs `sql`, as a query or as a statement the engine performs itself.
    ///
    /// One pipeline in front of dispatch, the same one [`Lang::analyze`](crate::Lang::analyze)
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
        let (admitted, admission) = self.admit(tag, sql.clone(), who.clone()).await?;
        match admitted {
            Admitted::Query { stmt, policy, .. } => engine
                .read(ws, tag, stmt.into_statement(), page_size, policy, admission)
                .await
                .map(RunOutcome::Rows),
            Admitted::Statement { kind, stmt, .. } => {
                let root = engine.data_root.lock().unwrap().clone();
                let cx = StmtCtx {
                    ctx: engine.ctx.clone(),
                    sql,
                    root,
                    owned: engine.owned_storage(),
                    internal: engine.internal.clone(),
                    tables: engine.tables.clone(),
                    sources: engine.source_defs.clone(),
                    registrants: engine.registrants.clone(),
                    live: engine.live.clone(),
                    formats: engine.formats.clone(),
                    scope: engine.session.clone(),
                    functions: engine.functions.clone(),
                    baseline: engine.overrides(),
                    policy: engine.policy.clone(),
                };
                let owner = engine.self_ref.clone();
                let report = engine
                    .bookkeep(ws, tag, "statement", admission, async move {
                        let ran = arms::execute(kind, stmt, &who, cx).await?;
                        let engine = owner.upgrade().ok_or_else(|| {
                            "Engine closed before statement completion".to_string()
                        })?;
                        Ok(engine.settle_effect(ran))
                    })
                    .await?;
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
        let who = Principal::new(Capability::read_only()).in_session(self.ws);
        let (admitted, admission) = self.admit(tag, sql, who).await?;
        self.engine
            .read(
                self.ws,
                tag,
                admitted.into_statement(),
                page_size,
                ReadPolicy::default(),
                admission,
            )
            .await
    }

    /// Builds a read-only plan, enforcing the engine's policy before planning or execution.
    pub async fn explain(self, tag: RunTag, sql: String) -> Result<QueryPlan, EngineError> {
        let who = Principal::new(Capability::read_only()).in_session(self.ws);
        let (admitted, admission) = self.admit(tag, sql, who).await?;
        let stmt = admitted.into_statement();
        let ctx = self.engine.ctx.clone();
        self.engine
            .bookkeep(self.ws, tag, "explain", admission, async move {
                explain::run_explain(&ctx, stmt).await
            })
            .await
    }

    async fn admit(
        self,
        tag: RunTag,
        sql: String,
        who: Principal,
    ) -> Result<(Admitted, ClassifyGuard<'a>), EngineError> {
        let ctx = self.engine.ctx.clone();
        let policy = self.engine.policy.clone();
        self.engine
            .classify_bracket(self.ws, tag, async move {
                let pipeline = Pipeline::new(&ctx);
                accept(&pipeline, &sql, policy.as_ref(), &who)
                    .await
                    .map_err(EngineError::Refused)
            })
            .await
    }

    /// Cancel the in-flight run/explain — or the statement still being classified — **iff** it
    /// is still run `tag` (a stale cancel can't abort a just-started newer run). Returns the
    /// elapsed time when something was actually cancelled; the awaiting [`run`](Self::run) /
    /// [`query`](Self::query) / [`explain`](Self::explain) settles
    /// [`StopReason::Cancelled`](crate::StopReason::Cancelled).
    ///
    /// A press can land in the window in front of dispatch, before anything is registered and
    /// before a snapshot is minted, so stopping one there is dropping the entry and aborting its
    /// task — the same settle a dispatched run gets.
    ///
    /// The `tag` — the UI's per-press nonce — is exactly right here, and the one place it
    /// is: the caller is asking to stop *the run it can see*, so if a repeat dispatch
    /// replaced the in-flight entry under the same tag, stopping that one is what the
    /// press meant.
    pub fn cancel(self, tag: RunTag) -> Option<u128> {
        let mut lc = self.engine.lifecycle.lock().unwrap();
        let mut elapsed = None;
        if lc.inflight.get(&self.ws).map(|f| f.tag) == Some(tag) {
            let f = lc.inflight.remove(&self.ws).unwrap();
            elapsed = Some(f.start.elapsed().as_millis());
            self.engine.abort_inflight(f);
        }
        lc.classifying.retain(|(ws, _), pending| {
            if *ws == self.ws && pending.tag == tag {
                elapsed = Some(pending.start.elapsed().as_millis());
                pending.abort.abort();
                false
            } else {
                true
            }
        });
        self.engine.publish_inflight(&lc);
        elapsed
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
        lc.classifying.retain(|(ws, _), pending| {
            if *ws == self.ws {
                pending.abort.abort();
                false
            } else {
                true
            }
        });
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
        lc.inflight.contains_key(&self.ws) || lc.classifying.keys().any(|(ws, _)| *ws == self.ws)
    }
}
