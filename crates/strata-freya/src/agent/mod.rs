//! **Agent access, the app-wide half** (AA-03) — the service directory a project window lends
//! itself to, the vocabulary of what the server may ask it, and the server's own lifecycle.
//!
//! Every hop of this design was verified against the fork before any of it was written.
//! In one paragraph: the server lives on its own Tokio
//! runtime (rmcp needs a reactor; the UI thread is not one), tabs and the catalog are UI-thread
//! Radio state, and the seam between them is a `tokio::sync` channel — runtime-agnostic, so a
//! send from the server thread invokes the receiver-future's waker, which is Freya's
//! `FuturesWaker(EventLoopProxy)`, which wakes the winit loop. The engine facade already proved
//! the outbound direction (the UI awaits Tokio `JoinHandle`s); this is the inbound one.
//!
//! **The window's half lives with the window**, because that is what it is made of: the driver
//! is one of the project subtree's reconcilers
//! ([`state::agent`](crate::apps::project::state::agent), beside the diagnostics driver), and
//! the record it writes is a satellite beside the event log
//! ([`state::agents`](crate::apps::project::state::agents)). Only the three things that outlive
//! any one window are here:
//!
//! - [`directory`] — the cross-thread service registry **and** the app's `Host` impl over it.
//! - [`ask`] — what travels the control plane.
//! - [`server`] — start / stop, off the `agent_access` setting.
//!
//! **Nothing here is a second results pipeline.** An agent's `run` is dispatched by the
//! directory straight at the engine, on its query session's own `WsId` — a real execution with
//! the same snapshot materialization, supersede, retire and cancel a person's press gets, and
//! counted by the same engine-wide flag the T2 close confirm reads. What it deliberately does
//! **not** touch is anything of the user's: no tab, no `QuerySpec`, no diagnostics pass, and
//! neither `history.jsonl` nor `session.json` (AA-03b — `state::agents` says why). The window
//! only brackets the run.

pub mod ask;
mod directory;
mod server;

use std::sync::Arc;

use freya::prelude::State;

pub use ask::RunOutcome;
pub use directory::AgentDirectory;
pub use server::use_agent_server;

use server::Running;

/// The app's agent-access globals, created once in `main` and carried on
/// [`AppCtx`](crate::state::AppCtx).
///
/// Two handles with two lifetimes, which is why they are a pair rather than one value. The
/// **directory** is the seam itself and lives for the process: windows join and leave it, and
/// it must outlive any of them so a server that is already listening keeps answering. The
/// **server slot** is what is listening right now, or nothing — dropping the value in it stops
/// the listener, terminates every live MCP session and shuts its runtime down, so "off" needs
/// no stop call to forget.
#[derive(Clone)]
pub struct AgentCtx {
    /// `pub(crate)` like the slot beside it: the directory's one consumer is the window's
    /// bridge, and the module doc above is the seam. A `pub` field would invite a second
    /// consumer reaching past it.
    pub(crate) directory: Arc<AgentDirectory>,
    pub(crate) server: State<Option<Running>>,
}

/// Both fields are handles on process-wide singletons created before the first window, so two
/// `AgentCtx`s are always the same two handles — which is what a component holding one as a
/// prop should conclude from a diff. Reactivity comes from what they *point at*, never from the
/// handle changing. Written out rather than derived because `Arc` compares its contents, and
/// `AgentDirectory` has none to compare.
impl PartialEq for AgentCtx {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.directory, &other.directory) && self.server == other.server
    }
}

/// Create the agent-access globals. Call **once**, in `main`, before `launch` — this is not a
/// hook. Nothing listens until [`use_agent_server`] finds the setting on.
pub fn create_global_agent() -> AgentCtx {
    AgentCtx {
        directory: Arc::new(AgentDirectory::default()),
        server: State::create_global(None),
    }
}
