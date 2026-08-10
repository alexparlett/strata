//! **The assistant** (AS-02) — the agentic loop behind Strata's chat pane, and the provider
//! seam under it.
//!
//! *genai is the mouth, [`StrataTools`] is the hands, the loop is ours.* One send streams a
//! reply from whichever provider the conversation picked; when the model asks for tools they
//! are executed through the same vocabulary the MCP router serves — in process, no MCP hop,
//! the same policy gate, the same error taxonomy verbatim — and the model is asked again until
//! it answers in prose.
//!
//! Freya-free like the rest of this crate, and that is what makes it testable: a turn runs
//! against [`mock::MockHost`](crate::mock::MockHost) and a stub endpoint with no window, no
//! renderer and no vendor account (`tests/assistant.rs`).
//!
//! ## The four pieces
//!
//! - [`provider`] — **the model of a pick**: the five provider kinds in one table, what each
//!   does with a base URL, a key and a reasoning effort, and the single place a `genai` client
//!   is built from a [`Selection`]. Settings (AS-03) and the composer footer (AS-04) both read
//!   that table; neither restates it.
//! - [`turn`] — the loop, the event stream the pane renders from, and cancel.
//! - [`dispatch`] — name to method: what the model answers with, bound to the ten tools.
//! - [`offer`] — `offer_sql`, the assistant's own eleventh tool: how it hands the user a
//!   statement to execute, checked before the card appears, and never on the MCP router.
//!
//! ## The runtime is this module's own
//!
//! `genai` needs a Tokio reactor and the render thread is not one, so [`Assistant`] owns a
//! small private runtime and the caller holds a handle — the Engine pattern, for the Engine's
//! reason. **Not** [`AgentServer`](crate::AgentServer)'s runtime, which was the obvious
//! economy and is the wrong one: that runtime exists only while agent access is switched on in
//! Settings, and the chat pane must not stop working because the user turned the MCP server
//! off. Two small runtimes with independent lifetimes beat one whose lifetime is a setting.
//!
//! ## It is one more agent below, and not one in the pane above
//!
//! Everything under the loop treats the assistant as an agent like any other: its own
//! `AgentId`, its own query sessions, the same gate. What it is **not** is a row in the Agents
//! pane — that pane answers "which external clients are connected to my project right now",
//! and the assistant is not connected to anything, it is part of the app. The discriminator is
//! [`Agent::in_app`](crate::host::Agent::in_app) — minted by
//! [`StrataTools::in_app`](crate::StrataTools::in_app) and carried to the host on the call
//! that opens a session — rather than the identity's name: a name is a claim any MCP client
//! can make, and a client that could make itself invisible by claiming it is the worst
//! version of this rule.

pub mod dispatch;
pub mod offer;
pub mod provider;
pub mod turn;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::task::JoinHandle;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::host::Host;
use crate::tools::StrataTools;

pub use dispatch::{Facts, Scope};
pub use offer::OFFER_SQL;
pub use provider::{
    BaseUrl, Brain, Effort, KeyUse, Provider, ProviderKind, Selection, SelectionError, PROVIDERS,
};
pub use turn::{Ask, ContextBlock, Conversation, Settle, TurnEvent, MAX_TOOL_ROUNDS, SYSTEM};

/// The chat pane's engine-side handle: a runtime, and a way to start a turn on it.
///
/// One per app rather than one per pane — a runtime is threads, and several conversations
/// streaming at once are several tasks on the same two.
pub struct Assistant {
    /// `Option` only so [`Drop`] can take it for a context-safe `shutdown_background`:
    /// dropping a `Runtime` from inside another runtime panics, and a caller may well be one.
    /// Always `Some` while the value lives.
    rt: Option<Runtime>,
    /// **The HTTP connection pool, held for the app rather than for a turn.**
    ///
    /// `Brain::resolve` builds a `genai::Client` per turn because the resolvers inside it carry
    /// that turn's key and endpoint — but the pool underneath has no reason to go with them,
    /// and rebuilding it made every user message pay a fresh TCP and TLS handshake against
    /// genai's own four-idle-connection, 20-second keep-alive tuning. `reqwest::Client` is an
    /// internal `Arc`, so handing it to each turn is a refcount bump and the key's lifetime is
    /// unchanged.
    pool: reqwest::Client,
}

impl Assistant {
    pub fn new() -> Result<Assistant, String> {
        let rt = RuntimeBuilder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("strata-assistant")
            .build()
            .map_err(|e| format!("assistant runtime: {e}"))?;
        // genai's own defaults for a long-lived client (`webc::web_client`), restated in full
        // because this one now outlives the `genai::Client`s that borrow it and would
        // otherwise get reqwest's bare defaults. **In full** is the point: three of the five
        // were copied once and two were not, which is not "genai's defaults" — it is a
        // different client that looks like them.
        let pool = reqwest::Client::builder()
            .pool_max_idle_per_host(4)
            .http2_keep_alive_interval(Some(Duration::from_secs(20)))
            .http2_keep_alive_timeout(Duration::from_secs(10))
            .http2_keep_alive_while_idle(true)
            .http2_adaptive_window(true)
            .build()
            .map_err(|e| format!("assistant http client: {e}"))?;
        Ok(Assistant { rt: Some(rt), pool })
    }

    /// Start one turn.
    ///
    /// `tools` is cloned by the caller from the pane's own [`StrataTools`], so every turn in a
    /// conversation is the same agent holding the same query sessions. `conversation` is the
    /// model's memory: the turn reads it once and commits its own messages to it once, at the
    /// settle. The pane renders from the events instead.
    pub fn send<H: Host>(
        &self,
        tools: StrataTools<H>,
        selection: Selection,
        scope: Scope,
        conversation: Arc<Mutex<Conversation>>,
        ask: Ask,
    ) -> Running {
        let cancel = CancellationToken::new();
        let (sender, events) = mpsc::unbounded_channel();
        let token = cancel.clone();
        let pool = self.pool.clone();
        let task = self
            .rt
            .as_ref()
            .expect("the runtime lives as long as the assistant")
            .spawn(async move {
                turn::run(
                    &tools,
                    &selection,
                    &scope,
                    &conversation,
                    ask,
                    &sender,
                    &token,
                    &pool,
                )
                .await
            });
        Running {
            _stop: cancel.clone().drop_guard(),
            cancel,
            events,
            task,
        }
    }
}

impl Drop for Assistant {
    /// **A turn dropped by this shutdown contributes nothing rather than half a conversation.**
    /// `shutdown_background` returns at once and drops in-flight tasks without polling them
    /// again, so a turn mid-tool-call never reaches its cancel arm — which used to leave the
    /// caller's `Conversation` (it outlives the `Assistant`) holding tool calls with no
    /// results. It cannot now: a turn stages its messages and commits them once, so a task
    /// dropped anywhere has committed either a whole block or nothing.
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

/// A turn in flight: its events, and the stop.
pub struct Running {
    cancel: CancellationToken,
    /// Cancels the turn when this handle goes: a turn nobody is listening to spends the user's
    /// tokens and their engine on an answer with nowhere to land.
    ///
    /// `tokio_util`'s own guard rather than a hand-rolled newtype with the same `Drop` — it is
    /// already in the graph and it is the shape the crate offers for exactly this. A field
    /// rather than `impl Drop for Running`, because a `Drop` impl would stop
    /// [`settle`](Running::settle) from moving the join handle out.
    _stop: DropGuard,
    events: UnboundedReceiver<TurnEvent>,
    task: JoinHandle<Settle>,
}

impl Running {
    /// Stop the turn. The stream is dropped, an in-flight tool call is dropped — which is the
    /// engine's own abort — and the turn settles as [`Settle::Cancelled`], never as failed.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// The next thing that happened, or `None` when the turn is over.
    ///
    /// [`TurnEvent::Settled`] is always the last event, carrying the same value
    /// [`settle`](Running::settle) returns.
    pub async fn next(&mut self) -> Option<TurnEvent> {
        self.events.recv().await
    }

    /// Wait for the turn to finish and take its outcome.
    ///
    /// A panicking task settles as a failure rather than hanging whoever is waiting: the
    /// events channel would close either way, and a pane left with a spinner is the worse of
    /// the two answers. **A task the runtime cancelled is not a panic** — that is the
    /// [`Assistant`] going away underneath the turn, which is a stop, and reporting it as
    /// failed would break the same rule the loop keeps everywhere else.
    pub async fn settle(mut self) -> Settle {
        match (&mut self.task).await {
            Ok(settle) => settle,
            Err(e) if e.is_cancelled() => Settle::Cancelled,
            Err(e) => Settle::Failed(format!("The assistant's turn did not finish: {e}.")),
        }
    }
}
