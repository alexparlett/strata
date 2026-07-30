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
/// The window's engine generation — for [`platform::owner`](crate::platform::owner), which
/// bounds a child window's life by the mount of this window's project subtree that it borrowed
/// its handles from. Safe for a child to hold precisely because it is owned by the *window*
/// rather than by that subtree, so it survives the remount it causes.
pub use state::EngineRestart;
/// The window's event log (P3-13), for the Export window: it is a separate OS window, so it
/// carries the handle as a launch value and records its outcome into the project window's log
/// — which is where the user is looking when the export window has closed itself.
pub use state::{log_event, LogCtx, LogLevel};
/// The `.strata` write funnel and where it reports, for the same window: a def written by
/// Configure is persisted the way every other def mutation is, its answer is checked rather than
/// assumed, and a failed write raises the same Problems row it would from here.
pub use state::{persisted_defs, use_report, ReportCtx};
/// The catalog store, its scan request and the pass that serves it — `pub` for the **Configure**
/// window, which is its own OS window and so carries the station as a launch value rather than
/// inheriting this one's context. It writes the def and asks for the pass; the driver here runs
/// it, which is what keeps "make the engine match the defs" a single implementation.
pub use state::{
    refresh_catalog, refresh_table, Catalog, CatalogRescan, ProjChan, ProjectState, Reg,
};
pub use views::{
    CancelButtonThemePreference, CatalogThemePreference, CellViewThemePreference,
    DataGridThemePreference, DrawerThemePreference, ExplainPlanThemePreference,
    HeaderBarThemePreference, InspectorThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference,
};
