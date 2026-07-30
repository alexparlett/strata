//! The project window's feature views (workbench · sidebar · inspector · drawer). Real, keeper
//! components built to the `design-handoff/` comps — grown in place, never thrown away.

mod configure_launch;
mod dialogs;
pub(super) mod drawer;
mod header;
mod inspector;
mod keeper;
mod rail;
mod shell;
mod sidebar;
mod workbench;

pub use configure_launch::{ConfigureLauncher, ConfigureRequest};
pub use dialogs::{
    use_profile_actions, CloseConfirm, DropConfirm, DropTarget, OpenPrompt, ProfileActions,
    ProfileConfirm, ProfileTarget, ProjectLoadFailed,
};
pub use drawer::DrawerThemePreference;
pub use header::{HeaderBar, HeaderBarThemePreference, WindowDragStrip};
pub use inspector::InspectorThemePreference;
pub use keeper::RequestKeepers;
pub use shell::Shell;
pub use sidebar::CatalogThemePreference;
pub use workbench::{
    CancelButtonThemePartial, CancelButtonThemePreference, CellViewThemePreference,
    DataGridThemePreference, ExplainPlanThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference, Workbench,
};
