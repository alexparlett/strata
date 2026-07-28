//! The project window's feature views (workbench · sidebar · inspector · drawer). Real, keeper
//! components built to the `design-handoff/` comps — grown in place, never thrown away.

mod configure_launch;
mod dialogs;
mod drawer;
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
    ProfileConfirm, ProfileTarget,
};
pub use drawer::DrawerThemePreference;
pub use header::{HeaderBar, HeaderBarThemePreference};
pub use inspector::InspectorThemePreference;
pub use keeper::RequestKeepers;
pub use shell::Shell;
pub use sidebar::CatalogThemePreference;
/// The `.strata` def-write funnel, re-exported for the Configure window (see `apps::project`).
pub use workbench::editor::actions::persisted;
pub use workbench::{
    CancelButtonThemePartial, CancelButtonThemePreference, CellViewThemePreference,
    DataGridThemePreference, ExplainPlanThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference, Workbench,
};
