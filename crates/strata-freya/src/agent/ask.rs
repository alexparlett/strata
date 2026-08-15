//! What the server asks a project window for — the **control plane's** whole vocabulary.
//!
//! One [`AgentAsk`] variant per [`Host`](strata_agent::Host) method that touches UI state,
//! and each carries its own reply channel rather than a shared `Option<oneshot>`: an ask that
//! could be built without somewhere to answer is an ask a driver can silently drop, and the
//! caller's only symptom would be a tool call that never returns.
//!
//! Everything a tool can answer **without** UI state is deliberately absent — `fetch_page`,
//! `validate`, `functions()` and, since AA-03b, the run itself go straight to the window's
//! `Arc<Engine>` from the server's own runtime, so nothing bulk queues behind a repaint. The
//! directory hands that engine out; this channel is only for the questions a window has to
//! answer for itself.
//!
//! ## Two channels, because one producer cannot wait
//!
//! An [`AgentNotice`] is a fact with no answer, and it exists because [`AgentAsk`]'s bounded
//! channel cannot serve every producer. A `send().await` is right for a tool call — a caller
//! that fills the queue simply waits, which is honest backpressure — but the most important
//! notice of all is sent from a `Drop`: an MCP connection ending, which has nothing to await
//! on and nowhere to report a failure to. So notices ride an **unbounded** channel, whose
//! bound is really the client's own rate (one notice per run settled, one per connection
//! ended) and which a synchronous producer can always reach.
//!
//! Both are drained by the same serial loop, asks first, so a settle can never overtake the
//! dispatch it belongs to.

use strata_agent::{
    Agent, AgentError, AgentId, CatalogEntry, Described, QuerySessionId, QuerySessionInfo,
    RunSettle, Settled,
};
use strata_engine::stopped_on_purpose;
use tokio::sync::oneshot;

/// What one of an agent's runs came to.
///
/// A stop is its own arm rather than an error string, because it is the one distinction the
/// pane must not get wrong: `strata_engine::stopped_on_purpose` settles three strings
/// that are news the user already has, and painting one red would report a fault nobody had.
/// The driver asks that predicate once, on the way in.
#[derive(Clone, PartialEq, Debug)]
pub enum RunOutcome {
    /// Dispatched, still in flight.
    Running,
    Rows {
        /// How many rows came **back to the agent** — page 1, bounded by its `page_size`.
        returned: u64,
        /// How many the query actually matched. Exact: no `LIMIT` was injected to make it
        /// otherwise, which is why the pane can state both (`200 of 4,821 rows`) rather than
        /// one number that could mean either.
        total: u64,
        elapsed_ms: u64,
    },
    Plan {
        analyze: bool,
    },
    /// Cancelled or superseded — the engine's own wording.
    Stopped(String),
    Failed(String),
}

impl RunOutcome {
    /// What a settled run reads as in the pane.
    ///
    /// The **only** judgement here is `stopped_on_purpose`, and it is asked rather than
    /// re-derived: the engine settles three such strings (`cancelled`, and two supersedes),
    /// and the event log already learned once what a hand-rolled `== "cancelled"` costs — a
    /// red row reading "superseded by a newer run", a fault the user never had. Everything
    /// else `Err` keeps the engine's own message, which is the same text the results pane
    /// would frame if this query were promoted into a tab and re-run.
    pub fn of(settled: &RunSettle) -> RunOutcome {
        match settled {
            Ok(Settled::Rows(output)) => RunOutcome::Rows {
                returned: output.rows.len() as u64,
                total: output.total as u64,
                elapsed_ms: output.elapsed_ms as u64,
            },
            Ok(Settled::Plan(plan)) => RunOutcome::Plan {
                analyze: plan.analyze,
            },
            Err(e) if stopped_on_purpose(e) => RunOutcome::Stopped(e.clone()),
            Err(e) => RunOutcome::Failed(e.clone()),
        }
    }
}

/// One question for one project window, with the channel its answer comes back on.
///
/// The readonly listings ([`Catalog`](AgentAsk::Catalog),
/// [`QuerySessions`](AgentAsk::QuerySessions)) reply with a plain value: a window that is
/// there to receive the ask can always answer them, and a window that is not never sees it
/// (the send fails, or the reply channel drops, and both are [`AgentError::WindowGone`] at
/// the directory). The rest can fail on their own terms — no such table, no such query
/// session — so they carry a `Result`.
pub enum AgentAsk {
    /// The catalog as the store shows it, never DataFusion introspection (AGENTS.md §2).
    Catalog(oneshot::Sender<Vec<CatalogEntry>>),
    Describe {
        name: String,
        reply: oneshot::Sender<Result<Described, AgentError>>,
    },
    /// **This agent's** sessions. The window is the only thing that knows whose is whose.
    QuerySessions {
        agent: AgentId,
        reply: oneshot::Sender<Vec<QuerySessionInfo>>,
    },
    /// Open a query session, introducing the agent if the window has not seen it before —
    /// which is the only moment an agent's `clientInfo` is needed, so it is the only ask
    /// carrying a whole [`Agent`].
    OpenQuerySession {
        agent: Agent,
        reply: oneshot::Sender<QuerySessionId>,
    },
    CloseQuerySession {
        agent: AgentId,
        session: QuerySessionId,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    /// A run is about to be dispatched **by the caller, on the engine directly**. This ask is
    /// not the run: it is the one half of it a window has to perform — check that the agent
    /// holds the session, and record a run as in flight in it.
    ///
    /// The reply carries the run's sequence number back, so the settle that follows can name
    /// the row it belongs to rather than taking whichever is newest: an agent that presses on
    /// before a slow query finishes would otherwise have the older outcome stamped onto the
    /// newer run.
    ///
    /// **It does not carry the SQL.** It did, for the pane that rendered it; the satellite
    /// records a run's `seq` and outcome and nothing else, so sending the text would be a
    /// clone per run for a reader that no longer exists. The engine still gets it — the
    /// dispatch is the caller's, and it never travelled this channel.
    RunStarting {
        agent: AgentId,
        session: QuerySessionId,
        reply: oneshot::Sender<Result<u64, AgentError>>,
    },
}

/// A fact about an agent, with no answer — see the module note on why these need a channel
/// of their own.
pub enum AgentNotice {
    /// The run started under `seq` settled. Judged already: the driver never re-reads
    /// `stopped_on_purpose`, which is asked in one place in the whole system.
    RunSettled {
        agent: AgentId,
        session: QuerySessionId,
        seq: u64,
        outcome: RunOutcome,
    },
    /// The agent's connection ended. Its sessions go with it, and their engine workspaces
    /// with them.
    AgentGone(AgentId),
}
