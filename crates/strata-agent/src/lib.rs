//! **Agent access** — the read-only tool vocabulary an AI agent drives a Strata project
//! with, everything about it that is frontend-agnostic.
//!
//! `docs/AGENT_ACCESS_SPEC.md` is the contract; this crate is the tool vocabulary, the
//! policy gate and the transport's guard rails, and the error taxonomy, over the one
//! deliberate abstraction: the [`Host`] seam. There is **no Freya dependency**, and
//! that is the property doing the work — it is what lets the vocabulary be tested against
//! [`mock::MockHost`] with no window or renderer, reused headless (AA-05), and called
//! in-process by the assistant (AS-01) rather than re-implemented for it.
//!
//! The in-process path is [`StrataTools`]'s own public methods: the ten tools with no rmcp
//! type in any signature, plus [`StrataTools::manifest`] — the same names, descriptions and
//! argument schemas an MCP client reads out of `tools/list`, derived from the router that
//! answers it. The `#[tool]` methods are wrappers over those bodies, so one vocabulary has
//! three transports rather than three vocabularies having one name.
//!
//! ```text
//!   rmcp server  ─┐
//!   stdio server ─┼─→ StrataTools (the ten tools) ─→ Host ─┬─→ AA-03: the app's bridge
//!   chat loop    ─┘        │                               └─→ AA-05: a plain Engine
//!                          └─ data plane: Arc<Engine> direct (fetch_page / validate /
//!                             functions), so bulk reads never queue behind UI work
//! ```
//!
//! The chat loop is [`assistant`] (AS-02): a provider-agnostic agentic turn over the same
//! vocabulary, with its own provider table, its own runtime and no Freya either.
//!
//! An agent's work lives in **query sessions** of its own (AA-03b) — not in the user's
//! editor tabs, which AA-03 tried and which put an agent's twenty-step investigation in the
//! window somebody was working in. A query session maps onto the engine's `WsId`, so the
//! runs stay real (same engine, same snapshots, same supersede) while the attention stays
//! the user's; nothing an agent runs opens, focuses or closes a tab of the user's own.
//!
//! Read-only in v1, the editor's managed-DDL policy exactly: `SELECT` / `EXPLAIN` / `SHOW` /
//! `DESCRIBE` pass and everything else is refused with the message the editor shows. The
//! gate is AA-01's export of the editor's own predicate, applied **before dispatch** — one
//! predicate, two surfaces, zero copies. Curated writes, if they ever arrive, arrive as new
//! tools; `run` never loosens.

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
