//! The per-window stores (Radio): the **Session** (open tabs + arrangement) and the
//! **Project** (the open project's catalog defs — the save targets).
//! See `docs/FREYA_STATE_ARCHITECTURE.md` §2–§4.

mod channel;
mod hooks;
mod project;
mod session;

pub use channel::Chan;
pub use hooks::{use_init_project, use_init_session};
pub use project::{ProjChan, ProjectState};
pub use session::{Origin, QueryTab, ResultsView, SessionState, TabId};
