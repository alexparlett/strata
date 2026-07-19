//! DataFusion engine on a dedicated thread with its own Tokio runtime.
//!
//! Pagination model (bounded memory): each query is executed **once** and its
//! full result is spooled to a temporary parquet **snapshot** on disk. The true
//! row count comes from a `COUNT(*)` over the snapshot, and every page is a
//! bounded `LIMIT/OFFSET` read from it — so RAM only ever holds one page, no
//! matter how far the user pages, and no query is ever recomputed per page.
//!
//! UI → engine: `tokio::mpsc::unbounded` of [`Command`]. engine → UI:
//! `tokio::mpsc::unbounded` of [`Event`], drained by the frontend.
//!
//! This module is the **handle** — a plain connection object (tokio channels + an
//! atomic request counter + the registered functions), framework-agnostic. Each
//! frontend owns it in its own reactive storage: the Dioxus app wraps it in a
//! per-window `GlobalSignal` (see that crate's `engine` shim); a Freya frontend
//! would hold it in context/Radio. The worker loop and its helpers live in the
//! submodules; `pub use` keeps the public types at their original `engine::*` paths.

mod catalog;
mod explain;
mod export;
mod message;
mod query;
mod worker;
pub mod config;
pub mod serialize;
pub mod plan;
pub mod sql;
pub mod profile;

pub use message::{Command, Event, TableMeta, TableSpec};
pub use query::purge_snapshot_root;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use sql::FunctionCatalog;

/// Process-unique id per spawned engine (one per project window), used to scope
/// snapshot files so windows never collide.
static ENGINE_SEQ: AtomicU64 = AtomicU64::new(0);

/// A window's engine — the connection **handle**: the command channel ([`send`](Engine::send)),
/// the request-id counter ([`next_req`](Engine::next_req)), the event stream
/// ([`take_evt_rx`](Engine::take_evt_rx)), and the registered SQL functions. Plain
/// tokio channels + an atomic — nothing tied to any UI framework; a frontend holds it
/// in its own reactive storage. [`spawn`](Engine::spawn) starts the worker thread and
/// hands back the instance, the event stream riding inside it (single-consumer, taken
/// once).
pub struct Engine {
    cmd_tx: UnboundedSender<Command>,
    /// This window's event stream — `Some` until the single drain task takes it
    /// ([`take_evt_rx`](Engine::take_evt_rx)). A receiver is single-consumer.
    evt_rx: Option<UnboundedReceiver<Event>>,
    /// Monotonic request-id source.
    next_req: AtomicU64,
    /// The engine's registered SQL functions — read (reactively, by the frontend) by
    /// the language service.
    functions: FunctionCatalog,
}

impl Engine {
    /// Start a window's engine worker (seeded with the given `datafusion.*`
    /// `overrides`, W2) and return the handle — the event stream rides inside, taken
    /// once via [`take_evt_rx`](Engine::take_evt_rx). Later config changes arrive as
    /// [`Command::SetEngineConfig`].
    pub fn spawn(overrides: BTreeMap<String, String>) -> Engine {
        let (cmd_tx, cmd_rx) = unbounded_channel::<Command>();
        let (evt_tx, evt_rx) = unbounded_channel::<Event>();
        let engine_id = ENGINE_SEQ.fetch_add(1, Ordering::Relaxed);
        std::thread::Builder::new()
            .name(format!("df-engine-{engine_id}"))
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(worker::engine_loop(cmd_rx, evt_tx, engine_id, overrides));
            })
            .expect("spawn engine");
        Engine {
            cmd_tx,
            evt_rx: Some(evt_rx),
            next_req: AtomicU64::new(1),
            functions: FunctionCatalog::default(),
        }
    }

    /// Send a command to this engine (no-op if the worker has gone away).
    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// A cloned command sender — the sending half of the handle, for a frontend that
    /// holds it in framework context (e.g. Freya) rather than the whole handle. The clone
    /// keeps the command channel open alongside the worker.
    pub fn sender(&self) -> UnboundedSender<Command> {
        self.cmd_tx.clone()
    }

    /// Allocate the next request id (monotonic).
    pub fn next_req(&self) -> u64 {
        self.next_req.fetch_add(1, Ordering::Relaxed)
    }

    /// The registered SQL functions (the editor's language catalog).
    pub fn functions(&self) -> &FunctionCatalog {
        &self.functions
    }

    /// Replace the registered SQL functions (from [`Event::Functions`]).
    pub fn set_functions(&mut self, functions: FunctionCatalog) {
        self.functions = functions;
    }

    /// Take this engine's event stream for the single drain task. A receiver is
    /// single-consumer; panics if taken twice.
    pub fn take_evt_rx(&mut self) -> UnboundedReceiver<Event> {
        self.evt_rx.take().expect("engine event stream already taken")
    }
}
