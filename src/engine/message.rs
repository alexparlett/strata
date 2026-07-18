//! The engine **protocol** — the `Command`s the UI sends and the `Event`s it gets
//! back — plus its own request/result payloads (`TableSpec`, `TableMeta`). The shared
//! data vocabulary these messages *carry* (`ColumnInfo`, `Stat`, `QueryOutput`, …) is
//! not the engine's; it lives in `crate::model`.

use std::collections::BTreeMap;

use datafusion::arrow::record_batch::RecordBatch;

use crate::model::{Cell, ColumnInfo, QueryOutput};
use crate::plan::QueryPlan;

/// What a (re)registration learned about a table: its columns, plus the free row count
/// (`None` when the source doesn't report one).
#[derive(Clone, Debug, PartialEq)]
pub struct TableMeta {
    pub columns: Vec<ColumnInfo>,
    pub rows: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct TableSpec {
    pub name: String,
    pub paths: Vec<String>,
    pub format: String,
    pub partitions: Vec<(String, String)>,
}

pub enum Command {
    Register(TableSpec),
    Deregister {
        table: String,
    },
    /// Re-infer every registered table's schema — picks up new columns or a changed
    /// partition scheme. Files, rows, and partition *values* are already live (each
    /// scan re-`LIST`s; we run no `ListFilesCache`), so this only re-registers to
    /// refresh the inferred schema. Emits a `Registered` per table.
    RefreshCatalog,
    /// Full-scan profile of one table (D4) — see [`crate::profile`]. Runs spawned and
    /// keyed by table, so profiles of different tables run concurrently.
    Profile {
        table: String,
    },
    /// Abort an in-flight profile.
    CancelProfile {
        table: String,
    },
    CreateView {
        name: String,
        sql: String,
    },
    DropView {
        name: String,
    },
    /// Run a query → spool a snapshot → return page 1 + total.
    Query {
        req_id: u64,
        ws_id: u64,
        sql: String,
        page_size: usize,
    },
    /// Run an `EXPLAIN [ANALYZE]` and return its parsed plan tree (no snapshot).
    Explain {
        req_id: u64,
        ws_id: u64,
        sql: String,
    },
    /// Abort the in-flight Query/Explain for `ws_id`, but only if it's still request
    /// `req_id` (S14).
    Cancel {
        ws_id: u64,
        req_id: u64,
    },
    /// Read a page from the workspace's existing snapshot (no recompute). `sort` =
    /// `(column name, ascending)` applied as an `ORDER BY` over the snapshot before the
    /// page window; `None` = snapshot order (Rz6).
    FetchPage {
        ws_id: u64,
        page: usize,
        page_size: usize,
        sort: Option<(String, bool)>,
    },
    /// Drop one workspace's snapshot (table + temp file) — e.g. on tab close.
    CleanupWorkspace {
        ws_id: u64,
    },
    /// Remove all snapshots (e.g. on app exit).
    CleanupAll,
    /// Apply new engine config overrides live (W2). The `ConfigOptions` keys take
    /// effect on the running context immediately; the two `datafusion.runtime.*`
    /// keys can't change on a live `RuntimeEnv`, so a change there emits a `Notice`
    /// (they apply when the window is reopened).
    SetEngineConfig(BTreeMap<String, String>),
    /// Write a workspace's snapshot to a file (or, with `partition_cols`, a
    /// Hive-partitioned directory) via `COPY … TO`.
    Export {
        ws_id: u64,
        path: String,
        format: String,
        all: bool,
        page: usize,
        page_size: usize,
        csv_delimiter: char,
        csv_header: bool,
        csv_null: String,
        pq_compression: String,
        pq_level: u32,
        partition_cols: Vec<String>,
        keep_partition: bool,
    },
}

pub enum Event {
    Registered {
        table: String,
        path: String,
        result: Result<TableMeta, String>,
    },
    Deregistered {
        table: String,
    },
    /// A profile scan finished (D4). The row's `profiling` flag clears either way.
    Profiled {
        table: String,
        result: Result<crate::profile::CatalogProfile, String>,
    },
    ViewChanged {
        name: String,
        sql: String,
        dropped: bool,
        /// The base tables the view reads (D10) — empty on a drop or a failure.
        deps: Vec<String>,
        /// Every name the view inlines — its referenced views, plus table-alias / CTE
        /// noise the UI filters against the known views. Empty on a drop or failure.
        aliases: Vec<String>,
        result: Result<Vec<ColumnInfo>, String>,
    },
    QueryResult {
        req_id: u64,
        ws_id: u64,
        /// `(display page, page `RecordBatch`)` — the batch is the type-aware source for the
        /// results Copy / Export-to-clipboard (Rz4). Kept out of `QueryOutput` so the grid's
        /// per-render clone never touches it (it's Arc-cheap to carry).
        result: Result<(QueryOutput, RecordBatch), String>,
    },
    /// Result of an `EXPLAIN [ANALYZE]` — a parsed plan tree or an error.
    ExplainResult {
        req_id: u64,
        ws_id: u64,
        result: Result<QueryPlan, String>,
    },
    /// A Query/Explain was cancelled (S14) — clears the tab's running state.
    QueryCancelled {
        req_id: u64,
        ws_id: u64,
        elapsed_ms: u128,
    },
    PageResult {
        ws_id: u64,
        page: usize,
        /// `(display rows, page `RecordBatch`)` — see `QueryResult`.
        result: Result<(Vec<Vec<Cell>>, RecordBatch), String>,
    },
    /// Result of an export: `Ok((path, rows_written))` or an error message.
    Exported {
        result: Result<(String, usize), String>,
    },
    /// The engine's registered function names (built-ins + any UDFs), sent once on
    /// startup so the UI SQL language service (S26/S7/S25) can complete + validate
    /// real functions. Names only; signatures/detail can follow later.
    Functions {
        scalar: Vec<String>,
        aggregate: Vec<String>,
        window: Vec<String>,
    },
    Notice(String),
    /// A saved `datafusion.runtime.*` change can't be applied to the running engine
    /// (its `RuntimeEnv` is fixed at build) — the UI offers a window restart (W2).
    EngineRestartRequired,
}
