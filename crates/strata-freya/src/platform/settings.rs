//! **Where the Settings window goes**: one instance app-wide, pinned above whichever window
//! asked for it.
//!
//! Settings isn't a place you work, it's a panel over the window you *were* working in — and
//! the app has several. So rather than a second free-floating window the user has to hunt
//! for, it is a native **child window** of its opener (`WindowConfig` can't express this, so
//! the fork grew [`set_window_parent`](freya::prelude::WinitPlatformExt::set_window_parent)):
//! it is ordered above the opener and can't be covered by it, it travels with it, and it goes
//! when the opener goes — while the opener stays fully interactive, which is what separates
//! this from a modal sheet. Asking again from *another* window re-points it at that one, so
//! the panel is always over the window you last asked from.
//!
//! [`open_settings`] is the single entry point behind every trigger (the project header's
//! gear, the launcher rail's Settings row, ⌘, in either, and the App menu's Settings item),
//! so "already open" can only ever mean *focus and re-pin*, never a second window.
//!
//! **The whole thing runs on the renderer**, in one callback, because that is the only place
//! that knows both halves at once: `post_callback` hands over the id of the window that asked
//! (a window never learns its own id any other way — see
//! [`use_register_window`](crate::platform::use_register_window)), and the
//! [`RendererContext`] holds every live window, which is what pinning one above another
//! needs.

use freya::prelude::*;

use crate::apps::settings::SettingsApp;
use crate::platform::windows::{register, WindowKind};
use crate::state::AppCtx;

/// Open Settings — or, when it is already open, focus it and re-pin it above the window that
/// asked this time.
///
/// `platform` is taken from the caller's component scope (the gear's render, the key
/// listener's), which is both how the callback learns *which* window asked and why this can
/// be called from an event handler with no scope of its own.
pub fn open_settings(platform: Platform, mut app: AppCtx) {
    drop(platform.post_callback(move |owner, ctx| {
        let open = app
            .windows
            .peek()
            .settings()
            .filter(|id| ctx.windows().contains_key(id));
        if open == Some(owner) {
            return;
        }
        let settings = match open {
            Some(id) => {
                if let Some(window) = ctx.windows_mut().get_mut(&id) {
                    window.window().focus_window();
                }
                id
            }
            None => {
                let id = ctx.launch_window(SettingsApp::window(app.clone()));
                register(app.windows, id, WindowKind::Settings);
                id
            }
        };
        ctx.set_window_parent(settings, Some(owner));
        app.windows.write().pin_settings(Some(owner));
    }));
}

/// Tie the Settings window to its owner for as long as it lives: close with the owner, and
/// clear the registry's pin on the way out. Call once in the Settings window root.
///
/// **Closing with the owner is ours, not AppKit's.** A child window is closed by its parent,
/// but AppKit does that behind winit's back: the `NSWindow` goes and Freya, which only ever
/// removes a window on a close it was asked for, keeps a live scope for one that is no longer
/// on screen. So the rule is expressed in the app's own terms instead — the owner leaving the
/// live registry closes this window through Freya's own path — which also covers the platforms
/// where the child relationship is a no-op.
///
/// The pin is cleared here rather than by whoever closed the window because *every* way it
/// can go ends here: the red button, Cancel, Esc, Apply, a quit, and the owner closing.
///
/// **Focus is not handed back from here**, and can't be: a `Platform` call posts an event
/// tagged with *this* window's id, and the renderer drops events whose window is already gone
/// — which it always is by the time this drop runs, since dropping the window's scope is what
/// runs it. Nothing is lost: AppKit hands a child window's focus back to its parent by itself,
/// which is the relationship `open_settings` set up.
pub fn use_settings_pin(app: AppCtx) {
    let platform = use_hook(Platform::get);
    let mut windows = app.windows;
    use_side_effect(move || {
        let registry = windows.read();
        if registry
            .settings_owner()
            .is_some_and(|id| !registry.is_open(id))
        {
            platform.close_current_window();
        }
    });
    use_drop(move || {
        windows.write().pin_settings(None);
    });
}
