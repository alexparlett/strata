//! The project window's feature views (workbench · sidebar · inspector · drawer). Real, keeper
//! components built to the `design-handoff/` comps — grown in place, never thrown away.

mod dialogs;
mod drawer;
mod header;
mod inspector;
mod rail;
mod shell;
mod sidebar;
mod workbench;

pub use dialogs::CloseConfirm;
pub use header::{HeaderBar, HeaderBarThemePreference};
pub use shell::Shell;
pub use sidebar::CatalogThemePreference;
pub use workbench::{
    CancelButtonThemePartial, CancelButtonThemePreference, CellViewThemePreference,
    DataGridThemePreference, ExplainPlanThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference, Workbench,
};
