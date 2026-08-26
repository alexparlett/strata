//! What this engine has in flight, engine-wide — the two answers no workspace can give.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::Engine;

/// This engine's in-flight work as a whole, from [`Engine::work`].
#[derive(Clone, Copy)]
pub struct Work<'a> {
    pub(super) engine: &'a Engine,
}

impl Work<'_> {
    /// Returns the flag mirroring "this engine has work in flight" — runs, classifications,
    /// profile scans and background work.
    ///
    /// Written inside every lifecycle mutation under the engine's own lock, and from birth, so a
    /// reader that can reach neither the lock nor async code can hold a copy and read it at any
    /// time. Only the engine can answer this: a run belongs to a workspace rather than to
    /// anything mounted, so a caller deriving it from what it can see reports idle for a run it
    /// is not looking at.
    pub fn flag(self) -> Arc<AtomicBool> {
        Arc::clone(&self.engine.inflight_flag)
    }

    /// Returns whether anything other than a workspace run is in flight: a profile scan, an
    /// export, or a drop deleting a table's data.
    ///
    /// [`flag`](Self::flag) counts these too, so a caller deciding *whose* work is at stake
    /// cannot answer from [`Workspace::is_running`](crate::Workspace::is_running) over the
    /// workspaces it knows about — the rest is not idle just because it is unnamed.
    pub fn background(self) -> bool {
        let lc = self.engine.lifecycle.lock().unwrap();
        !lc.profiles.is_empty() || lc.background > 0
    }
}
