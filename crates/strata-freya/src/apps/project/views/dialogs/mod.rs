//! The project window's modal dialogs. Each dialog is its own component, mounted early
//! at the window root (right after `ContextMenuViewer`) so that — in document order —
//! its key barrier precedes every feature listener while it is open.

mod close_confirm;

pub use close_confirm::CloseConfirm;
