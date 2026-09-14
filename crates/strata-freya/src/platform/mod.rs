//! Platform glue that belongs to the *app*, not to any one window: the window model (which
//! windows are open, how a project is opened, quit vs. close), the open path that decides
//! which window an open lands in, and the child windows' pinning — Settings' single-instance
//! pin, and the owner binding that bounds an Export or Configure window's lifetime by the
//! project subtree whose handles it holds ([`owner`]).

pub mod configure;
pub mod export;
pub mod open;
pub mod owner;
pub mod settings;
pub mod source;
pub mod windows;

pub use configure::open_configure;
pub use export::open_export;
pub use open::{create_global_open, FocusedOpen, OpenCtx, OpenTarget};
pub use owner::{use_owner_pin, Subtree};
pub use settings::{open_settings, use_settings_pin};
pub use source::open_source;
pub use windows::{
    close_this_window, create_global_windows, end_quit, is_quitting, open_project,
    pick_project_folder, quit, quit_windows, request_close, resolve_project_folder, resolve_recent,
    use_register_window, WindowKind, WindowRegistry, Windows,
};

/// **Every window draws its own title bar**, and this is the attribute half of that — the same
/// decision reached two different ways, because the two platforms take it away differently.
///
/// On **macOS** the frame stays and is made see-through: transparent titlebar, full-size content
/// view, hidden title. AppKit keeps drawing the traffic lights, which the fork's
/// `with_traffic_light_inset` then moves down into our strip, and it keeps owning resize.
///
/// **Elsewhere** there is no such treatment — a WM either decorates a window or it does not — so
/// the decoration comes off outright. That is the whole reason the rest of the chrome exists:
/// [`crate::components::chrome::WindowControls`] draws the buttons AppKit would have, and
/// `BorderlessPlugin` (registered at launch in `main`) puts the resize borders back. Take either
/// away and an undecorated window is one the user cannot close or resize.
///
/// Component tests stay platform-independent either way: this is called from a `WindowConfig`
/// builder, and `freya-testing` never builds one.
pub fn window_attributes(
    attrs: freya::winit::window::WindowAttributes,
) -> freya::winit::window::WindowAttributes {
    #[cfg(target_os = "macos")]
    {
        use freya::winit::platform::macos::WindowAttributesExtMacOS;
        attrs
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true)
    }
    #[cfg(not(target_os = "macos"))]
    attrs.with_decorations(false)
}
