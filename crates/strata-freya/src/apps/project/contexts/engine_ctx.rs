//! The engine bridge: spawn the shared `strata-core` [`Engine`] and expose it to the
//! Freya UI as a cloneable [`EngineCtx`] — the window's one engine handle, held in
//! context. The engine is a **direct-call async facade** (it owns its own Tokio runtime
//! internally), so freya-query capabilities simply await its methods — no protocol, no
//! event stream, no UI-side runtime. This wrapper adds only what's UI-shaped: the
//! [`TabId`] → [`WsId`](strata_core::engine::WsId) identity (a tab *is* an engine
//! workspace) and the tab-close cleanup hook the window root drives.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::sync::Arc;

use freya::query::Captured;
use strata_core::engine::{Engine, SnapshotPin};

use strata_model::{ChartData, ChartQuery, SnapshotId, TabId};

/// A window's engine handle for context — an `Arc` over the shared [`Engine`], cheap to
/// `Clone`, provided once via `use_provide_context`. Derefs to the engine, so callers
/// use the facade directly (`engine.query(…)`, `engine.fetch_page(…)`).
#[derive(Clone)]
pub struct EngineCtx {
    eng: Arc<Engine>,
}

impl EngineCtx {
    /// Spawn this window's engine (its private runtime + context) with the app's
    /// `datafusion.*` overrides (Settings ▸ Engine ▸ Properties, W2), and wrap it for context.
    ///
    /// The overrides are a **launch value**, not a subscription: the `RuntimeEnv` half of them is
    /// fixed the moment the `SessionContext` is built, so an engine is only ever *born* with a
    /// full set. Keeping them in step after that is
    /// [`use_engine_config`](crate::apps::project::state::use_engine_config)'s job.
    pub fn new(overrides: BTreeMap<String, String>) -> Self {
        Self {
            eng: Arc::new(Engine::new(overrides)),
        }
    }

    /// The engine itself, for a holder that is **not** on the render thread — the agent
    /// server's data plane (`crate::agent`), which calls `fetch_page` / `validate` /
    /// `functions` from its own runtime while the UI is busy.
    ///
    /// Handing out the `Arc` rather than the [`EngineCtx`] is the point: `EngineCtx` is this
    /// window's *UI* handle, and everything it adds over the facade (`cleanup`, `captured`,
    /// `pin_snapshot`) belongs to the render thread. A cross-thread holder wants the facade
    /// and nothing else.
    pub fn arc(&self) -> Arc<Engine> {
        Arc::clone(&self.eng)
    }

    /// Wrap this handle for a freya-query capability field — invisible to cache identity.
    /// (Consumed by the results pane's `use_query` wiring, P2-02.)
    #[allow(dead_code)]
    pub fn captured(&self) -> Captured<EngineCtx> {
        Captured(self.clone())
    }

    /// Tear down a closed tab's engine-side state — abort its in-flight run and retire
    /// its snapshot. Driven by the window root's side effect diffing the session's open
    /// tabs, so every close path funnels through one place.
    pub fn cleanup(&self, tab: TabId) {
        self.eng.cleanup_ws(tab.into());
    }

    /// Hold `snapshot` open for as long as the returned pin lives — the escape hatch from
    /// retire-on-dispatch for a reader that outlives one Run (SNAPSHOT_SPEC §4). The export
    /// window holds one for its whole life.
    ///
    /// Not reachable through `Deref`: [`Engine::pin_snapshot`] takes `&Arc<Engine>` (the pin
    /// keeps the engine alive), and deref only ever hands out `&Engine`.
    pub fn pin_snapshot(&self, snapshot: SnapshotId) -> SnapshotPin {
        self.eng.pin_snapshot(snapshot)
    }

    /// Read `snapshot` as a chart (Rz2, `docs/CHART_SPEC.md` §5) — the results Chart body's
    /// capability, behind `FetchChart`.
    ///
    /// Not reachable through `Deref` for the same reason [`Self::pin_snapshot`] isn't:
    /// [`Engine::chart`] takes `&Arc<Engine>` (it holds a pin across its own reads, and the
    /// pin keeps the engine alive), and deref only ever hands out `&Engine`.
    pub async fn chart(&self, snapshot: SnapshotId, q: ChartQuery) -> Result<ChartData, String> {
        self.eng.chart(snapshot, q).await
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
        Self::new(BTreeMap::new())
    }
}
