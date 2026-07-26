//! Platform glue that belongs to the *app*, not to any one window: the window model (which
//! windows are open, how a project is opened, quit vs. close), the open path that decides
//! which window an open lands in, and the Settings window's single-instance pinning. The
//! Export window joins [`windows`] as it lands.

pub mod open;
pub mod settings;
pub mod windows;

pub use open::{create_global_open, FocusedOpen, OpenCtx, OpenTarget};
pub use settings::{open_settings, use_settings_pin};
pub use windows::{
    close_this_window, create_global_windows, end_quit, is_quitting, open_project,
    pick_project_folder, quit, quit_windows, resolve_project_folder, resolve_recent,
    use_register_window, WindowKind, WindowRegistry,
};
