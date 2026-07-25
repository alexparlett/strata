//! Platform glue that belongs to the *app*, not to any one window: today the window
//! model (which windows are open, how a project is opened, quit vs. close) and the open
//! path that decides which window an open lands in. The Settings and Export windows join
//! [`windows`] as they land.

pub mod open;
pub mod windows;

pub use open::{create_global_open, FocusedOpen, OpenCtx, OpenTarget};
pub use windows::{
    close_this_window, create_global_windows, end_quit, is_quitting, open_project,
    pick_project_folder, quit, quit_windows, resolve_project_folder, resolve_recent,
    use_register_window, WindowKind, WindowRegistry,
};
