//! Overlays / modals — one submodule per overlay. Components are re-exported so
//! call sites keep using `ui::modals::ConfigModal` etc.

mod command_palette;
mod config;
mod export;
mod settings;

pub use command_palette::CmdkHost;
pub use config::ConfigModal;
pub use export::ExportHost;
pub use settings::SettingsHost;
