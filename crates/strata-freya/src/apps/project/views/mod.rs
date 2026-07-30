//! The project window's feature views (workbench · sidebar · inspector · drawer). Real, keeper
//! components built to the `design-handoff/` comps — grown in place, never thrown away.

mod agent_keeper;
mod configure_launch;
mod dialogs;
pub(super) mod drawer;
mod header;
mod inspector;
mod keeper;
mod loading;
mod rail;
mod shell;
mod sidebar;
/// `pub(super)` for the same reason `drawer` is: the agent bridge (`state::agent`) runs an
/// agent's SQL through the editor's own `actions`, so the tab it lands in holds the text the
/// user can then read, edit and re-run.
pub(super) mod workbench;

pub use agent_keeper::AgentKeepers;
pub use configure_launch::{ConfigureLauncher, ConfigureRequest};
pub use dialogs::{
    use_profile_actions, CloseConfirm, DropConfirm, DropTarget, OpenPrompt, ProfileActions,
    ProfileConfirm, ProfileTarget, ProjectLoadFailed,
};
pub use drawer::DrawerThemePreference;
pub use header::{HeaderBar, HeaderBarThemePreference, WindowDragStrip};
pub use inspector::InspectorThemePreference;
pub use keeper::RequestKeepers;
pub use loading::ProjectLoading;
pub use shell::Shell;
pub use sidebar::CatalogThemePreference;
pub use workbench::{
    CancelButtonThemePartial, CancelButtonThemePreference, CellViewThemePreference,
    DataGridThemePreference, ExplainPlanThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference, Workbench,
};
