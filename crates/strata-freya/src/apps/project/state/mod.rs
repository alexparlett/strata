//! The per-window **Session** store (Radio): the open tabs + their arrangement.
//! See `docs/FREYA_STATE_ARCHITECTURE.md` §3–§4.

mod channel;
mod hooks;
mod session;

pub use channel::Chan;
pub use hooks::use_init_session;
pub use session::{ArtifactKey, Origin, QueryTab, SessionState, TabId};
