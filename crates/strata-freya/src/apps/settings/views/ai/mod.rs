//! **Settings ▸ AI** (AS-03, design `Settings.dc.html`) — the three pages that make the chat
//! pane work, and the one that lets agents in.
//!
//! | Page | What it owns |
//! |---|---|
//! | [`ProvidersPane`] | Which brains exist: one row per kind, each a toggle and its credential |
//! | [`ChatPane`] | What a new chat starts with: provider · model · effort |
//! | [`McpPane`] | The inbound MCP server (AA-04, unchanged but for its name) |
//!
//! **Outbound credentials and inbound hosting are different capabilities**, which is why the
//! group has three pages rather than one. It is also why the old Agent access pane is now
//! *MCP*: what it configures is the MCP server, and the older name described the audience
//! rather than the thing — an audience it now shares with Providers.
//!
//! ## What is on a provider row, and what deliberately is not
//!
//! A row carries what **addresses** the provider: whether it is on, its endpoint where the kind
//! admits one, and its key. It carries **no model**. A model is what a provider is *asked*, and
//! that is a conversation's — picked in the chat pane, seeded from [`ChatPane`]'s defaults. The
//! same line `ConnectionDef` draws when a connection names a bucket and a *table* names the
//! connection.
//!
//! ## The shared half
//!
//! Both editing pages read the same three things, so they are here rather than in either:
//! [`Row`](row::ProviderRow) (the row anatomy every kind shares),
//! and the [`probe`] module — Test and the model list, which are one call
//! (`provider::list_models`) because listing a provider's models *is* a live request with the
//! configured credential, and a separate reachability ping would prove strictly less.

mod chat;
mod configure;
mod keys;
mod mcp;
mod probe;
mod providers;
mod row;

pub use chat::ChatPane;
pub use keys::{commit, TypedKeys};
pub use mcp::McpPane;
pub use probe::Probes;
pub use providers::{missing, ProvidersPane};
