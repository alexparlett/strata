//! **Agent access** — the tool vocabulary an AI agent drives a Strata project with, everything
//! about it that is frontend-agnostic.
//!
//! This crate is the tool vocabulary, the policy gate,
//! the transport's guard rails and the error taxonomy, over one deliberate abstraction: the [`Host`]
//! seam. There is **no Freya dependency**, and that is the property doing the work — it is what
//! lets the vocabulary be tested against [`mock::MockHost`] with no window, reused headless
//! (AA-05), and called in-process by the assistant rather than re-implemented for it.
//!
//! ```text
//!   rmcp server  ─┐
//!   stdio server ─┼─→ StrataTools (the eleven tools) ─→ Host ─┬─→ AA-03: the app's bridge
//!   chat loop    ─┘        │                               └─→ AA-05: a plain Engine
//!                          └─ data plane: Arc<Engine> direct (fetch_page / validate /
//!                             functions), so bulk reads never queue behind UI work
//! ```
//!
//! The in-process path is [`StrataTools`]'s own public methods, plus [`StrataTools::manifest`] —
//! the same names and schemas `tools/list` advertises, derived from the router that answers it. So
//! one vocabulary has three transports rather than three vocabularies having one name.
//!
//! An agent's work lives in **query sessions** of its own, not in the user's editor tabs (AA-03
//! tried that, and it put an agent's twenty-step investigation in the window somebody was working
//! in). A query session maps onto the engine's `WsId`, so the runs stay real while the attention
//! stays the user's.
//!
//! SQL is read-only, the editor's managed-DDL policy exactly, gated **before dispatch** through
//! AA-01's export of the editor's own predicate. `run` never loosens: the one curated write
//! (`export_result`, QE-05) is a tool of its own, and the only thing it can produce is a new file
//! outside the storage Strata owns.

pub mod assistant;
mod describe;
pub mod error;
pub mod headless;
pub mod host;
pub mod mock;
pub mod server;
pub mod tools;
pub mod wire;

pub use assistant::{Ask, Assistant, Conversation, Running, Selection, Settle, TurnEvent};
pub use error::AgentError;
pub use headless::{serve_stdio, HeadlessHost};
pub use host::{
    Agent, AgentId, AgentIdentity, CatalogEntry, Described, Host, Project, QuerySessionId,
    QuerySessionInfo, QuerySessionState, RegState, RunMode, RunSettle, Settled,
};
pub use server::{mint_token, AgentServer, MCP_PATH};
pub use tools::{StrataTools, ToolSpec, MAX_PAGE_SIZE};
