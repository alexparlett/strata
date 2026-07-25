//! The per-window stores (Radio): the **Session** (open tabs + arrangement) and the
//! **Project** (the open project's catalog defs — the save targets).
//! See `docs/FREYA_STATE_ARCHITECTURE.md` §2–§4.

mod catalog;
mod channel;
mod history;
mod hooks;
mod project;
mod session;

/// Only the sidebar's layout tests name the re-scan request type: they stand the catalog's
/// context signals up by hand, where the window goes through `use_init_catalog_rescan`.
#[cfg(test)]
pub use catalog::ScanRequest;
pub use catalog::{
    use_catalog_rescan, use_catalog_scan, use_catalog_selection, use_init_catalog_selection,
};
pub use channel::Chan;
pub use history::use_history_recording;
pub use hooks::{
    refresh_catalog, resolve_launch_root, use_autosave, use_init_history, use_init_project,
    use_init_session,
};
pub use project::{ProjChan, ProjectState, Reg};
pub use session::SessionState;
