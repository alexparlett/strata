//! Catalog action handlers: table configuration (register/validate), the
//! remove-confirmation flow, and the catalog-row context menu. Called from
//! `action::dispatch`.

use dioxus::prelude::*;

use crate::engine;
use crate::state::{AppState, RemoveKind};

/// Open the Table Config modal for a new external table.
pub fn open_config_new() {
    // The modal seeds a blank draft from the target — nothing to set up here.
    crate::overlays::open_config(crate::overlays::ConfigTarget::New);
}

/// Open the Table Config modal editing an existing table.
pub fn open_config_edit(table: &str) {
    // The modal loads the existing table into its local draft from the target.
    crate::overlays::open_config(crate::overlays::ConfigTarget::Edit(table.to_string()));
}

/// `Action::RegisterTable` — register the config modal's **draft** with the engine.
/// The project store is left untouched: the row data is stashed in the overlay
/// store (`begin_register`), and the engine's `Registered` event builds the real
/// catalog row on success, or surfaces an inline error on failure. No placeholder
/// row is ever written — only a successful register saves the table.
pub fn register_table(state: Signal<AppState>, draft: crate::state::ConfigForm) {
    let base = project_dir(&state.read());
    // Store paths as entered (relative-to-project where the user chose that); hand
    // the engine fully-resolved absolute paths.
    let rel_paths: Vec<String> = draft
        .sources
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let abs_paths: Vec<String> = rel_paths
        .iter()
        .map(|p| resolve_source(base.as_deref(), p))
        .collect();
    let partitions = if draft.hive_on {
        draft.part_cols.clone()
    } else {
        vec![]
    };
    let spec = engine::TableSpec {
        name: draft.name.clone(),
        paths: abs_paths,
        format: draft.format.clone(),
        partitions: partitions.clone(),
    };
    crate::overlays::begin_register(crate::overlays::PendingTable {
        name: draft.name.clone(),
        format: draft.format.clone(),
        sources: rel_paths,
        partition_cols: partitions,
    });
    crate::command!(Register(spec));
}

// ---- remove-confirmation flow ----

/// Confirm a removal (from the sidebar's confirm dialog): drop the view /
/// deregister the table. The engine's `Deregistered` / `ViewChanged{dropped}`
/// event logs the outcome. The dialog's open state is a sidebar-local signal, so
/// there's nothing to close here.
pub fn confirm_remove(mut state: Signal<AppState>, kind: RemoveKind, name: String) {
    match kind {
        RemoveKind::Table => {
            crate::command!(Deregister {
                table: name.clone(),
            });
            state.write().project.tables.retain(|x| x.name != name);
        }
        RemoveKind::View => {
            crate::command!(DropView { name: name.clone() });
            state.write().project.views.retain(|x| x.name != name);
        }
    }
}

/// Load a view's SQL into the active tab (catalog menu → "Edit query").
/// Open a view's SQL in its **own** tab (named after the view), reusing an
/// existing tab of that name rather than overwriting whatever tab is active.
pub fn edit_view(state: Signal<AppState>, name: &str) {
    let sql = state
        .read()
        .project
        .views
        .iter()
        .find(|v| v.name == name)
        .map(|v| v.sql.clone());
    let Some(sql) = sql else {
        return;
    };
    crate::session::open_named(name, sql, crate::project::Origin::View(name.to_string()));
}

// ---- catalog interactions ----

/// Expand/collapse a table row's schema.
pub fn toggle_table_open(mut state: Signal<AppState>, i: usize) {
    let mut w = state.write();
    if let Some(t) = w.project.tables.get_mut(i) {
        t.open = !t.open;
    }
}

/// Expand/collapse a view row's schema.
pub fn toggle_view_open(mut state: Signal<AppState>, i: usize) {
    let mut w = state.write();
    if let Some(v) = w.project.views.get_mut(i) {
        v.open = !v.open;
    }
}

/// Select a column for the inspector (and open the inspector).
pub fn select_column(table: String, column: String) {
    crate::inspector::select(table, column);
    crate::layout::set_inspector_open(true);
}

// ---- source scanning (validation + partition detection) ----

use std::path::{Path, PathBuf};

/// The project directory a `.psproj` lives in — the base for relative source
/// paths. `None` when the project isn't backed by a file yet.
pub fn project_dir(state: &AppState) -> Option<PathBuf> {
    state
        .project_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
}

/// Resolve a (possibly project-relative) source path to an absolute path for the
/// engine / filesystem, joining relative entries onto `base` (the project dir).
/// Absolute paths, and anything with no base, are returned unchanged.
pub fn resolve_source(base: Option<&Path>, p: &str) -> String {
    let path = Path::new(p);
    if path.is_absolute() {
        return p.to_string();
    }
    match base {
        Some(b) => b.join(p).to_string_lossy().into_owned(),
        None => p.to_string(),
    }
}

/// If `abs` sits inside `base`, return it relative to `base` (portable, stored in
/// the project); otherwise return it unchanged. Used when a path is picked/typed.
pub fn relativize(base: Option<&Path>, abs: &str) -> String {
    if let Some(b) = base {
        if let Ok(rel) = Path::new(abs).strip_prefix(b) {
            let r = rel.to_string_lossy();
            if !r.is_empty() {
                return r.into_owned();
            }
        }
    }
    abs.to_string()
}

/// Result of scanning the config modal's source paths.
pub struct ScanResult {
    /// Total data files matched across all paths.
    pub file_count: usize,
    /// True only when *every* provided path is an existing directory (the
    /// precondition for Hive partitioning).
    pub all_dirs: bool,
    /// Detected Hive partition keys with an inferred type, in nesting order.
    pub partition_keys: Vec<(String, String)>,
    /// A blocking problem: a missing path, files that don't match the format, or
    /// no matching files at all. `None` means the paths look registerable.
    pub error: Option<String>,
}

/// File extensions accepted for each table format.
fn format_exts(format: &str) -> &'static [&'static str] {
    match format {
        "parquet" => &["parquet", "pq"],
        "csv" => &["csv", "tsv"],
        "json" => &["json", "ndjson", "jsonl"],
        "arrow" => &["arrow", "feather", "ipc"],
        _ => &[],
    }
}

fn is_glob(p: &str) -> bool {
    p.contains('*') || p.contains('?') || p.contains('[')
}

fn ext_matches(path: &Path, exts: &[&str]) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) => exts.iter().any(|&x| e.eq_ignore_ascii_case(x)),
        None => false,
    }
}

/// Walk a directory (file-count-capped by `budget`) counting files that do /
/// don't match the accepted extensions. Hidden and `_`-prefixed marker files
/// (e.g. `_SUCCESS`, `.crc`) are ignored.
fn count_dir(root: &Path, exts: &[&str], budget: &mut usize) -> (usize, usize) {
    let (mut ok, mut bad) = (0usize, 0usize);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if *budget == 0 {
            break;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            if *budget == 0 {
                break;
            }
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || name.starts_with('_') {
                    continue;
                }
                *budget -= 1;
                if ext_matches(&entry.path(), exts) {
                    ok += 1;
                } else {
                    bad += 1;
                }
            }
        }
    }
    (ok, bad)
}

/// Follow one representative branch of `key=value` directories under `root`,
/// returning the ordered partition keys each with an inferred Arrow type.
fn detect_partitions(root: &Path) -> Vec<(String, String)> {
    let mut keys = Vec::new();
    let mut dir = root.to_path_buf();
    // Bounded — real Hive layouts are only a handful of levels deep.
    for _ in 0..16 {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => break,
        };
        let mut next: Option<PathBuf> = None;
        for entry in rd.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some((k, v)) = name.split_once('=') {
                if !k.is_empty() {
                    keys.push((k.to_string(), infer_type(v)));
                    next = Some(entry.path());
                    break;
                }
            }
        }
        match next {
            Some(p) => dir = p,
            None => break,
        }
    }
    keys
}

/// Cheap value → Arrow type guess for a partition value (a sensible default the
/// user can override).
fn infer_type(v: &str) -> String {
    let is_date = v.len() == 10
        && v.as_bytes().iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    if is_date {
        return "Date".into();
    }
    if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
        return if v.len() > 9 {
            "Int64".into()
        } else {
            "Int32".into()
        };
    }
    "Utf8".into()
}

/// Scan the (non-empty) source paths: validate they exist and their files match
/// `format`, whether they're all directories, and any Hive partition keys.
/// Relative paths are resolved against `base` (the project dir). Pure and
/// blocking, but bounded (20k files), so `modals::rescan` calls it inline.
pub fn scan_sources(paths: &[String], format: &str, base: Option<&Path>) -> ScanResult {
    let exts = format_exts(format);
    let live: Vec<String> = paths
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if live.is_empty() {
        return ScanResult {
            file_count: 0,
            all_dirs: false,
            partition_keys: vec![],
            error: None,
        };
    }

    let mut budget = 20_000usize;
    let (mut total, mut bad) = (0usize, 0usize);
    let mut all_dirs = true;
    let mut first_dir: Option<PathBuf> = None;
    let mut missing: Option<String> = None;

    for p in &live {
        // Error messages show the path the user entered; fs work uses the
        // resolved absolute path.
        let resolved = resolve_source(base, p);
        if is_glob(&resolved) {
            all_dirs = false;
            // Can't enumerate a glob without a glob crate; trust the pattern's
            // own trailing extension if it has one.
            let gp = Path::new(&resolved);
            if gp.extension().is_some() && !ext_matches(gp, exts) {
                bad += 1;
            }
            continue;
        }
        let path = Path::new(&resolved);
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => {
                all_dirs = false;
                missing = Some(p.clone());
                continue;
            }
        };
        if meta.is_dir() {
            if first_dir.is_none() {
                first_dir = Some(path.to_path_buf());
            }
            let (ok, b) = count_dir(path, exts, &mut budget);
            total += ok;
            bad += b;
        } else {
            all_dirs = false;
            if ext_matches(path, exts) {
                total += 1;
            } else {
                bad += 1;
            }
        }
    }

    let partition_keys = if all_dirs {
        first_dir
            .as_deref()
            .map(detect_partitions)
            .unwrap_or_default()
    } else {
        vec![]
    };

    let has_glob = live.iter().any(|p| is_glob(p));
    let error = if let Some(m) = missing {
        Some(format!("Path not found: {m}"))
    } else if bad == 1 {
        Some(format!("1 file doesn't match {format}"))
    } else if bad > 1 {
        Some(format!("{bad} files don't match {format}"))
    } else if total == 0 && !has_glob {
        Some(format!(
            "No {format} files found in the selected path{}",
            if live.len() == 1 { "" } else { "s" }
        ))
    } else {
        None
    };

    ScanResult {
        file_count: total,
        all_dirs,
        partition_keys,
        error,
    }
}
