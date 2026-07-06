//! Overlays / modals — one submodule per overlay. Components are re-exported so
//! call sites keep using `ui::modals::ConfigModal` etc.

mod cell;
mod command_palette;
mod config;
mod export;
mod project_menu;
mod remove;
mod settings;

pub use cell::CellPopover;
pub use command_palette::CommandPalette;
pub use config::ConfigModal;
pub use export::ExportModal;
pub use project_menu::ProjectMenu;
pub use remove::RemoveConfirm;
pub use settings::SettingsModal;
