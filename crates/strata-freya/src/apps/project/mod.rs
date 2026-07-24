//! The main **project** window: root shell + (coming, 1b onward) its per-window Radio
//! station (`state/`), feature `views/` (workbench · sidebar · inspector · drawer), and
//! the palette command registry (`commands.rs`). `mod.rs` is wiring only — private
//! submodules, re-exported.

mod close;
mod project;
mod contexts;
mod query;
mod state;
mod views;
pub mod model;

pub use project::ProjectApp;
pub use views::{
    CancelButtonThemePreference, CellViewThemePreference, DataGridThemePreference,
    ExplainPlanThemePreference, HeaderBarThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference,
};
