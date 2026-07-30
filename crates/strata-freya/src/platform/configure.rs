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
    // The receiver is dropped: the work happens inside the callback and nothing waits on it.
    drop(platform.post_callback(move |owner, ctx| {
        // Focus-if-open, on this window's own target. Peeked inside the callback rather than
        // read at the call site, so the answer is the registry as it is *now* — the press that
        // opened the menu and the press that chose the item are different moments.
        // Checked against the renderer's own window map, not just the registry: an entry is
        // added eagerly below, while its removal rides the window's `use_register_window` drop,
        // so a window that went before that hook resolved its id would leave an entry naming
        // nothing. Treating a dangling entry as "not open" makes that self-healing — the same
        // guard Settings needs, for the same reason.
        let open = launch
            .app
            .windows
            .peek()
            .by_id()
            .iter()
            // **Keyed by owner *and* target.** A target names a def, and a def belongs to one
            // project — two windows on different projects can both hold a table called `events`,
            // and matching on the name alone hands the second one the first project's def and
            // then writes to its store. One owner window shows one project, so the owner is what
            // says which — the project itself is not matched here, and does not need to be: a
            // window whose owner has since re-rooted has already had its close *requested* by its
            // pin ([`crate::platform::owner`]), a cycle before any press can arrive, so it is
            // never a candidate by the time this runs. Note that is event ordering and **not** the
            // dangling-entry filter below, which tests the renderer's live window map and so still
            // contains a window whose close is merely queued.
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
            launch.rescan,
            launch.catalog,
            launch.engine.clone(),
            launch.target.clone(),
            launch.log,
            owner,
        ));
        // Registered here rather than left to the window's own `use_register_window`, which can
        // only learn its id a render and a round trip later — until then the window would be
        // invisible to the focus-if-open check above, so two quick presses would open two.
        register(
            launch.app.windows,
            id,
            WindowKind::Configure {
                owner,
                target: launch.target.clone(),
            },
        );
        ctx.set_window_parent(id, Some(owner));
    }));
}
