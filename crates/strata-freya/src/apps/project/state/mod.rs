//! The per-window stores (Radio): the **Session** (open tabs + arrangement) and the
//! **Project** (the open project's catalog defs — the save targets).
//! See `docs/FREYA_STATE_ARCHITECTURE.md` §2–§4.

mod catalog;
mod channel;
mod history;
mod hooks;
mod project;
mod session;

pub use catalog::{use_catalog_selection, use_init_catalog_selection};
pub use channel::Chan;
pub use history::use_history_recording;
pub use hooks::{
    resolve_launch_root, use_autosave, use_init_history, use_init_project, use_init_session,
};
pub use project::{ProjChan, ProjectState, Reg};
pub use session::SessionState;
