//! The per-window stores (Radio): the **Session** (open tabs + arrangement) and the
//! **Project** (the open project's catalog defs — the save targets), plus the two satellites that
//! need no channels of their own — query [`history`], the [`log`] behind the drawer's Events
//! tab, and the [`agents`] satellite behind the sidebar's Agents pane. See
//! `docs/FREYA_STATE_ARCHITECTURE.md` §2–§4 (the stores) and §8 (the satellites).

mod agent;
mod agents;
mod catalog;
mod channel;
mod diagnostics;
mod engine_config;
mod history;
mod hooks;
mod log;
mod persist;
mod project;
mod session;

/// The window's half of agent access: the ask/notice driver (AA-03, re-pointed by AA-03b),
/// and the satellite it records into.
pub use agent::use_agent_bridge;
pub use agents::{use_init_agents, AgentRun, Agents, AgentsCtx, ConnectedAgent};
pub use catalog::{
    catalog_settled, use_catalog, use_catalog_rescan, use_catalog_selection,
    use_init_catalog_selection, Catalog, CatalogRescan, CatalogSelection,
};
/// Only tests name these: they stand the catalog's context signals up by hand, where the window
/// goes through `use_init_catalog` / `use_init_catalog_rescan`. Production code reaches
/// `CatalogState` through `use_catalog()`'s methods, never by naming the type.
#[cfg(test)]
pub use catalog::{CatalogState, ScanRequest, ScanScope};
pub use channel::Chan;
pub use diagnostics::use_diagnostics;
pub use engine_config::{use_engine_config, use_engine_restart, EngineRestart};
/// Only tests name the satellite itself: they stand its context signal up by hand, where the
/// window goes through `use_init_history`.
#[cfg(test)]
pub use history::History;
pub use history::{clear_history, use_history_recording, HistoryCtx};
pub use hooks::{
    load_project, refresh_catalog, refresh_table, use_autosave, use_init_history, use_init_project,
    use_init_session, Loaded,
};
/// Only tests name the log itself: they stand its context signal up by hand, where the window
/// goes through `use_init_log`. Production code holds a [`LogCtx`] and appends through
/// [`log_event`].
#[cfg(test)]
pub use log::Log;
pub use log::{log_event, use_init_log, use_run_logging, LogCtx, LogLevel};
/// Only the Project scope's own tests name the file enum directly; production code reaches it
/// through the funnel, which is the point — a writer names its store, not a path.
#[cfg(test)]
pub use persist::ProjectFile;
/// The defs write is the only one that leaves `state/` — every catalog mutation site is a view
/// (or, for Configure, another window's). The session and history writers live in here beside
/// their stores, and reach `persist` directly.
pub use persist::{
    persisted_defs, use_init_faults, use_report, FaultsCtx, PersistFaults, ReportCtx,
};
pub use project::{FaultKind, ProjChan, ProjectState, Reg};
/// The catalog's rows themselves. Nothing outside this module names them at runtime — every
/// consumer reads them through [`ProjectState`]'s own fields — but a test that builds a store
/// **inline** has to, which is what AGENTS.md §1 asks for instead of bending a signature to be
/// testable (the command palette's index is tested exactly this way).
#[cfg(test)]
pub use project::{TableRow, ViewInfo, ViewRow};
pub use session::{ProblemGroup, SessionState, Stamp};
