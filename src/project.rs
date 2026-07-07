//! The **project domain model** — catalog definitions, query tabs, and history
//! that make up a project. Persisted as a `.strata/` directory: the durable
//! definitions in `project.json` (committed) + the working session (tabs, history,
//! geometry) in `session.json` (gitignored). App/global state lives in
//! `crate::state`.
//!
//! Only *definitions* are durable. For tables/views the `columns`/`status` are
//! runtime and `#[serde(skip)]`-ped — re-derived when the engine re-registers a
//! project on open. Reference model: table `sources` are absolute paths.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::ColumnInfo;

/// File names inside the `.strata/` project directory.
const PROJECT_JSON: &str = "project.json";
const SESSION_JSON: &str = "session.json";

/// Registration lifecycle of a catalog table (runtime, not persisted).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum RegStatus {
    /// A freshly-loaded or -added table, awaiting engine registration.
    #[default]
    Loading,
    Ready,
    Failed,
}

/// Accept partition columns as either the legacy name-only `["year","month"]`
/// (→ typed `Utf8`) or the current typed `[["year","Int32"], …]` form, so old
/// `.psproj` files keep loading. Serialization always emits the typed form.
fn de_partition_cols<'de, D>(d: D) -> Result<Vec<(String, String)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Col {
        Named(String),
        Typed(String, String),
    }
    Ok(Vec::<Col>::deserialize(d)?
        .into_iter()
        .map(|c| match c {
            Col::Named(n) => (n, "Utf8".to_string()),
            Col::Typed(n, t) => (n, t),
        })
        .collect())
}

/// One logical table (a DataFusion `ListingTable` over many source paths).
#[derive(Serialize, Deserialize, Clone)]
pub struct CatalogTable {
    pub name: String,
    #[serde(skip)]
    pub meta: String,
    pub format: String,
    pub sources: Vec<String>,
    /// Hive partition columns as `(name, arrow_type)` — the persisted source of
    /// truth for deterministic reload (types aren't re-detected).
    #[serde(default, deserialize_with = "de_partition_cols")]
    pub partition_cols: Vec<(String, String)>,
    #[serde(skip)]
    pub columns: Vec<ColumnInfo>,
    #[serde(skip)]
    pub open: bool,
    #[serde(skip)]
    pub status: RegStatus,
    #[serde(skip)]
    pub error: Option<String>,
}

/// A saved, query-backed catalog view (a real DataFusion `CREATE VIEW`).
#[derive(Serialize, Deserialize, Clone)]
pub struct CatalogView {
    pub name: String,
    pub sql: String,
    #[serde(skip)]
    pub meta: String,
    #[serde(skip)]
    pub columns: Vec<ColumnInfo>,
    #[serde(skip)]
    pub open: bool,
}

/// What a tab is bound to — drives ⌘S behaviour and the dirty indicator.
#[derive(Clone, Serialize, Deserialize, Default, PartialEq)]
pub enum Origin {
    /// Ad-hoc SQL, not tied to a named catalog object.
    #[default]
    Scratch,
    /// Editing catalog view `name`.
    View(String),
    /// Editing saved query `name`.
    SavedQuery(String),
}

/// One past query run. `id` is runtime (reassigned on load).
#[derive(Serialize, Deserialize, Clone)]
pub struct HistoryItem {
    #[serde(skip)]
    pub id: u64,
    pub sql: String,
    pub ts_label: String,
    pub ms: u128,
    pub rows: usize,
    pub ok: bool,
    /// Cancelled by the user (S14) — distinct from ok / failed.
    #[serde(default)]
    pub cancelled: bool,
}

/// A named SQL snippet stored in the project — distinct from a `CatalogView`
/// (which is a real DataFusion view). Re-opened in a query tab, not queryable
/// by name.
#[derive(Serialize, Deserialize, Clone)]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
    pub meta: String,
}

/// Saved window geometry (physical pixels) so a project reopens where it was
/// last left. Restored *before* the window is created (see `crate::window`).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct WindowGeom {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// The open project — the durable core serialized to `<name>.psproj`. Global
/// prefs and ephemeral UI live on `AppState`, not here.
pub struct Project {
    pub name: String,
    // catalog
    pub tables: Vec<CatalogTable>,
    pub views: Vec<CatalogView>,
    pub saved_queries: Vec<SavedQuery>,
    // history
    pub history: Vec<HistoryItem>,
    pub next_hist: u64,
    // last window geometry (absent on new / never-moved projects)
    pub window: Option<WindowGeom>,
}

/// The committed definitions — `.strata/project.json`.
#[derive(Serialize, Deserialize, Default)]
struct DefsFile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    tables: Vec<CatalogTable>,
    #[serde(default)]
    views: Vec<CatalogView>,
    #[serde(default)]
    saved_queries: Vec<SavedQuery>,
}

/// The local working session — `.strata/session.json` (gitignored). Its workspace
/// portion mirrors [`crate::session::Session`]; on load it's handed to
/// `crate::session::load` to populate this window's reactive session store.
#[derive(Serialize, Deserialize, Default)]
struct SessionFile {
    #[serde(default)]
    workspaces: Vec<crate::session::Workspace>,
    #[serde(default)]
    active: crate::session::WorkspaceId,
    #[serde(default)]
    next_id: crate::session::WorkspaceId,
    #[serde(default)]
    view_clock: u64,
    #[serde(default)]
    history: Vec<HistoryItem>,
    #[serde(default)]
    window: Option<WindowGeom>,
}

fn write_json<T: Serialize>(path: &Path, val: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(val).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Write `.strata/.gitignore` (ignoring the local session) if it's not there yet.
fn ensure_gitignore(dir: &Path) {
    let gi = dir.join(".gitignore");
    if !gi.exists() {
        let _ = fs::write(gi, "session.json\n");
    }
}

impl Project {
    /// An empty project: no catalog, no history. Workspaces live in the reactive
    /// [`crate::session`] store, not here — the caller resets it separately (see
    /// `crate::session::reset_blank`).
    pub fn empty() -> Self {
        Project {
            name: "untitled".into(),
            tables: Vec::new(),
            views: Vec::new(),
            saved_queries: Vec::new(),
            history: Vec::new(),
            next_hist: 1,
            window: None,
        }
    }

    /// Write both files (definitions + session) into the `.strata/` dir, creating
    /// it and its `.gitignore` if needed. For full / explicit saves.
    pub fn save_all(&self, dir: &Path) -> Result<(), String> {
        self.save_defs(dir)?;
        self.save_session(dir)?;
        ensure_gitignore(dir);
        Ok(())
    }

    /// Write only the committed definitions (`project.json`).
    pub fn save_defs(&self, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let defs = DefsFile {
            name: self.name.clone(),
            tables: self.tables.clone(),
            views: self.views.clone(),
            saved_queries: self.saved_queries.clone(),
        };
        write_json(&dir.join(PROJECT_JSON), &defs)
    }

    /// Write only the local working session (`session.json`). The workspace portion
    /// comes from this window's reactive [`crate::session`] store (its snapshot);
    /// history + geometry come from the project.
    pub fn save_session(&self, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let snap = crate::session::snapshot();
        let sess = SessionFile {
            workspaces: snap.workspaces,
            active: snap.active,
            next_id: snap.next_id,
            view_clock: snap.view_clock,
            history: self.history.clone(),
            window: self.window,
        };
        write_json(&dir.join(SESSION_JSON), &sess)
    }

    /// Load from a `.strata/` dir (merging the two files). `Err` when the project
    /// file doesn't exist (caller then scaffolds a new project).
    ///
    /// **Side effect:** as well as returning the [`Project`] (catalog defs + history
    /// + geometry), this populates *this window's* reactive [`crate::session`] store
    /// from the session file's workspaces (via `crate::session::load`, which repairs
    /// ids and guarantees a valid active workspace). Workspaces no longer live on
    /// `Project`, so opening a project must go through here to seed the editor.
    pub fn load_from_dir(dir: &Path) -> Result<Project, String> {
        let pj = dir.join(PROJECT_JSON);
        if pj.exists() {
            let defs: DefsFile = read_json(&pj)?;
            let sess: SessionFile = read_json(&dir.join(SESSION_JSON)).unwrap_or_default();
            let mut project = Project {
                name: defs.name,
                tables: defs.tables,
                views: defs.views,
                saved_queries: defs.saved_queries,
                history: sess.history,
                next_hist: 0,
                window: sess.window,
            };
            // History ids are runtime — assign them 1..n on load (as the old
            // `normalize` did).
            for (i, h) in project.history.iter_mut().enumerate() {
                h.id = i as u64 + 1;
            }
            project.next_hist = project.history.len() as u64 + 1;
            // Populate this window's reactive session store from the loaded
            // workspaces (repairs legacy/duplicate ids, ensures ≥1 workspace).
            crate::session::load(crate::session::Session {
                workspaces: sess.workspaces,
                active: sess.active,
                next_id: sess.next_id,
                view_clock: sess.view_clock,
            });
            return Ok(project);
        }
        Err("no project files".into())
    }

    /// Whether a project already exists at `dir`: a `.strata/project.json`, or a
    /// legacy single-file project in the parent folder waiting to be migrated.
    /// Distinguishes "open existing" from "scaffold new" (so a corrupt file is
    /// never silently overwritten).
    pub fn exists_at(dir: &Path) -> bool {
        dir.join(PROJECT_JSON).exists()
    }

    /// Read just the saved window geometry from a `.strata/` dir (to size a window
    /// before it's created). `None` if absent.
    pub fn peek_window(dir: &Path) -> Option<WindowGeom> {
        read_json::<SessionFile>(&dir.join(SESSION_JSON))
            .ok()
            .and_then(|s| s.window)
    }

}
