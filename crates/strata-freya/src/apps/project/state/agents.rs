//! The window's **agents** satellite (AA-03b) — what each connected agent is doing in this
//! project, and the record behind the sidebar's Agents pane.
//!
//! A context signal rather than a store, on the same terms as [`Log`](super::log::Log) and
//! [`History`](super::history::History): nothing needs surgical per-channel updates, because
//! one append wakes exactly one reader (the pane, when it is mounted).
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
use strata_core::util::now_secs;

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

/// One query an agent ran.
#[derive(Clone, PartialEq, Debug)]
pub struct AgentRun {
    /// Append order — the row's list key, so a run arriving above another doesn't shuffle
    /// the rest through each other's scopes. Per satellite, which is all a key needs to be.
    pub seq: u64,
    pub sql: String,
    pub outcome: RunOutcome,
    /// Unix seconds at dispatch. Rendered through
    /// [`ago`](strata_core::util::ago) at paint, like the History drawer's cards — nothing
    /// re-renders this list on a clock.
    pub at: u64,
}

/// One agent's query session and what has run in it, newest first.
#[derive(Clone, PartialEq, Debug)]
pub struct QuerySession {
    pub id: QuerySessionId,
    /// What the pane calls it — `Session 1`, `Session 2`, per agent, in the order they were
    /// opened.
    ///
    /// A query session genuinely has no name (that is why `QuerySessionInfo` carries none),
    /// but a *list* of them needs rows a person can tell apart, and every alternative is
    /// worse: the handle is a uuid, the newest query repeats the card right below it, and a
    /// position shifts every time a session opens. So it is the tab strip's own answer —
    /// `next_query_name`'s monotonic counter — applied to the same problem.
    pub ordinal: usize,
    pub runs: VecDeque<AgentRun>,
}

impl QuerySession {
    /// Is this session's newest run still in flight? The satellite's own record of what the
    /// driver observed — `Engine::is_running` is the authority a *tool* is answered with
    /// (`state::agent::sessions`), and asking it here would put a second answer on screen.
    pub fn is_running(&self) -> bool {
        matches!(
            self.runs.front().map(|r| &r.outcome),
            Some(RunOutcome::Running)
        )
    }
}

/// One connected agent: what it calls itself, and the sessions it is working in.
#[derive(Clone, PartialEq, Debug)]
pub struct ConnectedAgent {
    pub id: AgentId,
    pub identity: AgentIdentity,
    /// **Oldest session first** (canvas): the list reads as the order the agent opened them,
    /// so the ordinals below run 1, 2, 3 down the pane. The *runs* inside a session are the
    /// other way round, newest first, because that is where "what is it doing now" is read.
    pub sessions: VecDeque<QuerySession>,
    /// The ordinal the next session gets. Monotonic and never reused, so closing session 2
    /// does not rename session 3 out from under whoever was reading it.
    next_ordinal: usize,
}

impl ConnectedAgent {
    /// What the pane's row calls this agent.
    ///
    /// A client is not obliged to introduce itself, and `clientInfo` is the only thing it ever
    /// tells us — so an empty name gets a plain stand-in rather than a blank row.
    pub fn name(&self) -> &str {
        match self.identity.name.trim() {
            "" => "Agent",
            name => name,
        }
    }

    /// The row's tooltip — name and version — or `None` when the client named no version, in
    /// which case a tooltip would only repeat the row.
    ///
    /// The canvas's title also said "· connected"; every agent in this pane is, by
    /// construction, which is the same tautology that removed the row's status dot.
    pub fn detail(&self) -> Option<String> {
        match self.identity.version.trim() {
            "" => None,
            version => Some(format!("{} {version}", self.name())),
        }
    }
}

/// Every agent connected to **this** project window.
///
/// **Only connected agents are here**, which is the pane's whole premise (canvas): a client that
/// disconnects takes its query sessions with it, so this answers "what is working on my project
/// right now" rather than becoming a second history. It is also why no row wears a
/// connected/disconnected mark — a mark with one possible value is decoration implying a
/// distinction the data does not carry, the same reasoning that left the History drawer's cards
/// without a status dot.
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
                    sessions: VecDeque::new(),
                    next_ordinal: 1,
                });
                0
            }
        };
        let ordinal = self.agents[at].next_ordinal;
        self.agents[at].next_ordinal += 1;
        let held = &mut self.agents[at].sessions;
        held.push_back(QuerySession {
            id: session,
            ordinal,
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
    /// anything — so without this a disconnect re-renders the pane in windows that never saw
    /// the agent.
    pub fn knows(&self, agent: AgentId) -> bool {
        self.agents.iter().any(|a| a.id == agent)
    }

    /// Is `session` one this agent holds? The **whole** ownership check, in one place: an
    /// agent addressing a session it does not own is answered exactly as it would be for one
    /// that never existed.
    pub fn holds(&self, agent: AgentId, session: QuerySessionId) -> bool {
        self.session(agent, session).is_some()
    }

    /// Drop one of `agent`'s sessions. `false` when it holds no such session.
    pub fn closed(&mut self, agent: AgentId, session: QuerySessionId) -> bool {
        let Some(held) = self.agents.iter_mut().find(|a| a.id == agent) else {
            return false;
        };
        let Some(at) = held.sessions.iter().position(|s| s.id == session) else {
            return false;
        };
        held.sessions.remove(at);
        true
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
    pub fn run_started(&mut self, agent: AgentId, session: QuerySessionId, sql: String) {
        self.next_seq += 1;
        let seq = self.next_seq;
        let Some(held) = self.session_mut(agent, session) else {
            return;
        };
        held.runs.push_front(AgentRun {
            seq,
            sql,
            outcome: RunOutcome::Running,
            at: now_secs(),
        });
        while held.runs.len() > RUNS_PER_SESSION {
            held.runs.pop_back();
        }
    }

    /// That run settled. Matched on the sequence number the dispatch minted rather than on
    /// "the newest run", because a settle can land after the agent has already pressed on:
    /// resolving it positionally would stamp the outcome of one query onto another.
    pub fn run_settled(
        &mut self,
        agent: AgentId,
        session: QuerySessionId,
        seq: u64,
        outcome: RunOutcome,
    ) {
        let Some(held) = self.session_mut(agent, session) else {
            return;
        };
        if let Some(run) = held.runs.iter_mut().find(|r| r.seq == seq) {
            run.outcome = outcome;
        }
    }

    /// The sequence number the next [`run_started`](Self::run_started) will use — read
    /// *before* dispatch so the settle has something to match on.
    pub fn next_run(&self) -> u64 {
        self.next_seq + 1
    }

    /// Every connected agent, newest connection first — the projection the pane renders.
    pub fn agents(&self) -> impl Iterator<Item = &ConnectedAgent> {
        self.agents.iter()
    }

    /// How many agents are working in this project. The pane's empty state and the rail's
    /// dress ask this; there is no `is_empty` beside it, for `Log::len`'s reason.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Every query session in flight across every agent — what the close confirm asks the
    /// engine about to tell "an agent is running a query" from "you are".
    pub fn sessions(&self) -> Vec<QuerySessionId> {
        self.agents
            .iter()
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
        }
    }

    fn opened(agents: &mut Agents, who: &Agent) -> QuerySessionId {
        let session = QuerySessionId::new();
        agents.opened(who, session);
        session
    }

    /// A run is recorded in flight and then resolved in place, so the pane shows one row per
    /// query rather than one on dispatch and a second on settle.
    #[test]
    fn a_run_is_recorded_running_and_then_settled_in_place() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let session = opened(&mut agents, &who);

        let seq = agents.next_run();
        agents.run_started(who.id, session, "SELECT 1".into());
        let listed = agents.agents().next().unwrap();
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
        let listed = agents.agents().next().unwrap();
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
        agents.run_started(who.id, session, "SELECT slow".into());
        let second = agents.next_run();
        agents.run_started(who.id, session, "SELECT fast".into());
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

        let runs = &agents.agents().next().unwrap().sessions[0].runs;
        assert_eq!(runs[0].sql, "SELECT fast");
        assert_eq!(
            runs[0].outcome,
            RunOutcome::Rows {
                returned: 1,
                total: 1,
                elapsed_ms: 2
            }
        );
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
        assert!(!agents.closed(mine.id, not_ours));

        // And a write against a session the agent does not hold records nothing.
        agents.run_started(mine.id, not_ours, "SELECT 1".into());
        let listed: Vec<usize> = agents.agents().map(|a| a.sessions[0].runs.len()).collect();
        assert_eq!(listed, vec![0, 0]);
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

        assert_eq!(agents.len(), 1);
        assert_eq!(agents.sessions(), vec![kept]);
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
        for i in 0..RUNS_PER_SESSION + 3 {
            agents.run_started(who.id, session, format!("SELECT {i}"));
        }
        let runs = &agents.agents().next().unwrap().sessions[0].runs;
        assert_eq!(runs.len(), RUNS_PER_SESSION);
        assert_eq!(runs[0].sql, format!("SELECT {}", RUNS_PER_SESSION + 2));

        // Settle the trail first: eviction deliberately skips a session that is still
        // working (see `the_session_cap_skips_a_session_that_is_working`), and this test is
        // about the cap rather than that rule.
        let newest = agents.next_seq;
        agents.run_settled(who.id, session, newest, RunOutcome::Plan { analyze: false });
        let mut evicted = Vec::new();
        for _ in 0..SESSIONS_PER_AGENT {
            evicted.extend(agents.opened(&who, QuerySessionId::new()));
        }
        assert_eq!(
            agents.agents().next().unwrap().sessions.len(),
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
    /// for a cancellation the *pane's* bound performed.
    #[test]
    fn the_session_cap_skips_a_session_that_is_working() {
        let mut agents = Agents::default();
        let who = agent("claude-code");
        let busy = opened(&mut agents, &who);
        agents.run_started(who.id, busy, "SELECT slow".into());
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
            agents.run_started(who.id, session, "SELECT slow".into());
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

    /// A session's name is minted once and never reused, so closing one does not renumber the
    /// rest under a reader's eyes.
    #[test]
    fn session_ordinals_are_monotonic_per_agent() {
        let mut agents = Agents::default();
        let (a, b) = (agent("claude-code"), agent("codex"));
        let first = opened(&mut agents, &a);
        let second = opened(&mut agents, &a);
        opened(&mut agents, &b);

        let ordinals = |agents: &Agents, who: AgentId| -> Vec<usize> {
            agents
                .agents()
                .find(|held| held.id == who)
                .unwrap()
                .sessions
                .iter()
                .map(|s| s.ordinal)
                .collect()
        };
        // Oldest first, so the ordinals read 1, 2 down the pane.
        assert_eq!(ordinals(&agents, a.id), vec![1, 2]);
        // Per agent, so the second agent starts at one of its own.
        assert_eq!(ordinals(&agents, b.id), vec![1]);

        agents.closed(a.id, first);
        assert_eq!(
            ordinals(&agents, a.id),
            vec![2],
            "session 2 is still session 2"
        );
        opened(&mut agents, &a);
        assert_eq!(ordinals(&agents, a.id), vec![2, 3], "and the next is 3");
        assert!(agents.holds(a.id, second));
    }

    /// A client that introduced itself is named, with its version in the tooltip; one that did
    /// not still gets a row a person can read, and no tooltip repeating it.
    #[test]
    fn an_agent_reads_as_something_even_unnamed() {
        let mut agents = Agents::default();
        let named = agent("claude-code");
        agents.opened(&named, QuerySessionId::new());
        agents.opened(
            &Agent {
                id: AgentId::new(),
                identity: AgentIdentity::default(),
            },
            QuerySessionId::new(),
        );

        let rows: Vec<(&str, Option<String>)> =
            agents.agents().map(|a| (a.name(), a.detail())).collect();
        // Agents stay **newest connection first** — a client that has just paired is the one
        // you are most likely looking for — while the sessions inside one run the other way.
        assert_eq!(
            rows,
            vec![
                ("Agent", None),
                ("claude-code", Some("claude-code 1.0".to_string())),
            ]
        );
    }
}
