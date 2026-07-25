//! Opening a project **from the launcher**: the two actions that stand this window down
//! once the project window is up.
//!
//! The open itself is [`platform::open_project`] and the picker is
//! [`platform::pick_and_open_project`]'s — the shared window paths, so a project that
//! already has a window is focused here exactly as in the header's switcher. What is
//! launcher-specific is only the *after*: this window exists because there was nothing to
//! look at, so it closes as soon as there is.

use freya::prelude::*;

use crate::platform;
use crate::state::AppCtx;

/// Open the recent at `path` (the stored
/// [`RecentProject::path`](strata_core::config::RecentProject::path)) and close the
/// launcher behind it.
///
/// A recent whose folder has since been moved or deleted can't be opened: the entry is
/// dropped ([`platform::resolve_recent`]), so the pressed row disappears, and the launcher
/// **stays up** — there is nothing to hand over to.
pub fn open_and_close(app: AppCtx, path: String) {
    let platform = Platform::get();
    spawn(async move {
        let Some(root) = platform::resolve_recent(app.config, &path) else {
            return;
        };
        platform::open_project(platform.clone(), app, root).await;
        platform.close_current_window();
    });
}

/// The OPEN action (and ⌘O / File ▸ Open… while the launcher is focused): pick a folder,
/// open it, stand down.
pub fn pick_and_open(app: AppCtx) {
    let platform = Platform::get();
    let pick = platform::pick_project_folder(&app);
    spawn(async move {
        let Some(root) = pick.await else {
            return;
        };
        platform::open_project(platform.clone(), app, root).await;
        platform.close_current_window();
    });
}
