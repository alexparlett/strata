//! App-global overlays — one submodule each. Every one is an always-mounted
//! **host** (`CmdkHost`, `ExportHost`, `ConfigHost`) that reads the per-window
//! overlay store (`crate::overlays`) and renders its window/dialog only when open.
//! The root mounts the hosts; triggers flip the store. (Settings moved to its own
//! OS window — `crate::ui::settings` — in W1.)

mod close_confirm;
mod command_palette;
mod config;
mod engine_restart;
mod export;
mod open_prompt;
mod profile_confirm;
mod running_close;

pub use close_confirm::CloseConfirmHost;
pub use command_palette::CmdkHost;
pub use config::ConfigHost;
pub use engine_restart::EngineRestartHost;
pub use export::ExportHost;
pub use open_prompt::OpenPromptHost;
pub use profile_confirm::ProfileConfirmHost;
pub use running_close::RunningCloseHost;
