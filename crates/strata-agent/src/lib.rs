//! **Agent access** — the read-only tool vocabulary an AI agent drives a Strata project
//! with, everything about it that is frontend-agnostic.
//!
//! `docs/AGENT_ACCESS_SPEC.md` is the contract; this crate is §5 (the vocabulary), §6 (the
//! policy gate and the transport's guard rails) and §7 (the error taxonomy), over the one
//! deliberate abstraction of §3: the [`Host`] seam. There is **no Freya dependency**, and
//! that is the property doing the work — it is what lets the vocabulary be tested against
//! [`mock::MockHost`] with no window or renderer, reused headless (AA-05), and later called
//! in-process by the chat pane (AA-06) rather than re-implemented for it.
//!
//! ```text
//!   rmcp server ─┐
//!   stdio server ─┼─→ StrataTools (the ten tools) ─→ Host ─┬─→ AA-03: the app's bridge
//!   chat loop    ─┘        │                               └─→ AA-05: a plain Engine
//!                          └─ data plane: Arc<Engine> direct (fetch_page / validate /
//!                             functions), so bulk reads never queue behind UI work
//! ```
//!
//! Read-only in v1, the editor's managed-DDL policy exactly: `SELECT` / `EXPLAIN` / `SHOW` /
//! `DESCRIBE` pass and everything else is refused with the message the editor shows. The
//! gate is AA-01's export of the editor's own predicate, applied **before dispatch** — one
//! predicate, two surfaces, zero copies. Curated writes, if they ever arrive, arrive as new
//! tools; `run` never loosens.

pub mod error;
pub mod host;
pub mod mock;
pub mod server;
pub mod tools;
pub mod wire;

pub use error::AgentError;
pub use host::{
    CatalogEntry, Described, Host, Project, RegState, RunMode, RunSettle, Settled, TabInfo,
    TabState,
};
pub use server::{AgentServer, MCP_PATH};
pub use tools::{StrataTools, MAX_PAGE_SIZE};
