//! T2 — the close-while-running *mechanics*. One predicate, one dialog, three triggers:
//! the OS close (red button, vetoed via the fork's `on_close` hook), ⌘Q / menu Quit
//! (`Command::CloseProject` / `MenuCmd::Quit`), and any single-tab close of the tab
//! whose query is in flight ([`TabCloser`]). The dialog itself is
//! `crate::apps::project::views::dialogs::CloseConfirm`.
//!
//! The `on_close` hook runs on the winit thread outside any component scope and must be
//! `Send`, so the window bridges it with atomics ([`CloseGuard`], mirrored from reactive
//! state by `use_side_effect`s) plus an unbounded channel: the hook reads the guard
//! synchronously, and on veto sends a ping that wakes the UI executor, which flips the
//! `State<Option<CloseTarget>>` and renders the dialog. "Close anyway" then closes
//! programmatically via `close_current_window()`, which bypasses the veto.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use freya::prelude::*;
use freya::radio::Radio;
use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use strata_core::config::Settings;

use crate::apps::project::query::RunId;
use crate::apps::project::state::{Chan, SessionState, TabId};

/// Shared with the winit `on_close` hook. The UI mirrors reactive state in
/// (`running` ← the workbench's in-flight derivation, `confirm` ← the
/// `confirm_close_running` setting); the hook only reads.
pub struct CloseGuard {
    pub running: AtomicBool,
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
    impl FnMut(RendererContext, freya::winit::window::WindowId) -> CloseDecision + Send + 'static,
) {
    let (tx, rx) = unbounded();
    let guard = Arc::new(CloseGuard {
        running: AtomicBool::new(false),
        confirm: AtomicBool::new(confirm_seed),
    });
    let hook_guard = guard.clone();
    // The parameter annotations keep the closure generic over `RendererContext`'s
    // lifetime (plain inference would pin it and fail the `for<'a> FnMut` bound).
    let hook = move |_ctx: RendererContext<'_>, _id: freya::winit::window::WindowId| {
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
/// and the nav dropdown's ×. Provided into context by the workbench (which owns the run
/// slots); bulk closes (close all / others / to-the-right) stay immediate — power
/// actions whose engine cleanup already runs through the root's tab-diff funnel.
#[derive(Clone, Copy, PartialEq)]
pub struct TabCloser {
    pub running: State<Option<RunId>>,
    pub confirm: State<Option<CloseTarget>>,
}

impl TabCloser {
    /// Close `id` — via the confirm when its query is in flight and the pref is on.
    pub fn close(&self, mut radio: Radio<SessionState, Chan>, settings: State<Settings>, id: TabId) {
        // `read()` is peek-equivalent here: close() runs in event handlers, no reactive scope.
        let in_flight = radio
            .read()
            .request(id)
            .is_some_and(|s| *self.running.peek() == Some(s.run));
        if in_flight && settings.peek().confirm_close_running {
            let mut confirm = self.confirm;
            confirm.set(Some(CloseTarget::Tab(id)));
        } else {
            radio.write().close_one(id);
        }
    }
}

