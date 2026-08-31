//! **Where the Configure window goes**: above the project window that asked, gone when that
//! window goes, and **one per target**.
//!
//! Like Export it is a native child window of its opener, ordered above it while the opener
//! stays interactive. What is different is the single-instance rule, and it lands between the
//! other two windows' rules for a reason worth keeping:
//!
//! - **Settings** is one app-wide panel over shared state, so a second ⌘, can only mean focus.
//! - **Export** is opened *on a result* and carries that run's snapshot, so focusing an open one
//!   would show the wrong run — it has no such rule at all.
//! - **Configure** is opened on a **def**, which is shared, mutable state. Two windows on one
//!   def would both `upsert_table` and both persist, so the second would silently revert the
//!   first — the same reason two windows cannot share a project. So "already open" means focus,
//!   *keyed by the target*: two different tables at once is fine, one table twice is not.
//!
//! How long it lives is not here: this window holds four `ProjectRoot`-scoped handles, so it is
//! pinned to that **subtree** rather than to a window id, by the rule Export shares
//! ([`crate::platform::owner`]).

use freya::prelude::*;

use crate::apps::configure::{ConfigureApp, ConfigureLaunch};
use crate::platform::windows::{register, WindowKind};

/// Open a Configure window on `launch.target`, pinned above the window that asked — or focus the
/// one already open on that target.
///
/// `platform` is taken from the caller's component scope (the catalog row's render), which is
/// both how the callback learns *which* window asked and why this can be called from an event
/// handler with no scope of its own.
pub fn open_configure(platform: Platform, launch: ConfigureLaunch) {
    drop(platform.post_callback(move |owner, ctx| {
        let open = launch
            .app
            .windows
            .peek()
            .by_id()
            .iter()
            .find(|(_, kind)| {
                matches!(kind, WindowKind::Configure { owner: o, target }
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

        let id = ctx.launch_window(ConfigureApp::window(
            launch.app.clone(),
            launch.project,
            launch.subtree.clone(),
            launch.registrations,
            launch.rescan,
            launch.catalog,
            launch.engine.clone(),
            launch.target.clone(),
            launch.report,
            launch.editor,
            owner,
        ));
        register(
            launch.app.windows,
            id,
            WindowKind::Configure {
                owner,
                target: launch.target,
            },
        );
        ctx.set_window_parent(id, Some(owner));
    }));
}
