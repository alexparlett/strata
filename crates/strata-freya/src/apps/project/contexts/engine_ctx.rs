//! The engine bridge: spawn the shared `strata-core` engine and expose it to the Freya UI
//! as a cloneable [`EngineCtx`] — an `Arc` over the handle, held in window context. The
//! sending side (`send`/`next_req`) is shared freely; the single-consumer event stream is
//! claimed once by the drain via [`take_evt_rx`](EngineCtx::take_evt_rx). The engine owns
//! its own Tokio runtime on its own thread, so nothing here needs a runtime (see `main`).

use std::sync::Arc;

use strata_core::engine::{Command, Engine, Event};
use tokio::sync::mpsc::UnboundedReceiver;

/// A window's engine handle for context: an `Arc` over the shared `strata-core` [`Engine`],
/// so it's cheap to `Clone` and hand to every component via `use_provide_context`.
#[derive(Clone)]
pub struct EngineCtx {
    eng: Arc<Engine>,
}

impl EngineCtx {
    /// Spawn this window's engine (its worker thread + runtime) and wrap it for context.
    pub fn new() -> Self {
        Self {
            eng: Arc::new(Engine::spawn(Default::default())),
        }
    }

    /// Send a command to the engine (non-blocking; no-op if the worker has gone).
    pub fn send(&self, cmd: Command) {
        self.eng.send(cmd);
    }

    /// Allocate the next request id (monotonic; shared across all clones of the handle).
    pub fn next_req(&self) -> u64 {
        self.eng.next_req()
    }

    /// Claim the event stream for the single drain task (panics if called twice). The drain
    /// then owns the receiver and awaits it directly — no lock held across `.await`.
    pub fn take_evt_rx(&self) -> UnboundedReceiver<Event> {
        self.eng.take_evt_rx()
    }
}

impl Default for EngineCtx {
    fn default() -> Self {
        Self::new()
    }
}
