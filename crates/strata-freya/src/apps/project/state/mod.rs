//! The per-window stores (Radio): the **Session** (open tabs + arrangement) and the
//! **Project** (the open project's catalog defs — the save targets), plus the two satellites that
//! need no channels of their own — query [`history`], the [`log`] behind the drawer's Events
//! tab, and the [`agents`] satellite holding what each agent is working in. See
//! `docs/FREYA_STATE_ARCHITECTURE.md` §2–§4 (the stores) and §8 (the satellites).

mod agent;
mod agents;
mod catalog;
mod channel;
mod chat;
mod chat_send;
mod chat_store;
mod diagnostics;
mod engine_config;
mod history;
mod hooks;
mod log;
mod persist;
mod project;
mod session;
mod statement;

/// The window's half of agent access: the ask/notice driver (AA-03, re-pointed by AA-03b),
/// and the satellite it records into.
pub use agent::use_agent_bridge;
pub use agents::{use_init_agents, Agents, AgentsCtx};
pub use catalog::{
    catalog_settled, use_catalog, use_catalog_rescan, use_catalog_selection,
    use_init_catalog_selection, use_init_remote_scans, use_remote_scans, Catalog, CatalogRescan,
    CatalogSelection, RemoteScans,
};
/// Only tests name these: they stand the catalog's context signals up by hand, where the window
/// goes through `use_init_catalog` / `use_init_catalog_rescan`. Production code reaches
/// `CatalogState` through `use_catalog()`'s methods, never by naming the type.
#[cfg(test)]
pub use catalog::{CatalogState, ScanRequest, ScanScope};
pub use channel::Chan;
/// Only tests name the satellite itself, like the catalog's above: they stand its context signal
/// up by hand, where the window goes through `use_init_chats`.
#[cfg(test)]
pub use chat::Chats;
pub use chat::{
    chats_cap, use_init_chats, Anchor, Block, Chat, ChatId, ChatsCtx, Pick, Reply, RowKey, Step,
    Turn,
};
pub use chat_send::{
    blocked, clear_all, discard, open_stored, seed_pick, send, store, store_shed, AssistantCtx,
    Stores,
};
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
pub use project::{ConnRow, FaultKind, ProjChan, ProjectState, Reg};
/// The remaining catalog rows. A test that builds a store **inline** names these, which is what
/// this codebase asks for instead of bending a signature to be testable (the command palette's index
/// is tested exactly this way); [`ConnRow`] is above because the data-sources tree's walk reads a
/// connection's registration off the row it is already iterating rather than looking it up again.
#[cfg(test)]
pub use project::{TableRow, ViewInfo, ViewRow};
pub use session::{ProblemGroup, QueryTab, SessionState, Stamp};
pub use statement::{settle, use_settle, use_statement_settle, Settle};
