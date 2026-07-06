//! The **project domain model** — catalog definitions, query tabs, and history
//! that make up a project. This is the persistable core, serialized to a single
//! `<name>.psproj` JSON file; app/session/UI state lives in `crate::state`.
//!
//! Only *definitions* are durable. For tables/views the `columns`/`status` are
//! runtime and `#[serde(skip)]`-ped — re-derived when the engine re-registers a
//! project on open. Reference model: table `sources` are absolute paths.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::{ColumnInfo, QueryOutput};
use crate::plan::{PlanTab, QueryPlan};
use crate::query_error::QueryError;

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
#[derive(Serialize, Deserialize)]
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
#[derive(Serialize, Deserialize)]
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

/// Per-tab, ephemeral query output — **never serialized**. The results panel
/// derives its whole state (grid / plan / error / running / pager) from the active
/// tab's `TabRun`, and the engine reducer routes each event to the owning tab by
/// `ws_id`. Same runtime-fields-on-a-durable-struct pattern as `CatalogTable`'s
/// `columns` / `status`.
pub struct TabRun {
    pub result: Option<QueryOutput>,
    pub query_error: Option<QueryError>,
    pub plan: Option<QueryPlan>,
    pub plan_tab: PlanTab,
    pub plan_raw: bool,
    pub running: bool,
    pub pending_req: Option<u64>,
    /// 1-based page into the snapshot.
    pub page: usize,
    pub page_size: usize,
    pub result_search: String,
}

impl Default for TabRun {
    fn default() -> Self {
        Self {
            result: None,
            query_error: None,
            plan: None,
            plan_tab: PlanTab::default(),
            plan_raw: false,
            running: false,
            pending_req: None,
            page: 1,
            page_size: 100,
            result_search: String::new(),
        }
    }
}

/// A query tab: its name + SQL buffer (persisted) plus its ephemeral `run`
/// (results — not persisted). `id` is runtime (reassigned on load).
#[derive(Serialize, Deserialize)]
pub struct Workspace {
    #[serde(skip)]
    pub id: u64,
    pub name: String,
    pub sql: String,
    #[serde(skip)]
    pub run: TabRun,
}

/// One past query run. `id` is runtime (reassigned on load).
#[derive(Serialize, Deserialize)]
pub struct HistoryItem {
    #[serde(skip)]
    pub id: u64,
    pub sql: String,
    pub ts_label: String,
    pub ms: u128,
    pub rows: usize,
    pub ok: bool,
}

/// A named SQL snippet stored in the project — distinct from a `CatalogView`
/// (which is a real DataFusion view). Re-opened in a query tab, not queryable
/// by name.
#[derive(Serialize, Deserialize)]
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
#[derive(Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    // catalog
    pub tables: Vec<CatalogTable>,
    pub views: Vec<CatalogView>,
    pub saved_queries: Vec<SavedQuery>,
    // workspaces
    pub workspaces: Vec<Workspace>,
    pub active_ws: usize,
    #[serde(skip)]
    pub next_ws_id: u64,
    // history
    pub history: Vec<HistoryItem>,
    #[serde(skip)]
    pub next_hist: u64,
    // last window geometry (absent on new / never-moved projects)
    #[serde(default)]
    pub window: Option<WindowGeom>,
}

impl Project {
    /// An empty project: no catalog, one blank query tab, no history.
    pub fn empty() -> Self {
        Project {
            name: "untitled".into(),
            tables: Vec::new(),
            views: Vec::new(),
            saved_queries: Vec::new(),
            workspaces: vec![Workspace {
                id: 1,
                name: "query 1".into(),
                sql: String::new(),
                run: TabRun::default(),
            }],
            active_ws: 0,
            next_ws_id: 2,
            history: Vec::new(),
            next_hist: 1,
            window: None,
        }
    }

    /// Write the project to `path` as pretty JSON (the `.psproj` manifest).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// Load a project from a `.psproj` file, normalizing runtime ids/counters.
    pub fn load(path: &Path) -> Result<Project, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut project: Project = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        project.normalize();
        Ok(project)
    }

    /// Rebuild the runtime id counters after a load (ids aren't serialized) and
    /// guarantee at least one tab with a valid `active_ws`.
    fn normalize(&mut self) {
        if self.workspaces.is_empty() {
            self.workspaces.push(Workspace {
                id: 0,
                name: "query 1".into(),
                sql: String::new(),
                run: TabRun::default(),
            });
        }
        for (i, w) in self.workspaces.iter_mut().enumerate() {
            w.id = i as u64 + 1;
        }
        self.next_ws_id = self.workspaces.len() as u64 + 1;
        for (i, h) in self.history.iter_mut().enumerate() {
            h.id = i as u64 + 1;
        }
        self.next_hist = self.history.len() as u64 + 1;
        if self.active_ws >= self.workspaces.len() {
            self.active_ws = self.workspaces.len() - 1;
        }
    }
}
