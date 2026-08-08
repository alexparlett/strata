//! **Where the connection editor goes**: above the project window that asked, gone when that
//! window goes, and **one per target**.
//!
//! [`crate::platform::configure`]'s rules verbatim, because the two windows have the same shape:
//! each is opened on a **def**, which is shared mutable state, so two windows on one def would
//! both write it and the second would silently revert the first. "Already open" therefore means
//! focus, *keyed by the target* — two different connections at once is fine, one connection twice
//! is not. (Export, by contrast, is opened on a *result* and has no such rule at all.)
//!
//! How long it lives is not here: this window holds `ProjectRoot`-scoped handles, so it is pinned
//! to that **subtree** rather than to a window id ([`crate::platform::owner`]).

use freya::prelude::*;

use crate::apps::connection::{ConnectionApp, ConnectionLaunch};
use crate::platform::windows::{register, WindowKind};

/// Open a connection editor on `launch.target`, pinned above the window that asked — or focus the
/// one already open on that target.
///
/// `platform` is taken from the caller's component scope, which is both how the callback learns
/// *which* window asked and why this can be called from an event handler with no scope of its own.
pub fn open_connection(platform: Platform, launch: ConnectionLaunch) {
    // The receiver is dropped: the work happens inside the callback and nothing waits on it.
    drop(platform.post_callback(move |owner, ctx| {
        // Focus-if-open, on this window's own target. Peeked inside the callback rather than at
        // the call site, so the answer is the registry as it is *now* — the press that opened the
        // menu and the press that chose the item are different moments. Checked against the
        // renderer's own window map as well, so a dangling entry (a window that went before its
        // `use_register_window` drop resolved its id) reads as "not open" rather than swallowing
        // the press.
        let open = launch
            .app
            .windows
            .peek()
            .by_id()
            .iter()
            // **Keyed by owner *and* target.** A target names a def, and a def belongs to one
            // project — two windows on different projects can both hold a connection to
            // `s3://lake`, and matching on the URL alone would hand the second one the first
            // project's def and then write to its store.
            .find(|(_, kind)| {
                matches!(kind, WindowKind::Connection { owner: o, target }
                    if *o == owner && *target == launch.target)
            })
            .map(|(id, _)| *id)
            .filter(|id| ctx.windows().contains_key(id));
        if let Some(id) = open {
            if let Some(window) = ctx.windows_mut().get_mut(&id) {
                window.window().focus_window();
            }
            return;
        }

        let id = ctx.launch_window(ConnectionApp::window(
            launch.app.clone(),
            launch.project,
            launch.subtree.clone(),
            launch.rescan,
            launch.catalog,
            launch.engine.clone(),
            launch.target.clone(),
            launch.report,
            owner,
        ));
        // Registered here rather than left to the window's own `use_register_window`, which can
        // only learn its id a render and a round trip later — until then the window would be
        // invisible to the focus-if-open check above, so two quick presses would open two.
        register(
            launch.app.windows,
            id,
            WindowKind::Connection {
                owner,
                target: launch.target.clone(),
            },
        );
        ctx.set_window_parent(id, Some(owner));
    }));
}
