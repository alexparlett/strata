//! The main **project** window: root shell + (coming, 1b onward) its per-window Radio
//! station (`state/`), feature `views/` (workbench · sidebar · inspector · drawer), and
//! the palette command registry (`commands.rs`). `mod.rs` is wiring only — private
//! submodules, re-exported.

mod close;
/// `pub` for the Export window: it is its own OS window, so it can't inherit this window's
/// context and instead carries an [`EngineCtx`](contexts::EngineCtx) clone as a launch value.
pub mod contexts;
pub mod model;
mod project;
mod query;
mod state;
mod views;

pub use close::{CloseGuard, CloseTarget};
pub use project::ProjectApp;
pub use views::{
    CancelButtonThemePreference, CatalogThemePreference, CellViewThemePreference,
    DataGridThemePreference, DrawerThemePreference, ExplainPlanThemePreference,
    HeaderBarThemePreference, InspectorThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference,
};
