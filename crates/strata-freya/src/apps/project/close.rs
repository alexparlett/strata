//! T2 — the close-while-running *mechanics*. One predicate, one dialog, three triggers:
//! the OS close (red button, vetoed via the fork's `on_close` hook), ⌘Q / menu Quit
//! (`Command::CloseProject` / `MenuCmd::Quit`), and any single-tab close of the tab
//! whose query is in flight ([`TabCloser`]). The dialog itself is
//! `crate::apps::project::views::dialogs::CloseConfirm`.
//!
//! The `on_close` hook runs on the winit thread outside any component scope and must be
//! `Send`, so the window bridges it with atomics ([`CloseGuard`]) plus an unbounded
//! channel: the hook reads the guard synchronously, and on veto sends a ping that wakes
//! the UI executor, which flips the `State<Option<CloseTarget>>` and renders the dialog.
//! "Close anyway" then closes programmatically via `close_current_window()`, which
//! bypasses the veto.
//!
//! **The in-flight half of the guard is the engine's, not the UI's.** A run belongs to a
//! workspace (a tab), not to a mounted view, and only the active tab's results are
//! mounted — so a derivation from the UI went false the moment the user switched tabs on a
//! running query, and both the window close and a background tab's ⌘W skipped the confirm
//! with the engine still executing. The engine owns both answers now
//! ([`Engine::watch_inflight`](strata_core::engine::Engine::watch_inflight) for the window,
//! [`Engine::is_running`](strata_core::engine::Engine::is_running) per tab).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use freya::prelude::*;
use freya::radio::Radio;
use freya::winit::window::WindowId;
use futures::channel::mpsc::{unbounded, UnboundedReceiver};

use crate::apps::project::contexts::EngineCtx;
use crate::state::ConfigStation;
use strata_model::TabId;

use crate::apps::project::state::{Chan, SessionState};

/// Shared with the winit `on_close` hook, which only reads it.
pub struct CloseGuard {
    /// Whether this window's engine has *any* run or explain executing — written by the
    /// engine itself, inside every lifecycle mutation. An `Arc` because the engine holds
    /// the very same flag: the window hands it over once, at engine creation.
    pub running: Arc<AtomicBool>,
    /// Mirrors the `confirm_close_running` setting (the window root's side effect).
    pub confirm: AtomicBool,
}

/// What the confirm dialog is about to close.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CloseTarget {
    /// The whole window (OS close / ⌘Q).
    Window,
    /// One tab whose query is in flight (⌘W).
    Tab(TabId),
}

/// The UI half of the bridge, carried in the `ProjectApp`: the shared guard plus the
/// veto-signal receiver (taken once by the root, which drains it into the confirm state).
/// (The other half — the winit `on_close` hook — is the closure `close_bridge` returns.)
pub struct CloseBridge {
    pub guard: Arc<CloseGuard>,
    rx: RefCell<Option<UnboundedReceiver<()>>>,
}

impl CloseBridge {
    pub fn take_rx(&self) -> Option<UnboundedReceiver<()>> {
        self.rx.borrow_mut().take()
    }
}

/// Build one window's close bridge: the UI half + the `on_close` hook for
/// `WindowConfig::with_on_close`. `confirm_seed` is the setting's value at build time —
/// the root's side effect keeps it mirrored after that.
pub fn close_bridge(
    confirm_seed: bool,
) -> (
    CloseBridge,
    impl FnMut(RendererContext, WindowId) -> CloseDecision + Send + 'static,
) {
    let (tx, rx) = unbounded();
    let guard = Arc::new(CloseGuard {
        running: Arc::new(AtomicBool::new(false)),
        confirm: AtomicBool::new(confirm_seed),
    });
    let hook_guard = guard.clone();
    // The parameter annotations keep the closure generic over `RendererContext`'s
    // lifetime (plain inference would pin it and fail the `for<'a> FnMut` bound).
    let hook = move |_ctx: RendererContext<'_>, _id: WindowId| {
        if hook_guard.running.load(Ordering::Relaxed) && hook_guard.confirm.load(Ordering::Relaxed)
        {
            // A query is in flight and the user wants the confirm: veto the close and
            // wake the UI to show the dialog.
            let _ = tx.unbounded_send(());
            CloseDecision::KeepOpen
        } else {
            CloseDecision::Close
        }
    };
    (
        CloseBridge {
            guard,
            rx: RefCell::new(Some(rx)),
        },
        hook,
    )
}

/// Close one tab through the close-while-running confirm — the gate **every**
/// single-tab close path shares: ⌘W, the tab's × button, the tab context menu's Close,
/// and the nav dropdown's ×. Provided into context by the workbench; bulk closes (close
/// all / others / to-the-right) stay immediate — power actions whose engine cleanup
/// already runs through the root's tab-diff funnel.
#[derive(Clone, Copy, PartialEq)]
pub struct TabCloser {
    /// This window's engine, in a `State` slot purely so the struct stays `Copy` —
    /// it is passed by value into several tab-strip closures, and `EngineCtx` is an
    /// `Arc` (`Clone`, not `Copy`).
    pub engine: State<EngineCtx>,
    pub confirm: State<Option<CloseTarget>>,
}

impl TabCloser {
    /// Close `id` — via the confirm when its query is in flight and the pref is on.
    pub fn close(&self, mut radio: Radio<SessionState, Chan>, config: ConfigStation, id: TabId) {
        // Ask the engine, not the UI: a tab *is* a workspace, and a background tab's run
        // has no mounted results pane to derive from. `peek()` because close() runs in
        // event handlers, which have no reactive scope to subscribe.
        let in_flight = self.engine.peek().is_running(id.into());
        if in_flight && config.peek().settings.confirm_close_running {
            let mut confirm = self.confirm;
            confirm.set(Some(CloseTarget::Tab(id)));
        } else {
            radio.write().close_one(id);
        }
    }
}
