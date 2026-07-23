//! The per-window **Project** store (Radio): the open project's catalog — the *save
//! targets* (state-arch §2). Each row wraps a pure persisted def ([`TableDef`] /
//! [`ViewDef`]) with what engine registration *learned* about it ([`Reg`]), so the
//! durable and the derived can't blur: `defs()` is a projection, not a clone-and-hope,
//! and invalid combinations (a Ready row carrying an error) are unrepresentable.
//!
//! Identity: **views and tables are addressed by name** — that is their engine/SQL
//! identity (one shared namespace). **Saved queries are addressed by `id`** — their
//! name is only a label. Renames must go through this store so it can keep session-tab
//! origins honest: a view rename rewrites matching `Origin::View` keys (no rename entry
//! point exists yet — when Phase 3 adds one, route it here); a saved-query rename is
//! free (ids don't move). User-entered names compare case-insensitively
//! ([`ProjectState::same_name`]) — DataFusion folds unquoted identifiers — while
//! landing engine answers matches exactly (round-trips of our own strings).
//!
//! Mutations happen through methods (like `SessionState`) via a `write_channel` guard;
//! persistence is [`ProjectState::save_defs`] — called at the def-mutation points
//! (save-as-view, register, drop), never on a timer. The local session file is the
//! session-persistence slice's, not this store's.

use std::path::PathBuf;

use freya::radio::RadioChannel;
use strata_core::engine::{TableMeta, ViewMeta};
use strata_core::project::{self as project_io, name_ord, ProjectDefs};
use strata_model::{CatalogKind, ColumnInfo, SavedQuery, TableDef, ViewDef};
use uuid::Uuid;

/// The Project store's channels — one per catalog section, so a registration landing
/// on one table wakes only table subscribers (the Phase-3 sidebar sections subscribe
/// individually).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum ProjChan {
    /// Project identity: name / root path.
    /// Feature reservoir: subscribed by the header / window title when it shows the
    /// open project (P4-13 UI).
    #[allow(dead_code)]
    Meta,
    Tables,
    Views,
    /// Feature reservoir: subscribed by the sidebar QUERIES section (Phase 3) and
    /// notified by save-as-query (P2-16).
    #[allow(dead_code)]
    Queries,
}

impl RadioChannel<ProjectState> for ProjChan {}

/// What the engine has said about a def so far. One value, so a row can't be `Ready`
/// and carry an error at once, and "loaded but never answered" is a first-class state.
pub enum Reg<T> {
    /// Awaiting the engine's answer (fresh load, or a def just (re)written).
    Loading,
    /// Registered — carrying what registration learned.
    Ready(T),
    /// The engine refused it (missing file, bad path, SQL that didn't plan). The def
    /// still exists — there's just nothing working behind it.
    Failed(String),
}

impl<T> Reg<T> {
    /// The landed answer, if any.
    #[allow(dead_code)]
    pub fn ready(&self) -> Option<&T> {
        match self {
            Reg::Ready(t) => Some(t),
            _ => None,
        }
    }

    /// The failure, if any — the sidebar's problem badge.
    #[allow(dead_code)]
    pub fn error(&self) -> Option<&str> {
        match self {
            Reg::Failed(e) => Some(e),
            _ => None,
        }
    }
}

/// One catalog table: its persisted def + registration state.
pub struct TableRow {
    pub def: TableDef,
    pub reg: Reg<TableMeta>,
}

impl TableRow {
    fn new(def: TableDef) -> Self {
        Self {
            def,
            reg: Reg::Loading,
        }
    }

    /// The row's summary label ("6 cols · 2 partitions") — derived, never stored.
    /// Feature reservoir: rendered by the sidebar rows (Phase 3).
    #[allow(dead_code)]
    pub fn meta_label(&self) -> String {
        match &self.reg {
            Reg::Ready(m) if self.def.partition_cols.is_empty() => {
                format!("{} cols", m.columns.len())
            }
            Reg::Ready(m) => format!(
                "{} cols · {} partitions",
                m.columns.len(),
                self.def.partition_cols.len()
            ),
            Reg::Loading => "loading…".into(),
            Reg::Failed(_) => "failed".into(),
        }
    }
}

/// What creating a view learned, with its aliases already resolved to actual views.
pub struct ViewInfo {
    /// Feature reservoir: the autocomplete symbol catalog (P2-04) + inspector (Phase 3).
    #[allow(dead_code)]
    pub columns: Vec<ColumnInfo>,
    /// The base tables it reads (transitive — the planner inlines nested views).
    /// Feature reservoir: the table-drop warning + profile invalidation (Phase 3).
    #[allow(dead_code)]
    pub deps: Vec<String>,
    /// The views it reads (transitive), resolved from the engine's raw aliases.
    #[allow(dead_code)] // Feature reservoir: the table-drop warning + reload ordering (Phase 3).
    pub view_deps: Vec<String>,
}

/// One catalog view: its persisted def + registration state.
pub struct ViewRow {
    pub def: ViewDef,
    pub reg: Reg<ViewInfo>,
}

impl ViewRow {
    fn new(def: ViewDef) -> Self {
        Self {
            def,
            reg: Reg::Loading,
        }
    }
}

/// The open project. Rows stay sorted by [`name_ord`] on their def names (the load
/// sorts, and every upsert inserts at the sorted slot), so index-addressed rows can't
/// shuffle.
#[derive(Default)]
pub struct ProjectState {
    pub name: String,
    /// The project folder — the parent of its `.strata/` dir, and the base relative
    /// source paths resolve against. `None` = no project on disk (in-memory only).
    pub root: Option<PathBuf>,
    pub tables: Vec<TableRow>,
    pub views: Vec<ViewRow>,
    pub saved_queries: Vec<SavedQuery>,
}

impl ProjectState {
    /// The store for a project loaded (or scaffolded) from `root` — every row starts
    /// `Loading`, awaiting registration.
    pub fn from_defs(defs: ProjectDefs, root: PathBuf) -> Self {
        Self {
            name: defs.name,
            root: Some(root),
            tables: defs.tables.into_iter().map(TableRow::new).collect(),
            views: defs.views.into_iter().map(ViewRow::new).collect(),
            saved_queries: defs.saved_queries,
        }
    }

    /// The durable defs — a pure projection of the rows (what `.strata/project.json`
    /// stores; registration state never travels).
    /// Feature reservoir (with `save_defs` and the def mutations below): consumed by
    /// save-as-view / ⌘S (P2-16) and the catalog sidebar's add/remove flows (Phase 3).
    #[allow(dead_code)]
    pub fn defs(&self) -> ProjectDefs {
        ProjectDefs {
            name: self.name.clone(),
            tables: self.tables.iter().map(|r| r.def.clone()).collect(),
            views: self.views.iter().map(|r| r.def.clone()).collect(),
            saved_queries: self.saved_queries.clone(),
        }
    }

    /// Persist the defs to `.strata/project.json`. Call at def-mutation points
    /// (view/saved-query create · drop · register/deregister). No-op without a root.
    #[allow(dead_code)]
    pub fn save_defs(&self) -> Result<(), String> {
        match &self.root {
            Some(root) => project_io::save_defs(root, &self.defs()),
            None => Ok(()),
        }
    }

    // --- identity ------------------------------------------------------------------

    /// The one name-equality rule for user-entered catalog names: case-insensitive
    /// (DataFusion folds unquoted identifiers).
    pub fn same_name(a: &str, b: &str) -> bool {
        a.eq_ignore_ascii_case(b)
    }

    /// Which section, if any, already owns `name` — tables and views share one SQL
    /// namespace, so a new name must be free in both; saved-query labels only clash
    /// with themselves.
    /// Feature reservoir: save-as / config-modal name validation (P2-16 / P4-11).
    #[allow(dead_code)]
    pub fn name_in_use(&self, name: &str) -> Option<CatalogKind> {
        if self.tables.iter().any(|r| Self::same_name(&r.def.name, name)) {
            Some(CatalogKind::Table)
        } else if self.views.iter().any(|r| Self::same_name(&r.def.name, name)) {
            Some(CatalogKind::View)
        } else if self.saved_queries.iter().any(|q| Self::same_name(&q.name, name)) {
            Some(CatalogKind::Query)
        } else {
            None
        }
    }

    // --- registration landing (the engine's answers, folded onto the rows) ----------

    /// Land a table registration answer on its row.
    pub fn table_registered(&mut self, name: &str, meta: TableMeta) {
        if let Some(r) = self.tables.iter_mut().find(|r| r.def.name == name) {
            r.reg = Reg::Ready(meta);
        }
    }

    /// Land a failed table registration on its row.
    pub fn table_failed(&mut self, name: &str, error: String) {
        if let Some(r) = self.tables.iter_mut().find(|r| r.def.name == name) {
            r.reg = Reg::Failed(error);
        }
    }

    /// Land a view creation answer on its row.
    ///
    /// The engine's `aliases` are raw — inlined view names mixed with table-alias /
    /// CTE noise it can't tell apart from a view inline. Keep only the ones that are
    /// actually views (a view can't reference itself, and every view has a row from
    /// load, so the filter sees them all regardless of registration order).
    pub fn view_registered(&mut self, name: &str, meta: ViewMeta) {
        let view_deps: Vec<String> = meta
            .aliases
            .into_iter()
            .filter(|a| self.views.iter().any(|v| v.def.name == *a && v.def.name != name))
            .collect();
        if let Some(v) = self.views.iter_mut().find(|v| v.def.name == name) {
            v.reg = Reg::Ready(ViewInfo {
                columns: meta.columns,
                deps: meta.tables,
                view_deps,
            });
        }
    }

    /// Land a failed view creation on its row.
    pub fn view_failed(&mut self, name: &str, error: String) {
        if let Some(v) = self.views.iter_mut().find(|v| v.def.name == name) {
            v.reg = Reg::Failed(error);
        }
    }

    // --- def mutations (the caller persists via `save_defs`) ------------------------

    /// Insert-or-replace a view def by name, at its alphabetical slot. The row resets
    /// to `Loading` — a (re)written def is unanswered until the engine speaks.
    #[allow(dead_code)]
    pub fn upsert_view(&mut self, def: ViewDef) {
        self.views.retain(|x| x.def.name != def.name);
        let at = self
            .views
            .partition_point(|x| name_ord(&x.def.name, &def.name).is_lt());
        self.views.insert(at, ViewRow::new(def));
    }

    /// Drop the view named `name`.
    #[allow(dead_code)]
    pub fn remove_view(&mut self, name: &str) {
        self.views.retain(|v| v.def.name != name);
    }

    /// Insert-or-replace a saved query by its stable `id`, keeping the alphabetical
    /// slot of its (possibly new) name.
    #[allow(dead_code)]
    pub fn upsert_saved_query(&mut self, query: SavedQuery) {
        self.saved_queries.retain(|x| x.id != query.id);
        let at = self
            .saved_queries
            .partition_point(|x| name_ord(&x.name, &query.name).is_lt());
        self.saved_queries.insert(at, query);
    }

    /// Drop the saved query with this `id`.
    #[allow(dead_code)]
    pub fn remove_saved_query(&mut self, id: Uuid) {
        self.saved_queries.retain(|q| q.id != id);
    }

    /// Insert-or-replace a table def by name (registration / config save), at its
    /// alphabetical slot. Resets the row to `Loading` like `upsert_view`.
    #[allow(dead_code)]
    pub fn upsert_table(&mut self, def: TableDef) {
        self.tables.retain(|x| x.def.name != def.name);
        let at = self
            .tables
            .partition_point(|x| name_ord(&x.def.name, &def.name).is_lt());
        self.tables.insert(at, TableRow::new(def));
    }

    /// Drop the table named `name`.
    #[allow(dead_code)]
    pub fn remove_table(&mut self, name: &str) {
        self.tables.retain(|t| t.def.name != name);
    }
}
