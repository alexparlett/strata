//! The **event log / drawer** vocabulary: a log entry's [`LogKind`] severity, the
//! [`LogTab`] the bottom drawer shows, and a [`LogEvent`] row.

use super::QueryError;

/// Severity of an entry in the Events tab. `Run` (a query started) and `Warn`
/// (e.g. a cancelled query) join the ok/info/error kinds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Ok,
    Info,
    Run,
    Warn,
    Error,
}

/// Which tab the bottom drawer shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogTab {
    History,
    Events,
    Problems,
}

/// One row in the Event Log panel. Fed from engine events (see
/// `app::apply_event`), mirroring the `tracing` records.
#[derive(Clone)]
pub struct LogEvent {
    pub id: u64,
    pub kind: LogKind,
    pub msg: String,
    pub ts: String,
    /// Structured error for expandable error rows (S6 Events-tab expansion).
    /// `None` for ordinary events, which aren't expandable.
    pub err: Option<QueryError>,
    /// Whether this row is expanded in the Events tab.
    pub open: bool,
    /// Owning query tab this event came from, if any. Problems no longer derives
    /// from the log (it reads `crate::diagnostics`); kept as event origin metadata
    /// for a future Events-by-tab grouping. `None` for non-tab events.
    #[allow(dead_code)]
    pub ws: Option<u64>,
}
