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
    drop(platform.post_callback(move |owner, ctx| {
        let open = launch
            .app
            .windows
            .peek()
            .by_id()
            .iter()
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
        register(
            launch.app.windows,
            id,
            WindowKind::Connection {
                owner,
                target: launch.target,
            },
        );
        ctx.set_window_parent(id, Some(owner));
    }));
}
