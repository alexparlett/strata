//! The **service directory**: which project windows exist right now, from the server's
//! thread — and the app's [`Host`] impl over them.
//!
//! This is the one registry in the app that AGENTS.md §4's no-registry rule does *not*
//! govern, and the distinction is worth stating rather than assuming. That rule is about
//! **reactive UI state**: a `State<HashMap<TabId, …>>` threading every tab's data through one
//! value into every consumer, where context or a prop already expresses the relationship. This
//! is a **DI seam between threads** — the server has no scope, no context and no render pass,
//! so a directory is the only shape a lookup can take. It is the [`Windows`] registry's shape
//! (`platform::windows`) for the same reason, one thread further out.
//!
//! What a window lends it is exactly three things: its `Arc<Engine>` (the **data plane**) and
//! two senders — [`AgentAsk`] for the questions that touch Radio state, [`AgentNotice`] for
//! the facts that carry no answer. Registration is per *mount* of the project subtree, not
//! per window, which is what makes a re-root and an engine restart deregister and re-register
//! through the same mount/drop path rather than needing a cleanup route of their own.
//!
//! ## The run is dispatched here, not in the window (AA-03b)
//!
//! An agent's query goes straight to the engine against its query session's own `WsId`, which
//! is what makes the three costs of AA-03 disappear by construction rather than by
//! mitigation: no tab is opened, so nothing steals focus, nothing has to be closed, and the
//! window's diagnostics driver has nothing extra to validate on the engine the user's own
//! press is waiting for. What the window still owns is the half only it can answer — does
//! this agent hold this session, and record what it ran — which travels as
//! [`AgentAsk::RunStarting`] before the dispatch and [`AgentNotice::RunSettled`] after it.
//!
//! The run is still a **real** execution: same engine, same snapshot lifecycle, same
//! supersede and cancel. That was the half of AA-03's founding decision that was right, and
//! it is why an agent's result can be paged, sorted and promoted into a tab rather than being
//! a second, thinner pipeline.
//!
//! [`Windows`]: crate::platform::Windows

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use strata_agent::{
    Agent, AgentError, AgentId, CatalogEntry, Described, Host, Project, QuerySessionId,
    QuerySessionInfo, RunMode, RunSettle, Settled,
};
use strata_core::engine::plan::as_explain;
use strata_core::engine::{Engine, RunTag, WsId, CANCELLED};
use tokio::sync::{mpsc, oneshot};

use super::ask::{AgentAsk, AgentNotice, RunOutcome};

/// How many asks may be queued for one window before a tool call waits its turn.
///
/// The driver is serial and never awaits a run (the dispatch is the *caller's*, on the
/// engine), so this is a burst buffer rather than a backlog — a bound at all only because an
/// unbounded queue would let a client that never reads its answers grow the window's memory
/// without limit. A caller that fills it simply waits, which is the honest backpressure.
/// Notices are unbounded for the reason `ask.rs` gives.
const ASK_QUEUE: usize = 16;

/// One registration's identity, so a drop can only ever remove *its own* entry.
///
/// A counter rather than the project root, which looks like it would do: an engine restart
/// remounts the subtree at the **same** root, and although Freya orders the outgoing scope's
/// drop before the incoming mount, keying on the root would make that ordering load-bearing
/// for correctness rather than merely for tidiness — and the failure if it ever changed is
/// silent (the agent stops seeing a project that is right there).
///
/// **Lookup is the other half of that**, and it is why every `find` here walks in `rev()`:
/// registrations are pushed, so the newest match is last, and two entries sharing a root can
/// only ever be an outgoing mount and its replacement. Taking the first would hand the agent
/// the dead engine's `Arc` and a sender whose driver is being torn down — the same
/// ordering dependency this id exists to remove, left in place on the read path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RegId(u64);

/// One project window's lending: what it is called, where it lives, and the three channels
/// the server reaches it through.
struct Window {
    id: RegId,
    root: PathBuf,
    name: String,
    engine: Arc<Engine>,
    asks: mpsc::Sender<AgentAsk>,
    notices: mpsc::UnboundedSender<AgentNotice>,
}

/// Every project window an agent can address, plus the one app-wide number a tool needs
/// (`run`'s default page size).
///
/// Created once in `main` and shared by the server and every window (`super::AgentCtx`).
#[derive(Default)]
pub struct AgentDirectory {
    windows: Mutex<Vec<Window>>,
    /// Mirrors `Settings::row_limit` — see [`Host::default_page_size`], which is sync
    /// precisely because it must never become a question a window has to be awake to answer.
    page_size: AtomicUsize,
    next: AtomicU64,
    /// Nonces for the runs this directory dispatches. Engine-side lifecycle keys on its own
    /// dispatch id rather than on this (see `Engine::query`), so all a tag has to be is
    /// distinct from its neighbours' — which is also what makes an agent's `cancel` and a
    /// user's read the same way.
    runs: AtomicU64,
}

impl AgentDirectory {
    /// Lend this mount's project to the server, and take back the id that ends the loan.
    pub fn register(
        &self,
        root: PathBuf,
        name: String,
        engine: Arc<Engine>,
    ) -> (
        RegId,
        mpsc::Receiver<AgentAsk>,
        mpsc::UnboundedReceiver<AgentNotice>,
    ) {
        let (asks, ask_rx) = mpsc::channel(ASK_QUEUE);
        let (notices, notice_rx) = mpsc::unbounded_channel();
        let id = RegId(self.next.fetch_add(1, Ordering::Relaxed));
        self.windows.lock().unwrap().push(Window {
            id,
            root,
            name,
            engine,
            asks,
            notices,
        });
        (id, ask_rx, notice_rx)
    }

    /// End the loan. Idempotent, and matched on [`RegId`] rather than on the root.
    pub fn deregister(&self, id: RegId) {
        self.windows.lock().unwrap().retain(|w| w.id != id);
    }

    /// Mirror the app's default row limit — the number [`Host::default_page_size`] answers
    /// with. Written by the same effect that starts and stops the server, so a change in
    /// Settings lands without restarting anything.
    pub fn set_default_page_size(&self, rows: usize) {
        self.page_size.store(rows, Ordering::Relaxed);
    }

    /// Take what a call needs from `project`'s window, under **one** lock.
    ///
    /// Every reach for a window goes through here, and the `rev()` is why: registrations are
    /// pushed, so the newest match is last, and two entries sharing a root can only ever be an
    /// outgoing mount and its replacement (see [`RegId`]). Taking the first would hand the
    /// caller the dead engine's `Arc` and a sender whose driver is being torn down.
    ///
    /// One lock **per call**, not per handle: a caller that reached three times would be three
    /// independent lookups, and an engine restart landing between two of them remounts at the
    /// same root — so the call could execute on the outgoing engine while recording on the
    /// incoming window, or record a run's start on one satellite and deliver its settle to
    /// another (where it silently matches nothing, stranding the row at `Running`). Resolving
    /// once makes that unrepresentable rather than unlikely.
    fn window<T>(&self, project: &Path, take: impl FnOnce(&Window) -> T) -> Option<T> {
        self.windows
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|w| w.root == project)
            .map(take)
    }

    /// Tell **every** window a fact. An agent's connection is not project-scoped: it may
    /// hold query sessions in several windows and none of them outlives it.
    fn notify_all(&self, notice: impl Fn() -> AgentNotice) {
        for window in self.windows.lock().unwrap().iter() {
            let _ = window.notices.send(notice());
        }
    }

    /// Put one question to `project` and wait for its answer.
    ///
    /// Every way this can fail is the same fact — the window went — and there are three of
    /// them: it was never in the directory, the driver's receiver has already dropped, or the
    /// reply channel dropped with the scope that held it. A re-root and a close are
    /// indistinguishable here and should be: what the agent needs to know is that the window
    /// it was addressing is not the one in front of the user any more.
    ///
    /// The sender is cloned out from under the lock rather than borrowed: the caller is about
    /// to `await`, and a `MutexGuard` held across an await is both a `!Send` future and a lock
    /// the UI thread could then block on.
    async fn ask<T>(
        &self,
        project: &Path,
        build: impl FnOnce(oneshot::Sender<T>) -> AgentAsk,
    ) -> Result<T, AgentError> {
        let asks = self
            .window(project, |w| w.asks.clone())
            .ok_or(AgentError::WindowGone)?;
        Self::send(&asks, build).await
    }

    /// Ask over an **already-resolved** sender — the half of [`ask`](Self::ask) a caller that
    /// resolved its window once needs, so it does not have to look the window up again.
    async fn send<T>(
        asks: &mpsc::Sender<AgentAsk>,
        build: impl FnOnce(oneshot::Sender<T>) -> AgentAsk,
    ) -> Result<T, AgentError> {
        let (tx, rx) = oneshot::channel();
        asks.send(build(tx))
            .await
            .map_err(|_| AgentError::WindowGone)?;
        rx.await.map_err(|_| AgentError::WindowGone)
    }
}

impl Host for AgentDirectory {
    async fn projects(&self) -> Vec<Project> {
        self.windows
            .lock()
            .unwrap()
            .iter()
            .map(|w| Project {
                name: w.name.clone(),
                root: w.root.clone(),
            })
            .collect()
    }

    fn default_page_size(&self) -> usize {
        self.page_size.load(Ordering::Relaxed)
    }

    async fn engine(&self, project: &Path) -> Result<Arc<Engine>, AgentError> {
        self.window(project, |w| Arc::clone(&w.engine))
            .ok_or(AgentError::WindowGone)
    }

    async fn catalog(&self, project: &Path) -> Result<Vec<CatalogEntry>, AgentError> {
        self.ask(project, AgentAsk::Catalog).await
    }

    async fn describe(&self, project: &Path, name: &str) -> Result<Described, AgentError> {
        let name = name.to_string();
        self.ask(project, |reply| AgentAsk::Describe { name, reply })
            .await?
    }

    async fn query_sessions(
        &self,
        project: &Path,
        agent: AgentId,
    ) -> Result<Vec<QuerySessionInfo>, AgentError> {
        self.ask(project, |reply| AgentAsk::QuerySessions { agent, reply })
            .await
    }

    async fn open_query_session(
        &self,
        project: &Path,
        agent: &Agent,
    ) -> Result<QuerySessionId, AgentError> {
        let agent = agent.clone();
        self.ask(project, |reply| AgentAsk::OpenQuerySession { agent, reply })
            .await
    }

    async fn close_query_session(
        &self,
        project: &Path,
        agent: AgentId,
        session: QuerySessionId,
    ) -> Result<(), AgentError> {
        self.ask(project, |reply| AgentAsk::CloseQuerySession {
            agent,
            session,
            reply,
        })
        .await?
    }

    /// Dispatch on the engine, with the window bracketing it.
    ///
    /// The order is the whole design. `RunStarting` first, because it is both the ownership
    /// check (only the window knows whose session this is) and the record of what is running
    /// — and it hands back the sequence number the settle names, so a slow query's outcome
    /// cannot land on a faster one the agent pressed after it. Then the engine, awaited here
    /// on the server's own runtime rather than parked against a press's nonce the way AA-03
    /// had to. Then the settle, as a notice, because by then there is nobody left to refuse
    /// it.
    async fn run(
        &self,
        project: &Path,
        agent: AgentId,
        session: QuerySessionId,
        sql: String,
        mode: RunMode,
        page_size: usize,
    ) -> Result<RunSettle, AgentError> {
        // **One resolution for the whole bracket.** The engine that executes, the driver that
        // records the start and the driver that hears the settle are taken together, under one
        // lock, so a re-registration cannot land between them — see `window`. Resolving three
        // times (as this did) let an engine restart execute the run on the outgoing engine
        // while the incoming window recorded it, or deliver a settle to a satellite that had
        // never heard of the session, where it silently matched nothing.
        let Some((engine, asks, notices)) = self.window(project, |w| {
            (Arc::clone(&w.engine), w.asks.clone(), w.notices.clone())
        }) else {
            return Err(AgentError::WindowGone);
        };

        let started = Self::send(&asks, |reply| AgentAsk::RunStarting {
            agent,
            session,
            sql: sql.clone(),
            reply,
        })
        .await??;

        let ws = WsId::from(session);
        let tag = RunTag(self.runs.fetch_add(1, Ordering::Relaxed) as u128);
        // **A dropped run still settles** (AS-04). Dropping this future is how a caller cancels
        // — the assistant's stop drops the whole turn, and an MCP client hanging up mid-run does
        // the same — and until this guard existed the settle below simply never ran, leaving the
        // satellite's row on `Running` for the rest of the window's life. AA-03c reaps such a
        // row when a *connection* ends, which covers a client that disconnects and nothing else:
        // the assistant's connection is the pane's whole mount, so its stopped runs would sit
        // there until the project closed.
        //
        // The guard's message is the engine's own `CANCELLED`, not a word invented here, so the
        // row reads exactly as a cancelled press does (AGENTS.md §2 — a stop is not a failure,
        // and only `stopped_on_purpose` knows which is which). It is **disarmed** on the normal
        // path, because a run that finished sends its real outcome one line further down.
        let mut cancelled = SettleOnDrop {
            armed: true,
            notices: notices.clone(),
            agent,
            session,
            seq: started,
        };
        let settled = match mode {
            RunMode::Run => engine
                .query(ws, tag, sql, page_size)
                .await
                .map(|(output, _)| Settled::Rows(output)),
            // Wrapped here, exactly as the app's own Run capability does it: `Explain` means
            // "plan this statement", never "the caller already wrote EXPLAIN" — and never
            // `analyze`, which would execute the query the caller was avoiding.
            RunMode::Explain => engine
                .explain(ws, tag, as_explain(&sql, false))
                .await
                .map(Settled::Plan),
        };

        // Straight down the sender the start was answered on — not a fresh lookup, which is
        // what could deliver this to a different mount.
        //
        // **A settle nobody is left to hear is dropped, deliberately.** If the window closed
        // while the query was executing, this send fails and the agent still gets its rows —
        // which is the honest answer, because the query really did run. Answering
        // `WindowGone` instead was considered and refused: it would report a fault for a
        // statement that succeeded, and the agent's recovery from a window loss is to run it
        // again, so it would pay for the same scan twice. Nothing is stranded by the silence
        // either — the satellite that would have shown the row went with the window. The one
        // case where a settle could have landed on a *live* satellite that never heard the
        // start is the same-root remount, and resolving the whole bracket once (above) is
        // what makes that unrepresentable.
        cancelled.armed = false;
        let _ = notices.send(AgentNotice::RunSettled {
            agent,
            session,
            seq: started,
            outcome: RunOutcome::of(&settled),
        });
        Ok(settled)
    }

    /// Sync and non-blocking, as the trait requires: an unbounded send, which is exactly
    /// what a `Drop` on the transport's runtime can afford.
    fn agent_gone(&self, agent: AgentId) {
        self.notify_all(|| AgentNotice::AgentGone(agent));
    }
}

/// **The settle a cancelled run still owes its window.**
///
/// A `Drop` rather than a `select!` arm, because there is nothing to select *on*: a caller
/// cancels by dropping the future, which never resumes to run a cleanup branch of its own.
/// Disarmed the moment the real outcome is known, so the two paths are exclusive by
/// construction rather than by the receiver deduplicating on `seq`.
struct SettleOnDrop {
    armed: bool,
    notices: mpsc::UnboundedSender<AgentNotice>,
    agent: AgentId,
    session: QuerySessionId,
    seq: u64,
}

impl Drop for SettleOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = self.notices.send(AgentNotice::RunSettled {
            agent: self.agent,
            session: self.session,
            seq: self.seq,
            outcome: RunOutcome::Stopped(CANCELLED.to_string()),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use futures::executor::block_on;
    use futures::future::join;
    use strata_agent::{AgentIdentity, QuerySessionState};

    use super::*;

    /// The directory's futures are executor-agnostic — that is the whole property the bridge
    /// rests on — so a plain `block_on` stands in for both the server's Tokio runtime and
    /// Freya's UI executor, and a hand-rolled responder stands in for the driver.
    type Lent = (
        mpsc::Receiver<AgentAsk>,
        mpsc::UnboundedReceiver<AgentNotice>,
    );

    fn window(directory: &AgentDirectory, name: &str, root: &str) -> Lent {
        let (_, asks, notices) = directory.register(
            PathBuf::from(root),
            name.into(),
            Arc::new(Engine::new(BTreeMap::new())),
        );
        (asks, notices)
    }

    fn agent(name: &str) -> Agent {
        Agent {
            id: AgentId::new(),
            identity: AgentIdentity {
                name: name.into(),
                version: "1.0".into(),
            },
            in_app: false,
        }
    }

    #[test]
    fn an_ask_reaches_the_registered_window_and_its_answer_comes_back() {
        let directory = AgentDirectory::default();
        let (mut asks, _notices) = window(&directory, "sales", "/w/sales");
        let who = agent("claude-code");
        let answered = QuerySessionId::new();

        let (opened, ()) = block_on(join(
            directory.open_query_session(Path::new("/w/sales"), &who),
            async {
                let Some(AgentAsk::OpenQuerySession { agent, reply }) = asks.recv().await else {
                    panic!("expected an open-query-session ask");
                };
                // The identity travels with the ask, because opening is when the window first
                // has anything of this agent's to show.
                assert_eq!(agent.identity.name, "claude-code");
                let _ = reply.send(answered);
            },
        ));

        assert_eq!(opened.unwrap(), answered);
    }

    /// A window that answers can still refuse: the ask's own `Result` is what carries "no such
    /// query session", as against the directory's "no such window".
    #[test]
    fn a_windows_own_refusal_travels_back_as_itself() {
        let directory = AgentDirectory::default();
        let (mut asks, _notices) = window(&directory, "sales", "/w/sales");
        let session = QuerySessionId::new();

        let (closed, ()) = block_on(join(
            directory.close_query_session(Path::new("/w/sales"), AgentId::new(), session),
            async {
                let Some(AgentAsk::CloseQuerySession { reply, .. }) = asks.recv().await else {
                    panic!("expected a close ask");
                };
                let _ = reply.send(Err(AgentError::no_such_query_session(session)));
            },
        ));

        assert!(matches!(closed, Err(AgentError::NotFound(_))));
    }

    /// **Three ways to lose a window, one answer.** Never registered, deregistered (the
    /// `use_drop` a close or a re-root runs), and registered but with its driver gone — an
    /// agent needs to know the window it was addressing is not the one in front of the user,
    /// and which of the three it was is a distinction with nothing behind it.
    #[test]
    fn every_way_of_losing_a_window_answers_the_same_thing() {
        let directory = AgentDirectory::default();
        assert!(matches!(
            block_on(directory.catalog(Path::new("/w/nope"))),
            Err(AgentError::WindowGone)
        ));

        let (id, asks, _notices) = directory.register(
            PathBuf::from("/w/sales"),
            "sales".into(),
            Arc::new(Engine::new(BTreeMap::new())),
        );
        // The driver's end goes, the entry does not: a send into a closed channel is the
        // same fact one step later.
        drop(asks);
        assert!(matches!(
            block_on(directory.catalog(Path::new("/w/sales"))),
            Err(AgentError::WindowGone)
        ));

        directory.deregister(id);
        assert!(matches!(
            block_on(directory.query_sessions(Path::new("/w/sales"), AgentId::new())),
            Err(AgentError::WindowGone)
        ));
        assert!(block_on(directory.projects()).is_empty());
    }

    /// A reply channel that drops without answering is the same answer again, rather than a
    /// hang — here on the ask that gates a run, which is the one a client waits longest on.
    #[test]
    fn a_reply_dropped_unanswered_is_a_window_gone_and_not_a_hang() {
        let directory = AgentDirectory::default();
        let (mut asks, _notices) = window(&directory, "sales", "/w/sales");

        let (settled, ()) = block_on(join(
            directory.run(
                Path::new("/w/sales"),
                AgentId::new(),
                QuerySessionId::new(),
                "SELECT 1".into(),
                RunMode::Run,
                100,
            ),
            async {
                // Taken, and then dropped with the scope that held it.
                drop(asks.recv().await);
            },
        ));

        assert!(matches!(settled, Err(AgentError::WindowGone)));
    }

    /// **A run is gated before it is dispatched.** The window is the only thing that knows
    /// whose session a handle names, so a refusal there must stop the engine ever being asked
    /// — which is also what keeps an agent from running against another agent's workspace.
    #[test]
    fn a_refused_run_never_reaches_the_engine() {
        let directory = AgentDirectory::default();
        let (mut asks, mut notices) = window(&directory, "sales", "/w/sales");
        let session = QuerySessionId::new();

        let (settled, ()) = block_on(join(
            directory.run(
                Path::new("/w/sales"),
                AgentId::new(),
                session,
                // Would fail loudly if it ever executed.
                "SELECT * FROM nothing_at_all".into(),
                RunMode::Run,
                100,
            ),
            async {
                let Some(AgentAsk::RunStarting { reply, .. }) = asks.recv().await else {
                    panic!("expected a run-starting ask");
                };
                let _ = reply.send(Err(AgentError::no_such_query_session(session)));
            },
        ));

        assert!(matches!(settled, Err(AgentError::NotFound(_))));
        // And nothing was recorded as settled, because nothing was recorded as started.
        assert!(notices.try_recv().is_err());
    }

    /// The bracket around a dispatched run: the window is told what started, then told what it
    /// came to — **naming the same run**, so a settle cannot land on a query the agent pressed
    /// after it.
    #[test]
    fn a_dispatched_run_is_reported_started_and_then_settled_under_one_sequence() {
        let directory = AgentDirectory::default();
        let (mut asks, mut notices) = window(&directory, "sales", "/w/sales");
        let who = AgentId::new();
        let session = QuerySessionId::new();

        let (settled, ()) = block_on(join(
            directory.run(
                Path::new("/w/sales"),
                who,
                session,
                "SELECT 1".into(),
                RunMode::Run,
                100,
            ),
            async {
                let Some(AgentAsk::RunStarting { sql, reply, .. }) = asks.recv().await else {
                    panic!("expected a run-starting ask");
                };
                assert_eq!(sql, "SELECT 1");
                let _ = reply.send(Ok(7));
            },
        ));

        assert!(settled.unwrap().is_ok(), "the engine ran it");
        let Some(AgentNotice::RunSettled {
            agent,
            session: settled_session,
            seq,
            outcome,
        }) = notices.try_recv().ok()
        else {
            panic!("the settle should have been reported");
        };
        assert_eq!((agent, settled_session, seq), (who, session, 7));
        assert!(matches!(
            outcome,
            RunOutcome::Rows {
                returned: 1,
                total: 1,
                ..
            }
        ));
    }

    /// **A run's bracket cannot span two mounts.** The engine that executes it, the driver that
    /// records its start and the driver that hears its settle are resolved together — so an
    /// engine restart landing mid-run (a remount at the *same* project root) cannot deliver the
    /// settle to a satellite that never heard of the session, where `run_settled` would match
    /// nothing and strand the row at `Running` forever.
    #[test]
    fn a_run_settles_into_the_registration_that_started_it() {
        let directory = AgentDirectory::default();
        let (id, mut asks, mut before) = directory.register(
            PathBuf::from("/w/sales"),
            "sales".into(),
            Arc::new(Engine::new(BTreeMap::new())),
        );
        let who = AgentId::new();
        let session = QuerySessionId::new();

        let mut after = None;
        let (settled, ()) = block_on(join(
            directory.run(
                Path::new("/w/sales"),
                who,
                session,
                "SELECT 1".into(),
                RunMode::Run,
                100,
            ),
            async {
                let Some(AgentAsk::RunStarting { reply, .. }) = asks.recv().await else {
                    panic!("expected a run-starting ask");
                };
                let _ = reply.send(Ok(7));
                // The restart: this mount goes and a fresh one takes the same root, exactly
                // as `ProjectLoaded` remounting does, while the run is still executing.
                directory.deregister(id);
                let (_, _new_asks, new_notices) = directory.register(
                    PathBuf::from("/w/sales"),
                    "sales".into(),
                    Arc::new(Engine::new(BTreeMap::new())),
                );
                after = Some(new_notices);
            },
        ));

        assert!(settled.unwrap().is_ok(), "the engine still ran it");
        assert!(
            matches!(
                before.try_recv(),
                Ok(AgentNotice::RunSettled { seq: 7, .. })
            ),
            "the settle belongs to the registration that answered the start"
        );
        assert!(
            after
                .expect("the replacement registered")
                .try_recv()
                .is_err(),
            "and must not land on the mount that replaced it"
        );
    }

    /// A connection ending is broadcast to **every** window, because an agent may hold query
    /// sessions in several and none of them outlives it. Sync and non-blocking, which is what
    /// a `Drop` on the transport's runtime can afford.
    #[test]
    fn a_departed_agent_is_announced_to_every_window() {
        let directory = AgentDirectory::default();
        let (_a_asks, mut a) = window(&directory, "sales", "/w/sales");
        let (_b_asks, mut b) = window(&directory, "ops", "/w/ops");
        let who = AgentId::new();

        directory.agent_gone(who);

        for notices in [&mut a, &mut b] {
            let Some(AgentNotice::AgentGone(gone)) = notices.try_recv().ok() else {
                panic!("every window hears it");
            };
            assert_eq!(gone, who);
        }
    }

    /// The root is the identity and the name is a label, which is what lets `host::resolve`
    /// report a colliding name rather than guess.
    #[test]
    fn projects_are_listed_by_root_and_name() {
        let directory = AgentDirectory::default();
        let _a = window(&directory, "data", "/a/data");
        let _b = window(&directory, "data", "/b/data");

        let listed = block_on(directory.projects());
        assert_eq!(
            listed.iter().map(|p| p.root.clone()).collect::<Vec<_>>(),
            vec![PathBuf::from("/a/data"), PathBuf::from("/b/data")]
        );
        assert!(listed.iter().all(|p| p.name == "data"));
    }

    /// `run`'s default page size is a number a `Host` answers **synchronously**, so it is
    /// mirrored here rather than asked of a window. Zero is the app's own "no limit" and is
    /// passed through untouched — resolving it is the tool layer's, once.
    #[test]
    fn the_default_page_size_is_mirrored_verbatim() {
        let directory = AgentDirectory::default();
        assert_eq!(directory.default_page_size(), 0);
        directory.set_default_page_size(250);
        assert_eq!(directory.default_page_size(), 250);
    }

    /// Nothing in the vocabulary reports a session state the directory invents: it comes back
    /// from the window verbatim. Pinned here because the enum is the one shape a well-meaning
    /// "unknown" arm could be added to.
    #[test]
    fn a_session_state_is_the_windows_answer_verbatim() {
        let directory = AgentDirectory::default();
        let (mut asks, _notices) = window(&directory, "sales", "/w/sales");
        let session = QuerySessionId::new();

        let (listed, ()) = block_on(join(
            directory.query_sessions(Path::new("/w/sales"), AgentId::new()),
            async {
                let Some(AgentAsk::QuerySessions { reply, .. }) = asks.recv().await else {
                    panic!("expected a sessions ask");
                };
                let _ = reply.send(vec![QuerySessionInfo {
                    session,
                    state: QuerySessionState::Running,
                }]);
            },
        ));

        let listed = listed.unwrap();
        assert_eq!(listed[0].session, session);
        assert!(matches!(listed[0].state, QuerySessionState::Running));
    }
}
