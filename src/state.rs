//! Central app state (held in one `Signal<AppState>`). This file defines the
//! state and its empty constructor; the durable project model lives in
//! `crate::project`. Dev builds open the bundled `sample/` project on launch.

use tokio::sync::mpsc::UnboundedSender;

use crate::engine::Command;
// The project domain model lives in `crate::project`; re-exported here so the
// familiar `crate::state::{CatalogTable, Project, …}` paths keep working.
pub use crate::project::{
    CatalogTable, CatalogView, HistoryItem, Origin, Project, RegStatus, SavedQuery,
};
// A tab's data now lives in the reactive session store; re-exported so the
// familiar `crate::state::Workspace` path keeps working.
pub use crate::session::Workspace;
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
    pub format: String, // csv / json / parquet / arrow / clipboard
    pub name: String,
    pub scope: String,     // "all" | "page"
    pub csv_delim: String, // comma | tab | semicolon | pipe
    pub csv_header: bool,
    pub csv_null: String,            // empty | null | nan
    pub pq_compression: String,      // zstd | snappy | gzip | brotli | lz4 | none
    pub pq_level: u32,               // compression level (codec-dependent)
    pub clip_format: String,         // markdown | tsv | csv | json
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
            clip_format: "markdown".into(),
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
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CatalogKind {
    Table,
    View,
    Query,
}

/// A closed query tab, retained so it can be reopened (⇧⌘T). Capped at 20.
pub struct ClosedTab {
    pub name: String,
    pub sql: String,
}

/// A panel edge the user can drag to resize.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResizeTarget {
    Sidebar,
    Inspector,
    Editor,
    Log,
}

/// An in-progress panel drag: axis, direction, the pointer anchor and the size
/// captured when the drag began.
#[derive(Clone)]
pub struct Resizing {
    pub target: ResizeTarget,
    pub axis_x: bool,
    pub sign: f64,
    pub origin: f64,
    pub start: f64,
    pub min: f64,
    pub max: f64,
}

/// Left-nav category in the Settings modal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsCat {
    Appearance,
    DataDisplay,
    System,
    Keymap,
}

pub struct AppState {
    // engine
    pub cmd_tx: Option<UnboundedSender<Command>>,
    // the open project (catalog, workspaces, history — the persisted part)
    pub project: Project,
    pub type_color_cells: bool,
    // layout
    pub sidebar_open: bool,
    pub inspector_open: bool,
    // project management (where the open project lives on disk + recents)
    pub project_path: Option<std::path::PathBuf>,
    pub recent_projects: Vec<crate::config::RecentProject>,
    // catalog filter (ephemeral UI)
    pub filter: String,
    // results — the per-tab query output (grid / plan / error / running / pager)
    // lives in the `crate::runs::RUNS` store, keyed by workspace id (the active
    // one from `crate::session::active_id`). Only these window-global bits stay
    // here: the request-id source, the
    // pager-dropdown open flag, and the column-inspector selection.
    pub next_req: u64,
    pub page_size_open: bool,
    pub selected_col: Option<(String, String)>,
    // status (ephemeral) — `status_kind` drives the status-bar dot colour and
    // must stay in step with `status_text` (set both via `set_status`).
    pub status_text: String,
    pub status_kind: LogKind,
    // bottom drawer (History + Events tabs)
    pub log: Vec<LogEvent>,
    pub log_open: bool,
    pub log_tab: LogTab,
    pub next_log: u64,
    // resizable panels (px)
    pub sidebar_w: f64,
    pub inspector_w: f64,
    pub editor_h: f64,
    pub log_h: f64,
    pub resizing: Option<Resizing>,
    pub closed_tabs: Vec<ClosedTab>,
    /// The engine's registered SQL functions (built-ins + UDFs), pushed once on
    /// startup (`engine::Event::Functions`, A9/F5). Read by the SQL language
    /// service (`crate::sql`) for completion + validation.
    pub functions: crate::sql::FunctionCatalog,
}

impl AppState {
    pub fn existing_table_names(&self) -> std::collections::BTreeSet<String> {
        self.project
            .tables
            .iter()
            .map(|t| t.name.clone())
            .chain(self.project.views.iter().map(|v| v.name.clone()))
            .collect()
    }

    /// The base, empty workspace: no catalog, one blank query tab, no results.
    /// Dev builds replace this by opening the bundled `sample/` project.
    pub fn empty() -> Self {
        AppState {
            cmd_tx: None,
            project: Project::empty(),
            type_color_cells: true,
            sidebar_open: true,
            inspector_open: true,
            project_path: None,
            recent_projects: Vec::new(),
            filter: String::new(),
            next_req: 1,
            page_size_open: false,
            selected_col: None,
            status_text: "Ready · DataFusion 54 · open a project or add a table to begin".into(),
            status_kind: LogKind::Ok,
            log: Vec::new(),
            log_open: false,
            log_tab: LogTab::History,
            next_log: 1,
            sidebar_w: 288.0,
            inspector_w: 292.0,
            editor_h: 240.0,
            log_h: 188.0,
            resizing: None,
            closed_tabs: Vec::new(),
            functions: crate::sql::FunctionCatalog::default(),
        }
    }

    /// Append an entry to the Event Log (newest first, capped at 200).
    /// Set the status-bar text + its severity together (keeps the status dot in
    /// step with the text — see `status_kind`).
    pub fn set_status(&mut self, kind: LogKind, text: impl Into<String>) {
        self.status_kind = kind;
        self.status_text = text.into();
    }

    pub fn push_log(&mut self, kind: LogKind, msg: impl Into<String>) {
        self.push_log_err(kind, msg, None, None);
    }

    /// Like `push_log`, but attaches a structured error (the Events-row detail /
    /// results error view) and the owning query tab `ws` (Problems grouping + jump,
    /// S23).
    pub fn push_log_err(
        &mut self,
        kind: LogKind,
        msg: impl Into<String>,
        err: Option<QueryError>,
        ws: Option<u64>,
    ) {
        let id = self.next_log;
        self.next_log += 1;
        self.log.insert(
            0,
            LogEvent {
                id,
                kind,
                msg: msg.into(),
                ts: now_hms(),
                err,
                open: false,
                ws,
            },
        );
        self.log.truncate(200);
    }
}

/// Wall-clock `HH:MM:SS` (UTC) for log timestamps — avoids a chrono dependency.
fn now_hms() -> String {
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
