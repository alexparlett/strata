//! Modal **form drafts** — the transient, UI-editable state of the table-config dialog,
//! distinct from the persisted definitions it produces.
//!
//! The export dialog's draft used to live here too. It went with P4-10: the Freya export
//! window owns its own `ExportDraft`, which is where a UI draft belongs — this crate is pure
//! serde defs, "exactly what `.strata/project.json` stores" (`AGENTS.md` §2), and a modal's
//! working state is neither persisted nor shared. `ConfigForm` is the same shape and will go
//! the same way with P4-11; it stays only because it is still the only definition there is.

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
