//! **Agent access, the app-wide half** (AA-03) — the service directory a project window lends
//! itself to, the vocabulary of what the server may ask it, and the server's own lifecycle.
//!
//! `docs/AGENT_ACCESS_SPEC.md` §4 is the design and every hop in it was verified against the
//! fork before any of this was written. In one paragraph: the server lives on its own Tokio
//! runtime (rmcp needs a reactor; the UI thread is not one), tabs and the catalog are UI-thread
//! Radio state, and the seam between them is a `tokio::sync` channel — runtime-agnostic, so a
//! send from the server thread invokes the receiver-future's waker, which is Freya's
//! `FuturesWaker(EventLoopProxy)`, which wakes the winit loop. The engine facade already proved
//! the outbound direction (the UI awaits Tokio `JoinHandle`s); this is the inbound one.
//!
//! **The window's half lives with the window**, because that is what it is made of: the driver
//! is one of the project subtree's reconcilers
//! ([`state::agent`](crate::apps::project::state::agent), beside the diagnostics driver) and the
//! settle observers are invisible pins at its root
//! ([`views::agent_keeper`](crate::apps::project::views), beside the request keepers). Only the
//! four things that outlive any one window are here:
//!
//! - [`directory`] — the cross-thread service registry **and** the app's `Host` impl over it.
//! - [`ask`] — what travels the control plane.
//! - [`server`] — start / stop, off the `agent_access` setting.
//! - [`status`] — the header's dot: listening, and whether anything is paired with it.
//!
//! **Nothing in AA-03 is a second results pipeline.** An agent `run` is an ordinary press: it
//! sets the tab's `QuerySpec` on `Chan::Request(id)` and everything downstream — freya-query
//! cache identity, snapshot materialization, supersede and retire, the tab's own request
//! keeper, history, the event log, the T2 close confirm — happens because it is the same press
//! a person makes. The bridge adds *observers*, never a path of its own.

pub mod ask;
mod directory;
mod server;
mod status;

use std::sync::Arc;

use freya::prelude::State;

pub use directory::AgentDirectory;
pub use server::use_agent_server;
pub use status::{use_agent_enabled, AgentStatusDot};

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
