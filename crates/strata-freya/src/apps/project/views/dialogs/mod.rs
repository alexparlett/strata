//! The project window's modal dialogs. Each dialog is its own component, mounted early
//! at the window root (right after `ContextMenuViewer`) so that — in document order —
//! its key barrier precedes every feature listener while it is open.

mod close_confirm;
mod drop_confirm;
mod open_prompt;
mod profile_confirm;

pub use close_confirm::CloseConfirm;
pub use drop_confirm::{DropConfirm, DropTarget};
pub use open_prompt::OpenPrompt;
pub use profile_confirm::{use_profile_actions, ProfileActions, ProfileConfirm, ProfileTarget};
