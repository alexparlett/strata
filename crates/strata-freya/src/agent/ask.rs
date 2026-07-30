//! What the server asks a project window for — the **control plane's** whole vocabulary.
//!
//! One variant per [`Host`](strata_agent::Host) method that touches UI state, and each
//! carries its own reply channel rather than a shared `Option<oneshot>`: an ask that could
//! be built without somewhere to answer is an ask a driver can silently drop, and the
//! caller's only symptom would be a tool call that never returns.
//!
//! Everything a tool can answer **without** UI state is deliberately absent — `fetch_page`,
//! `validate` and `functions()` go straight to the window's `Arc<Engine>` from the server's
//! own runtime, so bulk reads never queue behind a repaint. The directory hands that engine
//! out; this channel is only for the questions a window has to answer for itself.

use strata_agent::{AgentError, CatalogEntry, Described, RunMode, RunSettle, TabInfo};
use strata_model::TabId;
use tokio::sync::oneshot;

/// One question for one project window, with the channel its answer comes back on.
///
/// The two readonly listings ([`Catalog`](AgentAsk::Catalog), [`Tabs`](AgentAsk::Tabs)) reply
/// with a plain value: a window that is there to receive the ask can always answer them, and
/// a window that is not never sees it (the send fails, or the reply channel drops, and both
/// are [`AgentError::WindowGone`] at the directory). The rest can fail on their own terms —
/// no such table, no such tab — so they carry a `Result`.
pub enum AgentAsk {
    /// The catalog as the store shows it, never DataFusion introspection (AGENTS.md §2).
    Catalog(oneshot::Sender<Vec<CatalogEntry>>),
    Describe {
        name: String,
        reply: oneshot::Sender<Result<Described, AgentError>>,
    },
    Tabs(oneshot::Sender<Vec<TabInfo>>),
    OpenTab(oneshot::Sender<TabId>),
    CloseTab {
        tab: TabId,
        reply: oneshot::Sender<Result<(), AgentError>>,
    },
    /// Set the tab's request — **an ordinary press**. Its reply is the one the driver does
    /// not send itself: it is parked against the press's nonce and completed by that press's
    /// agent keeper when the run settles (`super::keeper`). So the driver stays free for the
    /// next ask while a query runs, which is what keeps it serial without making it slow.
    Run {
        tab: TabId,
        sql: String,
        mode: RunMode,
        page_size: usize,
        reply: oneshot::Sender<Result<RunSettle, AgentError>>,
    },
}
