//! The project window's feature views (workbench · sidebar · inspector · drawer). Real, keeper
//! components built to the `design-handoff/` comps — grown in place, never thrown away.

mod dialogs;
mod header;
mod workbench;

pub use dialogs::CloseConfirm;
pub use header::{HeaderBar, HeaderBarThemePreference};
pub use workbench::{
    CancelButtonThemePartial, CancelButtonThemePreference, CellViewThemePreference,
    DataGridThemePreference, ExplainPlanThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference, Workbench,
};
