//! The window's **agents** satellite (AA-03b) — which agents are working in this project, the
//! query sessions each holds, and what is in flight in them.
//!
//! It is the window's bookkeeping, not a surface: the ownership check every session-scoped
//! tool is answered through, the per-agent session cap, the teardown a retraction owes, and
//! the close confirm's "whose work is this". Nothing renders it.
//!
//! A context signal rather than a store, on the same terms as [`Log`](super::log::Log) and
//! [`History`](super::history::History): nothing needs surgical per-channel updates.
//!
//! ## Why it is not `SessionState`
//!
//! AA-03 put an agent's runs in the user's own `QueryTab`s, on the premise that the tab strip
//! *is* the investigation trail. That premise holds for someone watching the window and fails
//! for an MCP client, which is in a terminal on another desktop: twenty agent queries then
//! moved the editor out from under whoever was typing, left twenty tabs to close, and cost a
//! diagnostics pass each on the engine the user's own press was waiting for. So the runs
//! moved here, and the general rule they settled is worth keeping in view — **a surface's
//! state belongs to whoever is looking at that surface.** "Shared, last-writer-wins" is a
//! fine rule for *content* and a bad one for *attention*.
//!
//! Two consequences follow from being a satellite rather than the session, and both are the
//! point rather than a side effect:
//!
//! - **Nothing here reaches `session.json`.** `SessionSnapshot` is tabs, layout and geometry,
//!   and an agent owns no tabs — so reopening a project cannot restore work the user never
//!   asked for. Under AA-03 this was only half true: `QueryTab::agent` was kept out of
//!   `TabSnapshot` deliberately, but the *tab* persisted anyway.
//! - **Nothing here reaches `.strata/history.jsonl`.** History is capped at `max_history` and
//!   deduped before the cap (P3-14), so twenty exploratory agent queries would take twenty
//!   slots of the user's hundred and evict runs they actually made. History records what
//!   *the user* ran; promoting a row into a tab and pressing Run is what puts an agent's
//!   query in it, the ordinary way.
//!
//! ## Recorded by its observer
//!
//! No producer hook, for [`Log`](super::log::Log)'s reason: a run's outcome describes
//! something already finished and cannot be re-derived. The window's agent driver
//! (`state::agent`) watched every one of these facts — it took the ask that opened the
//! session and the notice that settled the run — so the driver is what appends.
//!
//! ## Everything here is bounded
//!
//! An agent is retracted when its connection ends, and a query session when it is closed or
//! its agent goes; a *run trail* has no such natural end, and neither does an agent that
//! opens sessions in a loop. So both are capped, oldest first — a trail is a scrollback, the
//! way the event log is.

use std::collections::VecDeque;

use freya::prelude::{use_provide_context, State};
use strata_agent::{Agent, AgentId, AgentIdentity, QuerySessionId};

use crate::agent::RunOutcome;

/// How many runs one query session keeps.
const RUNS_PER_SESSION: usize = 50;
/// How many query sessions one agent keeps. An agent that opens them in a loop is the only
/// way this is reached.
///
/// Eviction is **not** merely a display trim: the driver retires the evicted session's engine
/// workspace, so its handle stops answering and its settled result stops being readable. The
/// agent is told the plain not-found every stale handle gets. What eviction will never do is
/// take a session that is *working* — see [`Agents::opened`].
const SESSIONS_PER_AGENT: usize = 20;

/// One run an agent dispatched, and what became of it.
///
/// **The SQL is not here.** It was, for the pane that rendered it; with no surface left, a
/// trail of query text is memory the window holds and nothing can ever read. What the two
/// consumers below need is the sequence number a settle names its run by, and the outcome
/// [`is_running`](QuerySession::is_running) reads.
#[derive(Clone, PartialEq, Debug)]
pub struct AgentRun {
    /// Append order — and the key a settle matches on, so an agent that presses on before a
    /// slow query finishes cannot have the older outcome stamped onto the newer run. Per
    /// satellite, which is all a key needs to be.
    pub seq: u64,
    pub outcome: RunOutcome,
}

/// What closing a query session did.
///
/// Two answers rather than one, because a close can arrive while a run is being dispatched
/// into the very session it names — MCP permits concurrent requests on one connection, and
/// the dispatch is the *caller's* (`agent::directory`), bracketed by an ask and a notice. In
/// that window the engine has not been given the work yet, so tearing the workspace down
/// aborts and retires **nothing**, and the dispatch then lands on a `WsId` the satellite no
/// longer holds — an entry no later close, retraction or cap eviction can ever name again.
#[derive(PartialEq, Debug)]
pub enum Closed {
    /// Gone. Its engine workspace is the caller's to retire now.
    Now,
    /// Marked closed and kept, because a run is still in flight in it. The handle stops
    /// answering immediately ([`Agents::holds`] is already false), and the workspace is
    /// retired by whichever [`run_settled`](Agents::run_settled) lands last.
    WhenItSettles,
    /// This agent holds no such session.
    NoSuchSession,
}

/// One agent's query session and what has run in it, newest first.
#[derive(Clone, PartialEq, Debug)]
pub struct QuerySession {
    pub id: QuerySessionId,
    /// Closed by its agent while a run was still in flight — a tombstone, kept only until
    /// that run settles so its workspace can be torn down then. See [`Closed`].
    ///
    /// It stays in the list rather than being tracked beside it, and that is what reaps it:
    /// [`Agents::gone`] hands back every session an agent held, so a tombstone whose settle
    /// never arrives (a client that hung up mid-run, dropping the run future) goes when the
    /// connection does. The engine side of that case is already covered — `DispatchGuard`
    /// retires whatever a dropped run materialized.
    pub closing: bool,
    pub runs: VecDeque<AgentRun>,
}

impl QuerySession {
    /// Is **any** run in this session still in flight? The satellite's own record of what the
    /// driver observed — `Engine::is_running` is the authority a *tool* is answered with
    /// (`state::agent::sessions`), and asking it here would put a second answer beside it.
    ///
    /// Any, not the newest: MCP permits concurrent requests on one connection, so an agent
    /// can have two runs open in one session, and a fast second settling first would
    /// otherwise report the session idle while the first is still executing. Both consumers
    /// want the same reading — the eviction gate and the tombstone above — and each destroys
    /// work if it is wrong.
    pub fn is_running(&self) -> bool {
        self.runs.iter().any(|r| r.outcome == RunOutcome::Running)
    }
}

/// One connected agent: what it calls itself, and the sessions it is working in.
#[derive(Clone, PartialEq, Debug)]
pub struct ConnectedAgent {
    pub id: AgentId,
    pub identity: AgentIdentity,
    /// The app's own assistant rather than a client that dialled in (`Agent::in_app`, minted
    /// in `strata-agent` and delivered on the call that opens a session).
    ///
    /// It is **held like any other agent** — the ownership check, the per-agent session cap
    /// and the teardown on retraction all have to work for the assistant exactly as they do
    /// for an MCP client — and told apart only where the difference is a different sentence
    /// to the user: see [`Agents::sessions_of`].
    pub in_app: bool,
    /// **Oldest session first**: the list reads as the order the agent opened them. The *runs*
    /// inside a session are the other way round, newest first, because that is where "what is
    /// it doing now" is read.
    pub sessions: VecDeque<QuerySession>,
}

impl ConnectedAgent {
    /// What this agent is called — the name the event log attributes its runs to.
    ///
    /// A client is not obliged to introduce itself, and `clientInfo` is the only thing it ever
    /// tells us — so an empty name gets a plain stand-in rather than a blank row.
    pub fn name(&self) -> &str {
        match self.identity.name.trim() {
            "" => "Agent",
            name => name,
        }
    }
}

/// Every agent connected to **this** project window.
///
/// **Only connected agents are here**: a client that disconnects takes its query sessions with
/// it, so this answers "what is working on my project right now" rather than becoming a second
/// history.
#[derive(Default)]
pub struct Agents {
    agents: VecDeque<ConnectedAgent>,
    next_seq: u64,
}

impl Agents {
    /// Record a query session `agent` just opened, introducing the agent if this is the
    /// first thing it has done here.
    ///
    /// Returns whichever sessions were evicted to stay inside [`SESSIONS_PER_AGENT`], so the
    /// caller can tear their engine workspaces down — a display that forgets a session while
    /// the engine still holds its snapshot would leak exactly what the cap exists to stop.
    pub fn opened(&mut self, agent: &Agent, session: QuerySessionId) -> Vec<QuerySessionId> {
        let at = match self.agents.iter().position(|a| a.id == agent.id) {
            Some(at) => at,
            None => {
                self.agents.push_front(ConnectedAgent {
                    id: agent.id,
                    identity: agent.identity.clone(),
                    in_app: agent.in_app,
                    sessions: VecDeque::new(),
                });
                0
            }
        };
        let held = &mut self.agents[at].sessions;
        held.push_back(QuerySession {
            id: session,
            closing: false,
            runs: VecDeque::new(),
        });
        // Evict the oldest session that is **not working**, never the one just opened. The
        // caller tears the evicted workspace down, so taking a session with a run in flight
        // would abort it — and the engine settles an abort as `cancelled`, which the
        // vocabulary reports to the agent as "you stopped this" for a cancellation the app's
        // *display* cap performed. Handing back a handle that is already dead would be the
        // same trick played on the caller of this very call. If there is no idle older
        // session the list simply runs over: a bound on how much is shown must not be a
        // reason to destroy work.
        let mut evicted = Vec::new();
        while held.len() > SESSIONS_PER_AGENT {
            let older = held.len() - 1;
            let Some(at) = held.iter().take(older).position(|s| !s.is_running()) else {
                break;
            };
            if let Some(old) = held.remove(at) {
                evicted.push(old.id);
            }
        }
        evicted
    }

    /// Has this window ever lent anything to `agent`?
    ///
    /// Read before the retraction takes a write, because `agent_gone` is broadcast to every
    /// window and `State::write` wakes every subscriber whether or not the mutation changes
    /// anything — so without this a disconnect wakes every subscriber in windows that never
    /// saw the agent.
    pub fn knows(&self, agent: AgentId) -> bool {
        self.agents.iter().any(|a| a.id == agent)
    }

    /// Is `session` one this agent holds? The **whole** ownership check, in one place: an
    /// agent addressing a session it does not own is answered exactly as it would be for one
    /// that never existed.
    ///
    /// A tombstone (see [`Closed`]) is **not** held: it has been closed, so a second close or
    /// a fresh run against that handle gets the same not-found any stale handle does. That is
    /// also what stops a new run being dispatched into a session whose teardown is pending.
    pub fn holds(&self, agent: AgentId, session: QuerySessionId) -> bool {
        self.session(agent, session).is_some_and(|s| !s.closing)
    }

    /// Close one of `agent`'s sessions — or mark it closed, when a run is still in flight in
    /// it. See [`Closed`] for why the second answer exists.
    pub fn closed(&mut self, agent: AgentId, session: QuerySessionId) -> Closed {
        let Some(held) = self.agents.iter_mut().find(|a| a.id == agent) else {
            return Closed::NoSuchSession;
        };
        let Some(at) = held.sessions.iter().position(|s| s.id == session) else {
            return Closed::NoSuchSession;
        };
        // Already a tombstone: closed once, so this is the stale handle again.
        if held.sessions[at].closing {
            return Closed::NoSuchSession;
        }
        if held.sessions[at].is_running() {
            held.sessions[at].closing = true;
            return Closed::WhenItSettles;
        }
        held.sessions.remove(at);
        Closed::Now
    }

    /// The agent's connection ended: drop it, and hand back every session it was holding so
    /// the caller can retire their engine workspaces.
    pub fn gone(&mut self, agent: AgentId) -> Vec<QuerySessionId> {
        let Some(at) = self.agents.iter().position(|a| a.id == agent) else {
            return Vec::new();
        };
        match self.agents.remove(at) {
            Some(held) => held.sessions.iter().map(|s| s.id).collect(),
            None => Vec::new(),
        }
    }

    /// A run has been dispatched into `session`. Ignored when the agent holds no such
    /// session, which the caller has already refused — so this cannot invent a row.
    pub fn run_started(&mut self, agent: AgentId, session: QuerySessionId) {
        self.next_seq += 1;
        let seq = self.next_seq;
        let Some(held) = self.session_mut(agent, session) else {
            return;
        };
        held.runs.push_front(AgentRun {
            seq,
            outcome: RunOutcome::Running,
        });
        while held.runs.len() > RUNS_PER_SESSION {
            held.runs.pop_back();
        }
    }

    /// That run settled. Matched on the sequence number the dispatch minted rather than on
    /// "the newest run", because a settle can land after the agent has already pressed on:
    /// resolving it positionally would stamp the outcome of one query onto another.
    ///
    /// Returns the session when this settle was the **last** thing a tombstone was waiting
    /// for, so the caller can retire the engine workspace it deferred — the other half of
    /// [`Closed::WhenItSettles`]. `None` on every ordinary settle, which is all of them until
    /// a close races a dispatch.
    pub fn run_settled(
        &mut self,
        agent: AgentId,
        session: QuerySessionId,
        seq: u64,
        outcome: RunOutcome,
    ) -> Option<QuerySessionId> {
        let held = self.session_mut(agent, session)?;
        if let Some(run) = held.runs.iter_mut().find(|r| r.seq == seq) {
            run.outcome = outcome;
        }
        // A tombstone with a second run still open keeps waiting: the workspace belongs to
        // whichever settles last, not to whichever settles first.
        if !held.closing || held.is_running() {
            return None;
        }
        self.remove(agent, session);
        Some(session)
    }

    /// Drop a session from its agent's list, wherever it sits.
    fn remove(&mut self, agent: AgentId, session: QuerySessionId) {
        let Some(held) = self.agents.iter_mut().find(|a| a.id == agent) else {
            return;
        };
        if let Some(at) = held.sessions.iter().position(|s| s.id == session) {
            held.sessions.remove(at);
        }
    }

    /// The sequence number the next [`run_started`](Self::run_started) will use — read
    /// *before* dispatch so the settle has something to match on.
    pub fn next_run(&self) -> u64 {
        self.next_seq + 1
    }

    /// Every agent this satellite holds, newest connection first — the assistant included, for
    /// the bookkeeping that must not care where an agent came from (attribution in the event
    /// log).
    pub fn held(&self) -> impl Iterator<Item = &ConnectedAgent> {
        self.agents.iter()
    }

    /// Every query session in flight for agents on one side of the line the app draws between
    /// its own assistant and the clients that dialled in: `false` is the MCP clients, `true` is
    /// the assistant.
    ///
    /// What the close confirm asks the engine about to tell "an agent is running a query" from
    /// "you are" — and it takes a side because it also has to tell both from "the assistant
    /// is", which is a different sentence: "an agent" would send the user looking for a client
    /// that is not connected. The discriminator is [`ConnectedAgent::in_app`], the flag the
    /// core mints; nothing compares an id or a name, so a client cannot claim its way onto the
    /// other side of the line.
    ///
    /// Tombstones included, deliberately: a session closed while its run was still being
    /// dispatched is one the engine may well still be executing, and the confirm's question
    /// is about work that is about to be destroyed, not about handles that still answer.
    pub fn sessions_of(&self, in_app: bool) -> Vec<QuerySessionId> {
        self.agents
            .iter()
            .filter(|a| a.in_app == in_app)
            .flat_map(|a| a.sessions.iter().map(|s| s.id))
            .collect()
    }

    fn session(&self, agent: AgentId, session: QuerySessionId) -> Option<&QuerySession> {
        self.agents
            .iter()
            .find(|a| a.id == agent)?
            .sessions
            .iter()
            .find(|s| s.id == session)
    }

    fn session_mut(
        &mut self,
        agent: AgentId,
        session: QuerySessionId,
    ) -> Option<&mut QuerySession> {
        self.agents
            .iter_mut()
            .find(|a| a.id == agent)?
            .sessions
            .iter_mut()
            .find(|s| s.id == session)
    }
}

/// The satellite in context.
pub type AgentsCtx = State<Agents>;

/// Stand this project's agents satellite up and provide it. Call once in the window root,
/// before the agent bridge that writes it.
pub fn use_init_agents() -> AgentsCtx {
    use_provide_context(|| State::create(Agents::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn opened(agents: &mut Agents, who: &Agent) -> QuerySessionId {
        let session = QuerySessionId::new();
        agents.opened(who, session);
        session
    }

    /// A run is recorded in flight and then resolved in place, so a session holds one entry
    /// per query rather than one on dispatch and a second on settle.
    #[test]
    fn a_run_is_recorded_running_and_then_settled_in_place() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);

        let seq = agents.next_run();
        agents.run_started(who.id, session);
        let listed = agents.held().next().unwrap();
        assert_eq!(listed.sessions[0].runs.len(), 1);
        assert_eq!(listed.sessions[0].runs[0].outcome, RunOutcome::Running);

        agents.run_settled(
            who.id,
            session,
            seq,
            RunOutcome::Rows {
                returned: 5,
                total: 5,
                elapsed_ms: 41,
            },
        );
        let listed = agents.held().next().unwrap();
        assert_eq!(listed.sessions[0].runs.len(), 1, "still one row");
        assert_eq!(
            listed.sessions[0].runs[0].outcome,
            RunOutcome::Rows {
                returned: 5,
                total: 5,
                elapsed_ms: 41
            }
        );
    }

    /// **A settle names its run.** An agent that presses on before the first run finishes
    /// would otherwise have the older outcome stamped onto the newer row, which is the whole
    /// reason the dispatch mints a sequence number instead of the settle taking `front()`.
    #[test]
    fn a_late_settle_lands_on_its_own_run_not_the_newest() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);

        let first = agents.next_run();
        agents.run_started(who.id, session);
        let second = agents.next_run();
        agents.run_started(who.id, session);
        assert_ne!(first, second);

        agents.run_settled(
            who.id,
            session,
            second,
            RunOutcome::Rows {
                returned: 1,
                total: 1,
                elapsed_ms: 2,
            },
        );
        agents.run_settled(
            who.id,
            session,
            first,
            RunOutcome::Stopped("superseded by a newer run".into()),
        );

        // Newest first, so the second run is the head — and it wears the outcome its own
        // `seq` was settled with, not the one that landed after it.
        let runs = &agents.held().next().unwrap().sessions[0].runs;
        assert_eq!(runs[0].seq, second);
        assert_eq!(
            runs[0].outcome,
            RunOutcome::Rows {
                returned: 1,
                total: 1,
                elapsed_ms: 2
            }
        );
        assert_eq!(runs[1].seq, first);
        assert!(matches!(runs[1].outcome, RunOutcome::Stopped(_)));
    }

    /// Ownership is the satellite's, and it is total: one agent can neither see nor address
    /// another's session.
    #[test]
    fn an_agent_holds_only_its_own_sessions() {
        let mut agents = Agents::default();
        let (mine, theirs) = (agent("claude-code"), agent("some-other-client"));
        let ours = opened(&mut agents, &mine);
        let not_ours = opened(&mut agents, &theirs);

        assert!(agents.holds(mine.id, ours));
        assert!(!agents.holds(mine.id, not_ours));
        assert_eq!(agents.closed(mine.id, not_ours), Closed::NoSuchSession);

        // And a write against a session the agent does not hold records nothing.
        agents.run_started(mine.id, not_ours);
        let listed: Vec<usize> = agents.held().map(|a| a.sessions[0].runs.len()).collect();
        assert_eq!(listed, vec![0, 0]);
    }

    /// An idle session closes outright, and its workspace is the caller's to retire now.
    #[test]
    fn closing_an_idle_session_is_done_immediately() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);

        assert_eq!(agents.closed(who.id, session), Closed::Now);
        assert!(!agents.holds(who.id, session));
        assert!(
            agents.sessions_of(false).is_empty(),
            "nothing is left behind"
        );
    }

    /// **A close that races a dispatch does not orphan the workspace.** This is the AA-03c
    /// defect: a `RunStarting` is answered, and the close lands before the caller reaches
    /// `engine.query`. Tearing the workspace down there would abort nothing — the engine has
    /// not been given the work — and the dispatch would then register on a `WsId` no later
    /// close, retraction or cap eviction could ever name.
    ///
    /// So the close is a tombstone: the handle stops answering at once, and the teardown is
    /// handed to the settle.
    #[test]
    fn closing_a_session_mid_dispatch_defers_its_teardown_to_the_settle() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);
        let seq = agents.next_run();
        agents.run_started(who.id, session);

        assert_eq!(agents.closed(who.id, session), Closed::WhenItSettles);
        // Closed to the agent from this moment: nothing more can be dispatched into it, and
        // a second close is the same stale handle every other one is.
        assert!(!agents.holds(who.id, session));
        assert_eq!(agents.closed(who.id, session), Closed::NoSuchSession);
        // But the confirm still counts it, because the engine may still be executing it.
        assert_eq!(agents.sessions_of(false), vec![session]);

        // The settle is what retires it, and it names the session so the caller can.
        let retire = agents.run_settled(
            who.id,
            session,
            seq,
            RunOutcome::Rows {
                returned: 1,
                total: 1,
                elapsed_ms: 3,
            },
        );
        assert_eq!(retire, Some(session));
        assert!(agents.sessions_of(false).is_empty());
        assert_eq!(agents.held().count(), 1, "the agent itself stays connected");
    }

    /// The workspace belongs to whichever run settles **last**. MCP permits concurrent
    /// requests on one connection, so a tombstone can be waiting on two — and retiring on the
    /// first would abort the second, which the agent would then be told it had stopped
    /// itself.
    #[test]
    fn a_tombstone_waits_for_the_last_run_in_it() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);
        let slow = agents.next_run();
        agents.run_started(who.id, session);
        let fast = agents.next_run();
        agents.run_started(who.id, session);

        assert_eq!(agents.closed(who.id, session), Closed::WhenItSettles);
        assert_eq!(
            agents.run_settled(who.id, session, fast, RunOutcome::Plan { analyze: false }),
            None,
            "the slow one is still executing in that workspace"
        );
        assert_eq!(
            agents.run_settled(who.id, session, slow, RunOutcome::Plan { analyze: false }),
            Some(session)
        );
    }

    /// **The assistant is held like any other agent, and told apart by its flag.** Its
    /// sessions are owned, capped and torn down the same way, and `list_query_sessions` must
    /// still answer for it — what the flag buys is the close confirm's ability to say "the
    /// assistant is running a query" rather than "an agent is", which would send the user
    /// looking for a client that is not connected.
    #[test]
    fn the_in_app_assistant_is_held_and_told_apart_by_its_flag() {
        let mut agents = Agents::default();
        let dialled = agent("claude-code");
        let assistant = Agent {
            id: AgentId::new(),
            identity: AgentIdentity {
                name: "strata-assistant".into(),
                version: "0.2.0".into(),
            },
            in_app: true,
        };
        let (mine, theirs) = (QuerySessionId::new(), QuerySessionId::new());
        agents.opened(&dialled, theirs);
        agents.opened(&assistant, mine);

        // Held: both — so the assistant owns its session and can be retracted.
        assert_eq!(agents.held().count(), 2);
        assert!(agents.holds(assistant.id, mine));
        assert!(agents.knows(assistant.id));

        // Told apart: each side answers with its own sessions and nobody else's.
        assert_eq!(agents.sessions_of(false), vec![theirs]);
        assert_eq!(agents.sessions_of(true), vec![mine]);

        assert_eq!(agents.gone(assistant.id), vec![mine]);
    }

    /// A client that names itself after the assistant is still a client. The line keys on a
    /// flag the core mints, never on the identity, which is a claim made at `initialize`.
    #[test]
    fn claiming_the_assistants_name_does_not_move_a_client() {
        let mut agents = Agents::default();
        let liar = Agent {
            id: AgentId::new(),
            identity: AgentIdentity {
                name: "strata-assistant".into(),
                version: "0.2.0".into(),
            },
            in_app: false,
        };
        let theirs = QuerySessionId::new();
        agents.opened(&liar, theirs);

        assert_eq!(agents.sessions_of(false), vec![theirs]);
        assert!(agents.sessions_of(true).is_empty());
    }

    /// An ordinary settle retires nothing — the deferred teardown is the tombstone's alone.
    #[test]
    fn an_ordinary_settle_retires_nothing() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);
        let seq = agents.next_run();
        agents.run_started(who.id, session);

        assert_eq!(
            agents.run_settled(who.id, session, seq, RunOutcome::Plan { analyze: false }),
            None
        );
        assert!(
            agents.holds(who.id, session),
            "and the session is still open"
        );
    }

    /// **A tombstone whose settle never comes is reaped by the connection ending** — the case
    /// a deferred teardown has to answer for. A client that hangs up mid-run drops the run
    /// future, so no notice ever arrives; the session is still in the agent's list, so `gone`
    /// hands it back like any other.
    #[test]
    fn a_departed_agent_reaps_its_tombstones_too() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);
        agents.run_started(who.id, session);
        assert_eq!(agents.closed(who.id, session), Closed::WhenItSettles);

        assert_eq!(agents.gone(who.id), vec![session]);
    }

    /// A session is working while **any** run in it is, not merely its newest — a fast second
    /// run settling first must not report the session idle under a slow first one. Every
    /// consumer of this predicate destroys work when it is wrong: the cap evicts, and the
    /// tombstone retires.
    #[test]
    fn a_session_is_working_while_any_of_its_runs_is() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);
        let slow = agents.next_run();
        agents.run_started(who.id, session);
        let fast = agents.next_run();
        agents.run_started(who.id, session);

        agents.run_settled(who.id, session, fast, RunOutcome::Plan { analyze: false });
        let listed = agents.held().next().unwrap();
        assert!(
            listed.sessions[0].is_running(),
            "the newest settled, but the slow one is still in flight"
        );

        agents.run_settled(who.id, session, slow, RunOutcome::Plan { analyze: false });
        let listed = agents.held().next().unwrap();
        assert!(!listed.sessions[0].is_running());
    }

    /// A connection ending takes the agent and every session it held — and says which, so
    /// their engine workspaces can be retired rather than left holding snapshots.
    #[test]
    fn a_departed_agent_hands_back_its_sessions() {
        let mut agents = Agents::default();
        let (going, staying) = (agent("claude-code"), agent("codex"));
        let first = opened(&mut agents, &going);
        let second = opened(&mut agents, &going);
        let kept = opened(&mut agents, &staying);

        let mut released = agents.gone(going.id);
        released.sort_by_key(|s| s.0);
        let mut expected = vec![first, second];
        expected.sort_by_key(|s| s.0);
        assert_eq!(released, expected);

        assert_eq!(agents.held().count(), 1);
        assert_eq!(agents.sessions_of(false), vec![kept]);
        // Retracting twice is a no-op, not a panic — a `Drop` can race a close.
        assert!(agents.gone(going.id).is_empty());
    }

    /// Both caps hold, and the session cap hands its evictions back for the same reason
    /// `gone` does.
    #[test]
    fn the_trail_and_the_session_list_are_both_capped() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);
        let mut dispatched = Vec::new();
        for _ in 0..RUNS_PER_SESSION + 3 {
            dispatched.push(agents.next_run());
            agents.run_started(who.id, session);
        }
        let runs = &agents.held().next().unwrap().sessions[0].runs;
        assert_eq!(runs.len(), RUNS_PER_SESSION);
        // Oldest first out, and the trail is newest-first: the head is the last run
        // dispatched, and the three over the cap are the ones missing from the tail.
        assert_eq!(runs[0].seq, *dispatched.last().unwrap());
        assert_eq!(runs[RUNS_PER_SESSION - 1].seq, dispatched[3]);

        // Settle the **whole** trail first: eviction deliberately skips a session that is
        // still working (see `the_session_cap_skips_a_session_that_is_working`), and this
        // test is about the cap rather than that rule. Every row, not just the newest —
        // `is_running` is any run in flight, so one unsettled row would keep the session
        // working and it would never be evicted.
        for seq in 1..=agents.next_seq {
            agents.run_settled(who.id, session, seq, RunOutcome::Plan { analyze: false });
        }
        let mut evicted = Vec::new();
        for _ in 0..SESSIONS_PER_AGENT {
            evicted.extend(agents.opened(&who, QuerySessionId::new()));
        }
        assert_eq!(
            agents.held().next().unwrap().sessions.len(),
            SESSIONS_PER_AGENT
        );
        assert!(
            evicted.contains(&session),
            "the oldest session is named so its workspace can be retired"
        );
    }

    /// **The display cap never destroys work.** The driver retires whatever `opened` evicts,
    /// so taking a session with a run in flight would abort it — and the engine settles an
    /// abort as `cancelled`, which the vocabulary reports to the agent as "you stopped this"
    /// for a cancellation the *cap* performed.
    #[test]
    fn the_session_cap_skips_a_session_that_is_working() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let busy = opened(&mut agents, &who);
        agents.run_started(who.id, busy);
        let mut idle = Vec::new();
        for _ in 1..SESSIONS_PER_AGENT {
            idle.push(opened(&mut agents, &who));
        }

        let evicted = agents.opened(&who, QuerySessionId::new());

        assert_eq!(
            evicted,
            vec![idle[0]],
            "the oldest *idle* session goes, not the working one"
        );
        assert!(agents.holds(who.id, busy), "the running session is kept");
    }

    /// And when everything is working, nothing is evicted — the list runs briefly over rather
    /// than cancelling a query to stay inside a display bound.
    #[test]
    fn a_fully_busy_agent_evicts_nothing() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let mut held = Vec::new();
        for _ in 0..SESSIONS_PER_AGENT {
            let session = opened(&mut agents, &who);
            agents.run_started(who.id, session);
            held.push(session);
        }

        assert!(agents.opened(&who, QuerySessionId::new()).is_empty());
        assert!(held.iter().all(|s| agents.holds(who.id, *s)));
    }

    /// `knows` is the peek the driver takes before a notifying write, so a broadcast
    /// retraction cannot wake windows that never lent the agent anything.
    #[test]
    fn a_window_only_knows_agents_it_lent_something_to() {
        let mut agents = Agents::default();
        let mine = agent("claude-code");
        agents.opened(&mine, QuerySessionId::new());

        assert!(agents.knows(mine.id));
        assert!(!agents.knows(AgentId::new()));
        agents.gone(mine.id);
        assert!(
            !agents.knows(mine.id),
            "and stops knowing it once retracted"
        );
    }

    /// A client that introduced itself is named; one that did not still reads as something,
    /// because the name is what the event log attributes a run to.
    #[test]
    fn an_agent_reads_as_something_even_unnamed() {
        let mut agents = Agents::default();
        let named = agent("claude-code");
        agents.opened(&named, QuerySessionId::new());
        agents.opened(
            &Agent {
                id: AgentId::new(),
                identity: AgentIdentity::default(),
                in_app: false,
            },
            QuerySessionId::new(),
        );

        let names: Vec<&str> = agents.held().map(ConnectedAgent::name).collect();
        // Agents stay **newest connection first**, while the sessions inside one run the
        // other way.
        assert_eq!(names, vec!["Agent", "claude-code"]);
    }
}
