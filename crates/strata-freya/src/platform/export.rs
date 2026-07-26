//! **Where the Export window goes**: above the project window that asked, and gone when that
//! window goes.
//!
//! It is a native **child window** of its opener, like Settings — ordered above it, travelling
//! with it, while the opener stays fully interactive. What is different is the **absence of a
//! single-instance rule**: Settings is one panel showing app-wide state, so a second ⌘, can
//! only mean "focus it". An export window is opened *on a result* and carries that result's
//! snapshot, schema and sort, so focusing an open one would show the wrong run. Every press of
//! Download opens a window on the run in front of the user, and each closes itself when its
//! write lands.
//!
//! Like Settings, the whole thing runs on the renderer, in one callback: that is the only place
//! that knows both the id of the window that asked (`post_callback`) and every live window (the
//! [`RendererContext`], which is what pinning one above another needs).

use freya::prelude::*;
use freya::winit::window::WindowId;

use crate::apps::export::{ExportApp, ExportTarget};
use crate::apps::project::contexts::EngineCtx;
use crate::platform::windows::{register, WindowKind};
use crate::state::AppCtx;

/// Open an Export window on `target`, pinned above the window that asked.
///
/// `platform` is taken from the caller's component scope (the results toolbar's render), which
/// is both how the callback learns *which* window asked and why this can be called from an
/// event handler with no scope of its own.
pub fn open_export(platform: Platform, app: AppCtx, engine: EngineCtx, target: ExportTarget) {
    // The receiver is dropped: the work happens inside the callback and nothing waits on it.
    drop(platform.post_callback(move |owner, ctx| {
        let id = ctx.launch_window(ExportApp::window(
            app.clone(),
            engine.clone(),
            target.clone(),
            owner,
        ));
        // Registered here rather than left to the window's own `use_register_window`, which
        // can only learn its id a render and a round trip later — until then the window would
        // be invisible to `is_last`, and a project closing in that gap would think it was the
        // last one.
        register(app.windows, id, WindowKind::Export { owner });
        ctx.set_window_parent(id, Some(owner));
    }));
}

/// Tie this Export window to its owner for as long as it lives: close when the owner closes.
/// Call once in the Export window root.
///
/// **Closing with the owner is ours, not AppKit's** — the same reason Settings needs this. A
/// child window is closed by its parent, but AppKit does that behind winit's back: the
/// `NSWindow` goes and Freya, which only removes a window on a close it was asked for, keeps a
/// live scope for a window that is no longer on screen. Expressing it in the registry's terms
/// (the owner leaving closes this window through Freya's own path) also covers the platforms
/// where the child relationship is a no-op.
///
/// There is no pin to clear on the way out, unlike Settings: this window's owner is recorded in
/// its own registry entry, which goes when it does.
pub fn use_export_pin(app: AppCtx) {
    let platform = use_hook(Platform::get);
    let windows = app.windows;
    let mut me = use_state(|| None::<WindowId>);
    use_hook(move || {
        let platform = Platform::get();
        spawn(async move {
            if let Ok(id) = platform.post_callback(|id, _| id).await {
                me.set(Some(id));
            }
        });
    });
    use_side_effect(move || {
        let registry = windows.read();
        // Before this window's own id lands there is nothing to look up — that is the frame
        // between mounting and the renderer answering, not a closed owner.
        let Some(id) = *me.read() else {
            return;
        };
        let owner = registry.by_id().get(&id).and_then(|kind| match kind {
            WindowKind::Export { owner } => Some(*owner),
            _ => None,
        });
        if owner.is_some_and(|owner| !registry.is_open(owner)) {
            platform.close_current_window();
        }
    });
}
