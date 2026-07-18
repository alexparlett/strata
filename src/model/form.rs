//! Modal **form drafts** — the transient, UI-editable state of the table-config and
//! export dialogs, distinct from the persisted definitions they produce.

/// The table-config modal's draft (register / edit an external table).
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

/// The export modal's draft.
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
