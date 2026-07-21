//! The project window's feature views (workbench · sidebar · inspector · drawer). Real, keeper
//! components built to the `design-handoff/` comps — grown in place, never thrown away.

mod workbench;
mod header;

pub use header::{HeaderBar, HeaderBarThemePreference};
pub use workbench::{
    DataGridThemePreference, StatusBarThemePreference, TabBarThemePreference, TabThemePreference,
    Workbench,
};