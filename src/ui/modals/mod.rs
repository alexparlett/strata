//! App-global overlays — one submodule each. Every one is an always-mounted
//! **host** (`CmdkHost`, `SettingsHost`, `ExportHost`, `ConfigHost`) that reads the
//! per-window overlay store (`crate::overlays`) and renders its window/dialog only
//! when open. The root mounts the hosts; triggers flip the store.

mod close_confirm;
mod command_palette;
mod config;
mod export;
mod open_prompt;
mod settings;

pub use close_confirm::CloseConfirmHost;
pub use command_palette::CmdkHost;
pub use config::ConfigHost;
pub use export::ExportHost;
pub use open_prompt::OpenPromptHost;
pub use settings::SettingsHost;
