//! Multi-window coordination.
//!
//! Each project opens in its **own** window — its own `VirtualDom`, hence its own
//! `AppState` + DataFusion engine. Open project windows are tracked in a
//! thread-local registry (all window lifecycle runs on the main thread) so we can
//! focus a sibling when one closes and cycle between them with ⌘`.
//!
//! The launcher is a separate window, opened *only* when "Close project" closes
//! the last project window — never from an OS close-button.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use dioxus::desktop::tao::window::WindowId;
use dioxus::desktop::{Config, LogicalSize, WeakDesktopContext, WindowBuilder};
use dioxus::prelude::*;

use crate::app::{ProjectRoot, ProjectRootProps};
use crate::ui::launcher::LauncherRoot;

thread_local! {
    /// Live project windows in creation order. Weak so a closed window's
    /// `DesktopService` can actually drop.
    static WINDOWS: RefCell<Vec<WeakDesktopContext>> = RefCell::new(Vec::new());
}

/// Register the current window as a project window; returns its id so it can be
/// de-registered from `use_drop`.
pub fn register_current_window() -> WindowId {
    let ctx = dioxus::desktop::window();
    let id = ctx.id();
    WINDOWS.with(|w| w.borrow_mut().push(Rc::downgrade(&ctx)));
    id
}

/// Remove a window from the registry.
pub fn unregister_window(id: WindowId) {
    WINDOWS.with(|w| {
        w.borrow_mut()
            .retain(|weak| weak.upgrade().map(|c| c.id() != id).unwrap_or(false))
    });
}

/// Number of live project windows (the launcher isn't counted).
pub fn project_window_count() -> usize {
    WINDOWS.with(|w| {
        let mut b = w.borrow_mut();
        b.retain(|weak| weak.upgrade().is_some());
        b.len()
    })
}

/// Focus some other live project window — used when the current one closes so a
/// sibling comes to the front.
pub fn focus_another_window() {
    let me = dioxus::desktop::window().id();
    WINDOWS.with(|w| {
        for weak in w.borrow().iter() {
            if let Some(ctx) = weak.upgrade() {
                if ctx.id() != me {
                    ctx.set_focus();
                    return;
                }
            }
        }
    });
}

/// ⌘` — focus the next project window after the current one (wrapping).
pub fn cycle_to_next_window() {
    let me = dioxus::desktop::window().id();
    WINDOWS.with(|w| {
        let live: Vec<_> = w.borrow().iter().filter_map(|x| x.upgrade()).collect();
        if live.len() < 2 {
            return;
        }
        let cur = live.iter().position(|c| c.id() == me).unwrap_or(0);
        live[(cur + 1) % live.len()].set_focus();
    });
}

/// Open `open_path` (empty string → a fresh untitled project) in a new window.
///
/// `new_window` queues creation synchronously and returns a handle-future we
/// don't need, so dropping it is safe (the window is already scheduled).
pub fn spawn_project_window(open_path: String) {
    let cfg = project_window_config_for(&open_path);
    let dom = VirtualDom::new_with_props(ProjectRoot, ProjectRootProps { open_path });
    let _ = dioxus::desktop::window().new_window(dom, cfg);
}

/// The current window's geometry (physical px) for persisting into a project.
pub fn current_window_geom() -> Option<crate::project::WindowGeom> {
    geom_of(&dioxus::desktop::window())
}

/// Geometry of the registered window with `id`, if still live. Reads from the
/// thread-local registry so it works from a `use_wry_event_handler` (which has no
/// dioxus scope, so `window()` isn't available there).
pub fn window_geom_by_id(id: WindowId) -> Option<crate::project::WindowGeom> {
    WINDOWS.with(|w| {
        let ctx = w
            .borrow()
            .iter()
            .filter_map(|weak| weak.upgrade())
            .find(|c| c.id() == id)?;
        geom_of(&ctx)
    })
}

fn geom_of(ctx: &dioxus::desktop::DesktopContext) -> Option<crate::project::WindowGeom> {
    let size = ctx.inner_size();
    let pos = ctx.outer_position().ok()?;
    Some(crate::project::WindowGeom {
        x: pos.x,
        y: pos.y,
        w: size.width,
        h: size.height,
    })
}

/// Open the welcome / launcher window.
pub fn open_launcher_window() {
    let dom = VirtualDom::new(LauncherRoot);
    let _ = dioxus::desktop::window().new_window(dom, launcher_window_config());
}

/// The project directory for a chosen folder: `<folder>/.strata`. Whether it
/// already exists, needs scaffolding, or has a legacy single-file project to
/// migrate is decided later by `Project::load_from_dir` / `exists_at`.
pub fn resolve_project_dir(folder: &Path) -> PathBuf {
    folder.join(".strata")
}

// ---- window configuration ----

/// The runtime window icon (window + taskbar on Windows/Linux; macOS shows the
/// bundle / dock icon instead). Built from a pre-decoded 256×256 RGBA blob so we
/// don't pull in a runtime image-decoding dependency.
fn strata_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    use dioxus::desktop::tao::window::Icon;
    const RGBA: &[u8] = include_bytes!("../assets/icons/strata-256.rgba");
    Icon::from_rgba(RGBA.to_vec(), 256, 256).ok()
}

fn base_window(w: f64, h: f64, min_w: f64, min_h: f64) -> WindowBuilder {
    let window = WindowBuilder::new()
        .with_title("Strata")
        .with_window_icon(strata_window_icon())
        .with_inner_size(LogicalSize::new(w, h))
        .with_min_inner_size(LogicalSize::new(min_w, min_h));
    #[cfg(target_os = "macos")]
    let window = {
        use dioxus::desktop::tao::dpi::LogicalPosition;
        use dioxus::desktop::tao::platform::macos::WindowBuilderExtMacOS;
        window
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true)
            .with_traffic_light_inset(LogicalPosition::new(13.0, 21.0))
    };
    window
}

/// Config for a project window, restoring the project's saved size + position if
/// present (peeked from the `.psproj` before the window is created).
pub fn project_window_config_for(path: &str) -> Config {
    let mut win = base_window(1360.0, 860.0, 900.0, 600.0);
    if let Some(g) = peek_geom(path) {
        use dioxus::desktop::tao::dpi::{PhysicalPosition, PhysicalSize};
        win = win
            .with_inner_size(PhysicalSize::new(g.w, g.h))
            .with_position(PhysicalPosition::new(g.x, g.y));
    }
    Config::new()
        .with_window(win)
        .with_as_child_window()
        .with_background_color((11, 14, 19, 255))
}

/// Read just the saved window geometry from a project file (empty path / new
/// project → none).
fn peek_geom(path: &str) -> Option<crate::project::WindowGeom> {
    if path.is_empty() {
        return None;
    }
    crate::project::Project::peek_window(std::path::Path::new(path))
}

pub fn launcher_window_config() -> Config {
    // The launcher opens at its minimum size (default == min): compact, resizable
    // larger if wanted.
    Config::new()
        .with_window(base_window(880.0, 600.0, 880.0, 600.0))
        .with_as_child_window()
        .with_background_color((10, 13, 18, 255))
}

/// macOS: paint the NSWindow background dark so a resize doesn't flash white
/// (the webview repaints a beat behind the frame).
#[cfg(target_os = "macos")]
pub fn paint_ns_background(r: f64, g: f64, b: f64) {
    use dioxus::desktop::tao::platform::macos::WindowExtMacOS;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    let ns_window = dioxus::desktop::window().ns_window() as *mut Object;
    unsafe {
        let color: *mut Object = msg_send![
            class!(NSColor),
            colorWithSRGBRed: r green: g blue: b alpha: 1.0f64
        ];
        let _: () = msg_send![ns_window, setBackgroundColor: color];
    }
}
