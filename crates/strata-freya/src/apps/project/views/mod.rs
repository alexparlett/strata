//! The project window's feature views (workbench · sidebar · inspector · drawer). Real, keeper
//! components built to the `design-handoff/` comps — grown in place, never thrown away.

mod dialogs;
mod drawer;
mod header;
mod inspector;
mod keeper;
mod rail;
mod shell;
mod sidebar;
mod workbench;

pub use dialogs::{
    use_profile_actions, CloseConfirm, DropConfirm, DropTarget, OpenPrompt, ProfileActions,
    ProfileConfirm, ProfileTarget,
};
pub use header::{HeaderBar, HeaderBarThemePreference};
pub use inspector::InspectorThemePreference;
pub use keeper::RequestKeepers;
pub use shell::Shell;
pub use sidebar::CatalogThemePreference;
pub use workbench::{
    CancelButtonThemePartial, CancelButtonThemePreference, CellViewThemePreference,
    DataGridThemePreference, ExplainPlanThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference, Workbench,
};
