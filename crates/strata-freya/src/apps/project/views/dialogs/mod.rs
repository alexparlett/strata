//! The project window's modal dialogs. Each dialog is its own component, mounted early
//! at the window root (right after `ContextMenuViewer`) so that — in document order —
//! its key barrier precedes every feature listener while it is open. The one exception is
//! `load_failed`: not a barrier over features but the whole project subtree's fault arm,
//! deliberately non-modal because there is nothing behind it to protect — see its module
//! doc before copying either shape for a new dialog.

mod close_confirm;
mod drop_confirm;
mod load_failed;
mod open_prompt;
mod profile_confirm;

pub use close_confirm::CloseConfirm;
pub use drop_confirm::{DropConfirm, DropTarget};
pub use load_failed::ProjectLoadFailed;
pub use open_prompt::OpenPrompt;
pub use profile_confirm::{use_profile_actions, ProfileActions, ProfileConfirm, ProfileTarget};
