//! Platform glue that belongs to the *app*, not to any one window: today the window
//! model (which windows are open, how a project is opened, quit vs. close). The Settings
//! and Export windows join [`windows`] as they land.

pub mod windows;

pub use windows::{
    close_this_window, create_global_windows, end_quit, is_quitting, open_project,
    pick_and_open_project, pick_project_folder, quit, quit_windows, resolve_project_folder,
    use_register_window, WindowKind, WindowRegistry,
};
