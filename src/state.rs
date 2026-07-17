//! Shared UI value types (form drafts, log entries, catalog-menu kinds) used
//! across the app. The durable project model lives in `crate::project`; per-window
//! runtime state lives in focused stores (`crate::session`, `crate::runs`, …).

// The project domain model lives in `crate::project`; re-exported here so the
// familiar `crate::state::{CatalogTable, Project, …}` paths keep working.
pub use crate::project::{
    CatalogTable, CatalogView, Origin, RegStatus,
};
// A tab's data now lives in the reactive session store; re-exported so the
// familiar `crate::state::Workspace` path keeps working.
use crate::query_error::QueryError;

#[derive(Clone)]
pub struct ConfigForm {
    pub editing: Option<String>,
    pub name: String,
    pub format: String,
    pub fmt_open: bool,
    pub sources: Vec<String>,
    pub hive_on: bool,
    pub part_cols: Vec<(String, String)>,
    // --- live scan results (filled by modals::rescan on path/format change) ---
    /// Every provided path is an existing directory → Hive partitioning allowed.
    pub all_dirs: bool,
    /// Data files matched across the current paths.
    pub file_count: usize,
    /// A scan is in flight.
    pub scanning: bool,
    /// Blocking scan problem (format mismatch, missing path, no files).
    pub scan_error: Option<String>,
    /// Hive keys detected under the directories (name, inferred type), in order.
    pub detected_parts: Vec<(String, String)>,
}

impl Default for ConfigForm {
    fn default() -> Self {
        Self {
            editing: None,
            name: String::new(),
            format: "parquet".into(),
            fmt_open: false,
            sources: vec![String::new()],
            hive_on: false,
            part_cols: vec![],
            all_dirs: false,
            file_count: 0,
            scanning: false,
            scan_error: None,
            detected_parts: vec![],
        }
    }
}

#[derive(Clone)]
pub struct ExportForm {
    pub format: String, // csv / json / parquet / arrow
    pub name: String,
    pub scope: String,     // "all" | "page"
    pub csv_delim: String, // comma | tab | semicolon | pipe
    pub csv_header: bool,
    pub csv_null: String,            // empty | null | nan
    pub pq_compression: String,      // zstd | snappy | gzip | brotli | lz4 | none
    pub pq_level: u32,               // compression level (codec-dependent)
    pub partition_cols: Vec<String>, // ordered columns → Hive dir export
    pub keep_partition: bool,        // keep partition columns inside the files
    pub adv_open: bool,              // advanced-options disclosure
}

impl Default for ExportForm {
    fn default() -> Self {
        Self {
            format: "csv".into(),
            name: "query_result".into(),
            scope: "all".into(),
            csv_delim: "comma".into(),
            csv_header: true,
            csv_null: "empty".into(),
            pq_compression: "zstd".into(),
            pq_level: 3,
            partition_cols: Vec::new(),
            keep_partition: false,
            adv_open: false,
        }
    }
}

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

/// What a pending removal targets — drives the confirm dialog's wording and the
/// engine command sent on confirm.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RemoveKind {
    Table,
    View,
}

#[derive(Clone)]
pub struct RemoveTarget {
    pub kind: RemoveKind,
    pub name: String,
}

/// Which catalog section a right-clicked row belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CatalogKind {
    Table,
    View,
    Query,
}

/// Wall-clock `HH:MM:SS` (UTC) for log timestamps — avoids a chrono dependency.
pub(crate) fn now_hms() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}
