//! The window's **close bridge**: the close-while-running confirm (T2) *and* the
//! last-window-becomes-the-launcher rule, which share one mechanism because both need the
//! same thing — the OS close held off long enough for the UI to act.
//!
//! The `on_close` hook runs on the winit thread outside any component scope and must be
//! `Send`, so the window bridges it with atomics ([`CloseGuard`]) plus an unbounded
//! channel: the hook reads the guard synchronously, and when it declines the close it
//! sends a [`Veto`] that wakes the UI executor. The UI then does what the hook couldn't
//! and closes programmatically via `close_current_window()`, which bypasses the veto.
//!
//! Two reasons to decline, in precedence order:
//!
//! 1. [`Veto::Confirm`] — a query is in flight and the confirm pref is on: show the T2
//!    dialog (`views::dialogs::CloseConfirm`) and let the user decide. Shared by the red
//!    button, ⇧⌘W, menu Close Project, ⌘Q, and any single-tab close of a running tab
//!    ([`TabCloser`]).
//! 2. [`Veto::Launcher`] — this is the app's last window and no quit is in flight: the
//!    launcher has to be up *before* this window goes, or the app would exit instead
//!    ([`platform::close_this_window`]).
//!
//! **The in-flight half of the guard is the engine's, not the UI's.** A run belongs to a
//! workspace (a tab), not to a mounted view, and only the active tab's results are
//! mounted — so a derivation from the UI went false the moment the user switched tabs on a
//! running query, and both the window close and a background tab's ⌘W skipped the confirm
//! with the engine still executing. The engine owns both answers now
//! ([`Engine::watch_inflight`](strata_core::engine::Engine::watch_inflight) for the window,
//! [`Engine::is_running`](strata_core::engine::Engine::is_running) per tab). `confirm` and
//! `last` are still mirrored from reactive state by the root's `use_side_effect`s.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use freya::prelude::*;
use freya::radio::Radio;
use freya::winit::window::WindowId;
use futures::channel::mpsc::{unbounded, UnboundedReceiver};

use crate::apps::project::contexts::EngineCtx;
use crate::platform;
use crate::platform::{close_this_window, OpenCtx};
use crate::state::{AppCtx, ConfigStation};
use strata_model::TabId;

use crate::apps::project::state::{Chan, SessionState};

/// Shared with the winit `on_close` hook, which only reads it. `running` is written by the
/// engine; `confirm` (← the `confirm_close_running` setting) and `last` (← the app-global
/// window registry) are mirrored from reactive state by the window root's side effects.
pub struct CloseGuard {
    /// Whether this window's engine has *any* run or explain executing — written by the
    /// engine itself, inside every lifecycle mutation. An `Arc` because the engine holds
    /// the very same flag: the window hands it over once, at engine creation.
    pub running: Arc<AtomicBool>,
    /// Mirrors the `confirm_close_running` setting (the window root's side effect).
    pub confirm: AtomicBool,
    /// Whether this is the app's last window — if so its close has to put the launcher up
    /// first (see [`Veto::Launcher`]).
    pub last: AtomicBool,
}

/// Why the `on_close` hook declined a close, and so what the UI has to do about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Veto {
    /// A query is in flight: raise the T2 confirm.
    Confirm,
    /// Last window out: open the launcher, then close.
    Launcher,
}

/// What the confirm dialog is about to close.
///
/// Not `Copy`: [`Reroot`](Self::Reroot) carries the folder to open once the question is
/// answered. Every read of the slot clones instead — a handful of sites, against keeping a
/// second pending-path slot in step with this one.
#[derive(Clone, PartialEq, Eq)]
pub enum CloseTarget {
    /// The whole window (OS close / ⌘Q).
    Window,
    /// One tab whose query is in flight (⌘W).
    Tab(TabId),
    /// The window's **project**, in favour of the one at this folder — opening in place
    /// ([`OpenCtx::reroot`](crate::platform::OpenCtx::reroot)) unmounts the project subtree,
    /// and dropping its engine aborts everything in flight. Same loss of work as closing the
    /// window, so it asks on the same terms rather than being the one destructive path that
    /// doesn't.
    Reroot(PathBuf),
    /// The window's **engine**, in favour of one built with changed `datafusion.runtime.*`
    /// properties (P4-07). Those configure the `RuntimeEnv`, which is fixed when the
    /// `SessionContext` is built, so applying one means a new engine — which unmounts the
    /// project subtree exactly as a re-root does, and aborts everything in flight for the same
    /// reason.
    Restart,
}

/// The UI half of the bridge, carried in the `ProjectApp`: the shared guard plus the
/// veto-signal receiver (taken once by the root, which drains it into the confirm state).
/// (The other half — the winit `on_close` hook — is the closure `close_bridge` returns.)
pub struct CloseBridge {
    pub guard: Arc<CloseGuard>,
    rx: RefCell<Option<UnboundedReceiver<Veto>>>,
}

impl CloseBridge {
    pub fn take_rx(&self) -> Option<UnboundedReceiver<Veto>> {
        self.rx.borrow_mut().take()
    }
}

/// Build one window's close bridge: the UI half + the `on_close` hook for
/// `WindowConfig::with_on_close`. `confirm_seed` is the setting's value at build time —
/// the root's side effects keep it and `last` mirrored after that.
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
        last: AtomicBool::new(false),
    });
    let hook_guard = guard.clone();
    // The parameter annotations keep the closure generic over `RendererContext`'s
    // lifetime (plain inference would pin it and fail the `for<'a> FnMut` bound).
    let hook = move |_ctx: RendererContext<'_>, _id: WindowId| {
        let veto = if hook_guard.running.load(Ordering::Relaxed)
            && hook_guard.confirm.load(Ordering::Relaxed)
        {
            // A query is in flight and the user wants the confirm: hold the close and wake
            // the UI to show the dialog.
            Some(Veto::Confirm)
        } else if hook_guard.last.load(Ordering::Relaxed) && !platform::is_quitting() {
            // Last window out, and this is a close rather than a quit: the launcher has to
            // be up before this window goes, or there'd be no windows left and the app
            // would exit. Quitting is the case where that's the point.
            Some(Veto::Launcher)
        } else {
            None
        };
        match veto {
            Some(veto) => {
                let _ = tx.unbounded_send(veto);
                CloseDecision::KeepOpen
            }
            None => CloseDecision::Close,
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

/// What closing means on a project subtree that has **no engine** — the two arms of
/// `ProjectRoot` that are not an open project: the load still in flight
/// ([`ProjectLoading`](crate::apps::project::views::ProjectLoading)) and the load that failed
/// ([`ProjectLoadFailed`](crate::apps::project::views::ProjectLoadFailed)). Returns the window's
/// close and whether it is already under way; call once in such an arm.
///
/// Two halves that always travel together, which is why they are one hook rather than a copy in
/// each arm.
///
/// The close is **once-only**. It is several async hops — it may have to stand the launcher up
/// before this window goes — and neither arm can dismiss itself in the gap, so without the flag
/// a second press closes a second window. `spawn_forever`, not `spawn`: the close unmounts the
/// very scope the handler belongs to, and scope teardown drops that scope's tasks before they
/// are ever polled.
///
/// And the window's **close-confirm slot is drained rather than rendered**. Neither arm mounts
/// [`CloseConfirm`](crate::apps::project::views::CloseConfirm) — there is no project to name in
/// the question — yet the slot's writers still fire: `guard.running` can be true on either arm,
/// because a run in flight when the window re-rooted or restarted keeps the *outgoing* engine
/// alive until it settles. Left alone, a red-button close (vetoed into
/// [`CloseTarget::Window`]) or a parked re-root would sit in a slot nothing renders, and read as
/// a dead control.
///
/// Draining **acts** rather than re-asks, and that is a boundary review settled (AGENTS.md §2):
/// `guard.running` can only be true here for runs orphaned by a stop the user already agreed to
/// — the re-root or restart that replaced the subtree asked the T2 question and got its answer,
/// or the pref that gates every writer of this slot asked never to ask — so a second confirm
/// would re-ask about work already condemned. [`CloseTarget::Tab`] and [`CloseTarget::Restart`]
/// have no writer on either arm; clearing them keeps the slot from carrying a stale question
/// into the next mount.
pub fn use_engineless_close(
    app: AppCtx,
    confirm: State<Option<CloseTarget>>,
) -> (impl Fn() + Clone, State<bool>) {
    // Taken in the render scope so the handlers can run from a task.
    let platform = use_hook(Platform::get);
    let open = use_consume::<OpenCtx>();
    let closing = use_state(|| false);
    let close = {
        move || {
            let mut closing = closing;
            if *closing.peek() {
                return;
            }
            closing.set(true);
            spawn_forever(close_this_window(platform.clone(), app.clone()));
        }
    };
    {
        let close = close.clone();
        use_side_effect(move || {
            // Read into a value first — a match on the guard's temporary would hold the read
            // borrow across the `set` on the same `State`.
            let target = confirm.read().clone();
            let Some(target) = target else {
                return;
            };
            let mut confirm = confirm;
            confirm.set(None);
            match target {
                CloseTarget::Window => close(),
                CloseTarget::Reroot(root) => open.reroot_confirmed(root),
                CloseTarget::Tab(_) | CloseTarget::Restart => {}
            }
        });
    }
    (close, closing)
}

/// Close this **window** through the close-while-running confirm — [`TabCloser::close`]'s
/// window-scoped peer, and the gate every deliberate window close shares: ⇧⌘W, File ▸ Close
/// Project, and the command palette's Close project row. (The red button reaches the same
/// dialog by a different road: the `on_close` hook vetoes with [`Veto::Confirm`] and the root
/// drains it into the same slot.)
///
/// The predicate is the hook's, exactly — the engine's own in-flight flag plus
/// `confirm_close_running` — because AGENTS.md §2 requires every path that destroys a window's
/// work to ask on the same terms. It lives here, beside the tab gate it mirrors, rather than
/// inline at a call site: it was inline in `ProjectApp`'s shortcut handler, which is a scope the
/// palette cannot reach, and a second copy of a confirm predicate is how one path ends up
/// silently skipping the question.
///
/// `spawn_forever`, for [`use_engineless_close`]'s reason: the close unmounts the scope the
/// handler belongs to — for the palette, its own overlay, which dismisses itself in the same
/// breath — and scope teardown drops that scope's tasks before they are ever polled.
pub fn close_project(
    guard: &CloseGuard,
    config: ConfigStation,
    mut confirm: State<Option<CloseTarget>>,
    platform: Platform,
    app: AppCtx,
) {
    if guard.running.load(Ordering::Relaxed) && config.peek().settings.confirm_close_running {
        confirm.set(Some(CloseTarget::Window));
    } else {
        spawn_forever(close_this_window(platform, app));
    }
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
