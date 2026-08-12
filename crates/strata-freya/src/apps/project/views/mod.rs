//! The project window's feature views (workbench · sidebar · inspector · drawer). Real, keeper
//! components built to the `design-handoff/` comps — grown in place, never thrown away.

mod chat;
mod configure_launch;
mod connection_launch;
mod dialogs;
pub(super) mod drawer;
mod header;
mod inspector;
mod keeper;
mod loading;
mod palette;
mod rail;
mod right_rail;
mod shell;
mod sidebar;
/// `pub(super)` for the same reason `drawer` is: the chat pane's offer cards promote the
/// assistant's SQL into a tab of the user's own through the editor's own `actions`, so it
/// holds the text they can then read, edit and run.
pub(super) mod workbench;

pub use chat::{ask_about, result_anchor, ChatThemePreference};
pub use configure_launch::{ConfigureLauncher, ConfigureRequest};
pub use connection_launch::{ConnectionLauncher, ConnectionRequest};
pub use dialogs::{
    use_profile_actions, ChatConfirm, ChatDrop, CloseConfirm, DropConfirm, DropTarget, OpenPrompt,
    ProfileActions, ProfileConfirm, ProfileTarget, ProjectLoadFailed,
};
pub use drawer::DrawerThemePreference;
pub use header::{HeaderBar, HeaderBarThemePreference, WindowDragStrip};
pub use inspector::InspectorThemePreference;
pub use keeper::RequestKeepers;
pub use loading::ProjectLoading;
pub use palette::{CommandPalette, CommandPaletteThemePreference, PaletteOpen};
pub use shell::Shell;
/// The catalog's actions, for the window's command registry ([`commands`](super::commands)): a
/// palette row that opens a table, a view or a saved query performs the catalog's own gesture,
/// so the two cannot generate different SQL or bind a tab to a different [`Origin`].
///
/// [`Origin`]: strata_model::Origin
pub use sidebar::{open_saved_query, use_catalog_actions, view_row, CatalogActions};
pub use sidebar::{CatalogThemePreference, ConnectionsThemePreference};
/// The editor's shared actions, for the window's command registry
/// ([`commands`](super::commands)): the palette's Run and Save-as-view rows are the same
/// presses ⌘↵ and the Eye button make, gate included.
pub use workbench::editor::actions;
pub use workbench::{
    CancelButtonThemePartial, CancelButtonThemePreference, CellViewThemePreference,
    ChartThemePreference, DataGridThemePreference, ExplainPlanThemePreference,
    RecordViewThemePreference, StatusBarThemePreference, TabBarThemePreference, TabThemePreference,
    Workbench,
};
