//! **Where the Export window goes**: above the project window that asked.
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
//!
//! How long it lives is not here either: it holds the opener's event log, which belongs to that
//! window's project subtree, so it is pinned to the **subtree** rather than to a window id, by
//! the rule Configure shares ([`crate::platform::owner`]).

use freya::prelude::*;

use crate::apps::export::{ExportApp, ExportLaunch};
use crate::platform::windows::{register, WindowKind};

/// Open an Export window on `launch.target`, pinned above the window that asked.
///
/// `platform` is taken from the caller's component scope (the results toolbar's render), which
/// is both how the callback learns *which* window asked and why this can be called from an
/// event handler with no scope of its own.
pub fn open_export(platform: Platform, launch: ExportLaunch) {
    // The receiver is dropped: the work happens inside the callback and nothing waits on it.
    drop(platform.post_callback(move |owner, ctx| {
        let id = ctx.launch_window(ExportApp::window(
            launch.app.clone(),
            launch.engine.clone(),
            launch.subtree.clone(),
            launch.target.clone(),
            launch.log,
            owner,
        ));
        // Registered here rather than left to the window's own `use_register_window`, which
        // can only learn its id a render and a round trip later — until then the window would
        // be invisible to `is_last`, and a project closing in that gap would think it was the
        // last one.
        register(launch.app.windows, id, WindowKind::Export);
        ctx.set_window_parent(id, Some(owner));
    }));
}
