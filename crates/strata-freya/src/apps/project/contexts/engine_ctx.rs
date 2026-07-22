//! The engine bridge: spawn the shared `strata-core` [`Engine`] and expose it to the
//! Freya UI as a cloneable [`EngineCtx`] — the window's one engine handle, held in
//! context. The engine is a **direct-call async facade** (it owns its own Tokio runtime
//! internally), so freya-query capabilities simply await its methods — no protocol, no
//! event stream, no UI-side runtime. This wrapper adds only what's UI-shaped: the
//! [`TabId`] → [`WsId`] identity (a tab *is* an engine workspace) and the tab-close
//! cleanup hook the window root drives.

use std::ops::Deref;
use std::sync::Arc;

use strata_core::engine::{Engine, WsId};

use crate::apps::project::state::TabId;

impl From<TabId> for WsId {
    fn from(tab: TabId) -> Self {
        WsId(tab.0.as_u128())
    }
}

/// A window's engine handle for context — an `Arc` over the shared [`Engine`], cheap to
/// `Clone`, provided once via `use_provide_context`. Derefs to the engine, so callers
/// use the facade directly (`engine.query(…)`, `engine.fetch_page(…)`).
#[derive(Clone)]
pub struct EngineCtx {
    eng: Arc<Engine>,
}

impl EngineCtx {
    /// Spawn this window's engine (its private runtime + context) and wrap it for context.
    pub fn new() -> Self {
        Self {
            eng: Arc::new(Engine::new(Default::default())),
        }
    }

    /// Wrap this handle for a freya-query capability field — invisible to cache identity.
    /// (Consumed by the results pane's `use_query` wiring, P2-02.)
    #[allow(dead_code)]
    pub fn captured(&self) -> freya::query::Captured<EngineCtx> {
        freya::query::Captured(self.clone())
    }

    /// Tear down a closed tab's engine-side state — abort its in-flight run and retire
    /// its snapshot. Driven by the window root's side effect diffing the session's open
    /// tabs, so every close path funnels through one place.
    pub fn cleanup(&self, tab: TabId) {
        self.eng.cleanup_ws(tab.into());
    }
}

impl Deref for EngineCtx {
    type Target = Engine;

    fn deref(&self) -> &Engine {
        &self.eng
    }
}

impl Default for EngineCtx {
    fn default() -> Self {
        Self::new()
    }
}
