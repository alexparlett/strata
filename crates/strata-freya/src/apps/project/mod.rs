//! The main **project** window: root shell + its per-window Radio station (`state/`), feature
//! `views/` (workbench · sidebar · inspector · drawer · palette), and the palette command
//! registry (`commands.rs` — an attribute-macro'd impl block in rmcp's shape, generating an
//! enum so dispatch is total; the port plan's "trait-object, valin-style" note is overturned,
//! see that module). `mod.rs` is wiring only — private submodules, re-exported.

mod app;
mod close;
mod commands;
/// `pub` for the Export window: it is its own OS window, so it can't inherit this window's
/// context and instead carries an [`EngineCtx`](contexts::EngineCtx) clone as a launch value.
pub mod contexts;
pub mod model;
mod query;
mod state;
mod views;

/// [`window_geometry`](app::window_geometry) is `pub` for every path that opens a project
/// window: a window's size and position can only be set as it is created, so they are a launch
/// input the caller resolves — off the render thread, and with a deadline.
pub use app::{window_geometry, window_geometry_blocking, ProjectApp};
pub use close::{CloseGuard, CloseTarget};
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
    refresh_catalog, refresh_table, use_registrations, Catalog, CatalogRescan, ProjChan,
    ProjectState, RegistrationsCtx,
};
/// The window's one statement fold, for the **Configure** window: a memory table is created by a
/// statement rather than registered from a def (IT-01), and it folds its report exactly as the
/// editor's own Run does.
pub use state::{settle, use_settle, Settle};
/// The *values* behind the four handles above, for a **child window's tests**.
///
/// A window that is not this one carries `Catalog`, `CatalogRescan` and `ReportCtx` as launch
/// values, so it never names what is inside them — but a test that stands one of those windows up
/// has to create them, and `CatalogState` has no `Default` to hide behind (`Settled(0)` is a
/// deliberate seed; see its doc). `#[cfg(test)]` rather than a widened module, because this is
/// the whole of the need and it must not become a production coupling: the list above is what a
/// child window may actually reach for.
#[cfg(test)]
pub use state::{CatalogState, Log, PersistFaults, ScanRequest};
/// The data-source editor request slot, for the **Configure** window (W7 · 04): its CONNECTION
/// picker's *New data source…* sets this window's slot and stops, exactly as the pane's own `+`
/// does, so the editor still opens from the one place that holds the handles for it.
pub use views::SourceRequest;
pub use views::{
    CancelButtonThemePreference, CatalogThemePreference, CellViewThemePreference,
    ChartThemePreference, ChatThemePreference, CommandPaletteThemePreference,
    DataGridThemePreference, DrawerThemePreference, ExplainPlanThemePreference,
    HeaderBarThemePreference, InspectorThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference,
};
