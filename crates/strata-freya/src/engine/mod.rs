//! The engine bridge: spawn the shared `strata-core` engine and hand the Freya UI its two
//! halves — a cloneable command sender ([`EngineCtx`], held in window context) and the
//! event stream (drained by a Freya task into the window's state). The engine owns its own
//! Tokio runtime on its own thread, so nothing here needs a runtime (see `main`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use strata_core::engine::{Command, Engine, Event};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// The sending half of a window's engine, cloned into UI context. `Command`s go out on a
/// `tokio::sync` unbounded sender (non-blocking, runtime-free); request ids are allocated
/// UI-side to match a `Query`/`Explain` to its result event.
#[derive(Clone)]
pub struct EngineCtx {
    cmd_tx: UnboundedSender<Command>,
    req_seq: Arc<AtomicU64>,
}

impl EngineCtx {
    /// Send a command to the engine (no-op if the worker has gone).
    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Allocate the next request id (monotonic).
    pub fn next_req(&self) -> u64 {
        self.req_seq.fetch_add(1, Ordering::Relaxed)
    }
}

/// Spawn this window's engine. Returns the sender half (for context) + the event stream to
/// drain. The core `Engine` is dropped here — its cloned sender keeps the command channel
/// open alongside the worker thread.
pub fn spawn() -> (EngineCtx, UnboundedReceiver<Event>) {
    let mut engine = Engine::spawn(Default::default());
    let evt_rx = engine.take_evt_rx();
    let cmd_tx = engine.sender();
    (
        EngineCtx {
            cmd_tx,
            req_seq: Arc::new(AtomicU64::new(1)),
        },
        evt_rx,
    )
}
