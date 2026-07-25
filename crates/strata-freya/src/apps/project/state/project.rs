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
//! ([`ProjectState::same_name`]) — DataFusion folds unquoted identifiers — and that rule
//! governs **every** def-identity decision: `name_in_use`, the upserts and the removes.
//! Only **landing engine answers** matches exactly (round-trips of our own strings).
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
    /// Project identity: name / root path. Subscribed by the header's project switcher (a
    /// rename / re-open re-labels the trigger); the window title joins it with P4-13.
    Meta,
    Tables,
    Views,
    /// Notified by save-as-query (⌘S on a scratch / saved-query tab); subscribed by
    /// the sidebar QUERIES section (Phase 3).
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
    /// The autocomplete symbol catalog (P2-04); the inspector reads it too (Phase 3).
    pub columns: Vec<ColumnInfo>,
    /// The base tables it reads (transitive — the planner inlines nested views).
    /// Feature reservoir: the table-drop warning + profile invalidation (Phase 3).
    #[allow(dead_code)]
    pub deps: Vec<String>,
    /// The views it reads (transitive), resolved from the engine's raw aliases.
    #[allow(dead_code)]
    // Feature reservoir: the table-drop warning + reload ordering (Phase 3).
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
/// shuffle. Always built **full, from load or scaffold** ([`from_defs`](Self::from_defs))
/// — there is no `Default`: a project can't exist without a folder on disk, so a rootless
/// in-memory project is not a representable state.
pub struct ProjectState {
    pub name: String,
    /// The project folder — the parent of its `.strata/` dir, and the base relative
    /// source paths resolve against. Always set: opening a project that can't be
    /// canonicalized is an unrecoverable error, not a rootless fallback.
    pub root: PathBuf,
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
            root,
            tables: defs.tables.into_iter().map(TableRow::new).collect(),
            views: defs.views.into_iter().map(ViewRow::new).collect(),
            saved_queries: defs.saved_queries,
        }
    }

    /// The durable defs — a pure projection of the rows (what `.strata/project.json`
    /// stores; registration state never travels).
    pub fn defs(&self) -> ProjectDefs {
        ProjectDefs {
            name: self.name.clone(),
            tables: self.tables.iter().map(|r| r.def.clone()).collect(),
            views: self.views.iter().map(|r| r.def.clone()).collect(),
            saved_queries: self.saved_queries.clone(),
        }
    }

    /// Persist the defs to `.strata/project.json`. Call at def-mutation points
    /// (view/saved-query create · drop · register/deregister).
    pub fn save_defs(&self) -> Result<(), String> {
        project_io::save_defs(&self.root, &self.defs())
    }

    // --- identity ------------------------------------------------------------------

    /// The one name-equality rule for user-entered catalog names: case-insensitive
    /// (DataFusion folds unquoted identifiers).
    pub fn same_name(a: &str, b: &str) -> bool {
        a.eq_ignore_ascii_case(b)
    }

    /// Which section, if any, already owns `name` — tables and views share one SQL
    /// namespace, so a new name must be free in both; saved-query labels only clash
    /// with themselves. (Also the config-modal name validation, P4-11.)
    pub fn name_in_use(&self, name: &str) -> Option<CatalogKind> {
        if self
            .tables
            .iter()
            .any(|r| Self::same_name(&r.def.name, name))
        {
            Some(CatalogKind::Table)
        } else if self
            .views
            .iter()
            .any(|r| Self::same_name(&r.def.name, name))
        {
            Some(CatalogKind::View)
        } else if self
            .saved_queries
            .iter()
            .any(|q| Self::same_name(&q.name, name))
        {
            Some(CatalogKind::Query)
        } else {
            None
        }
    }

    // --- registration landing (the engine's answers, folded onto the rows) ----------

    /// Land a table registration answer on its row.
    ///
    /// The one funnel every table answer arrives through — project open, a catalog re-scan
    /// (P3-03), and a table-config save (P4-11) all land here. **When P3-09 adds the profile
    /// cache, this is where it is dropped**: a landing answer means the files may have moved
    /// under the row, which is exactly when a cached full-scan becomes a lie — and with it the
    /// profile of every view whose [`ViewInfo::deps`] name this table, the half of "cached until
    /// it changes" a view can't get from its own row (D10).
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
            .filter(|a| {
                self.views
                    .iter()
                    .any(|v| v.def.name == *a && v.def.name != name)
            })
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

    // --- re-scan (P3-03) -----------------------------------------------------------

    /// Reset every table row to `Loading` — the start of a catalog re-scan. The defs are
    /// untouched; only what the engine last said is dropped, so a row that was `Failed` shows
    /// as unanswered while it is retried rather than displaying a stale error the re-scan may
    /// be about to clear.
    pub fn reload_tables(&mut self) {
        for t in &mut self.tables {
            t.reg = Reg::Loading;
        }
    }

    /// Reset every view row to `Loading` — the view half of a re-scan.
    ///
    /// Views are re-created after the tables land, and the reason is subtle enough to be worth
    /// stating: a view captures each base table **by `Arc`** in the plan it stores at `CREATE
    /// VIEW` time, and the table's *name* is never re-resolved at query time (verified against
    /// DataFusion 54 for D10/D11). So re-registering `orders` does not break a view over it —
    /// worse, the view keeps scanning the *old* provider with the *old* inferred schema. Only
    /// re-issuing `CREATE OR REPLACE VIEW` re-plans it against what the re-scan just found.
    ///
    /// That is a *refresh*, not a validity check: a view-of-a-view masks a missing table behind
    /// the still-live inner `Arc`, which is why P3-04 derives validity from `deps` instead.
    pub fn reload_views(&mut self) {
        for v in &mut self.views {
            v.reg = Reg::Loading;
        }
    }

    // --- def mutations (the caller persists via `save_defs`) ------------------------

    /// Insert-or-replace a view def by name, at its alphabetical slot. The row resets
    /// to `Loading` — a (re)written def is unanswered until the engine speaks.
    ///
    /// Replacement uses [`same_name`](Self::same_name), like every other user-entered-name
    /// comparison: the engine folds unquoted identifiers, so saving `Orders` over an
    /// existing `orders` replaces *one* view there — an exact-match dedup would leave two
    /// catalog rows over it, one of them a permanent lie.
    pub fn upsert_view(&mut self, def: ViewDef) {
        self.views
            .retain(|x| !Self::same_name(&x.def.name, &def.name));
        let at = self
            .views
            .partition_point(|x| name_ord(&x.def.name, &def.name).is_lt());
        self.views.insert(at, ViewRow::new(def));
    }

    /// Drop the view named `name` — matched like [`upsert_view`](Self::upsert_view), so a
    /// drop names the same row a save would have replaced.
    #[allow(dead_code)]
    pub fn remove_view(&mut self, name: &str) {
        self.views.retain(|v| !Self::same_name(&v.def.name, name));
    }

    /// Insert-or-replace a saved query by its stable `id`, keeping the alphabetical
    /// slot of its (possibly new) name.
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
    /// alphabetical slot. Resets the row to `Loading`, and dedups by
    /// [`same_name`](Self::same_name), like `upsert_view`.
    #[allow(dead_code)]
    pub fn upsert_table(&mut self, def: TableDef) {
        self.tables
            .retain(|x| !Self::same_name(&x.def.name, &def.name));
        let at = self
            .tables
            .partition_point(|x| name_ord(&x.def.name, &def.name).is_lt());
        self.tables.insert(at, TableRow::new(def));
    }

    /// Drop the table named `name` — matched like [`upsert_table`](Self::upsert_table).
    #[allow(dead_code)]
    pub fn remove_table(&mut self, name: &str) {
        self.tables.retain(|t| !Self::same_name(&t.def.name, name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_def(name: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: "parquet".into(),
            sources: vec![format!("{name}.parquet")],
            partition_cols: vec![("year".into(), "Int32".into())],
        }
    }

    /// A store with one settled table, one refused table, and one settled view — the three
    /// registration states a re-scan has to reset.
    fn settled() -> ProjectState {
        let defs = ProjectDefs {
            name: "test".into(),
            tables: vec![table_def("orders"), table_def("users")],
            views: vec![ViewDef {
                name: "orders_daily".into(),
                sql: "SELECT 1".into(),
            }],
            saved_queries: vec![SavedQuery {
                id: Uuid::from_u128(1),
                name: "orders by region".into(),
                sql: "SELECT 2".into(),
                meta: "—".into(),
            }],
        };
        let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-reload-test"));
        p.table_registered(
            "orders",
            TableMeta {
                columns: Vec::new(),
                rows: Some(10),
            },
        );
        p.table_failed("users", "no such file".into());
        p.view_registered(
            "orders_daily",
            ViewMeta {
                columns: Vec::new(),
                tables: vec!["orders".into()],
                aliases: Vec::new(),
            },
        );
        p
    }

    /// A re-scan drops what the engine last said and nothing else: every row goes back to
    /// `Loading` — including the one that **failed**, which is the case ↻ most needs to serve
    /// (the user fixed the path and pressed refresh; keeping the stale error would read as if
    /// nothing was retried).
    #[test]
    fn reloading_resets_every_registration_state_including_failures() {
        let mut p = settled();
        assert!(p.tables[0].reg.ready().is_some());
        assert_eq!(p.tables[1].reg.error(), Some("no such file"));
        assert!(p.views[0].reg.ready().is_some());

        p.reload_tables();
        p.reload_views();

        assert!(p.tables.iter().all(|t| matches!(t.reg, Reg::Loading)));
        assert!(p.views.iter().all(|v| matches!(v.reg, Reg::Loading)));
    }

    /// The defs are the *project*; a re-scan only re-asks the engine about them. Nothing
    /// persisted may move — otherwise ↻ would be a mutation of the project file's contents.
    #[test]
    fn reloading_leaves_the_defs_untouched() {
        let mut p = settled();
        let before = p.defs();

        p.reload_tables();
        p.reload_views();

        let after = p.defs();
        assert_eq!(before.name, after.name);
        assert_eq!(
            before.tables.iter().map(|t| &t.name).collect::<Vec<_>>(),
            after.tables.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        assert_eq!(before.tables[0].sources, after.tables[0].sources);
        assert_eq!(
            before.tables[0].partition_cols,
            after.tables[0].partition_cols
        );
        assert_eq!(before.views[0].sql, after.views[0].sql);
        // Saved queries aren't engine-registered at all, so a re-scan can't reach them.
        assert_eq!(before.saved_queries.len(), after.saved_queries.len());
        assert_eq!(before.saved_queries[0].id, after.saved_queries[0].id);
    }

    /// Def identity is `same_name`, not `==`. The engine folds unquoted identifiers, so
    /// saving a view as `Orders_Daily` when `orders_daily` exists replaces the *one* view
    /// the engine will replace — an exact-match dedup left a second catalog row over it,
    /// permanently describing a def that no longer exists.
    #[test]
    fn def_mutations_match_names_the_way_the_engine_does() {
        let mut p = settled();

        p.upsert_view(ViewDef {
            name: "Orders_Daily".into(),
            sql: "SELECT 2".into(),
        });
        assert_eq!(p.views.len(), 1, "one row per folded name");
        assert_eq!(p.views[0].def.name, "Orders_Daily", "the new spelling wins");
        assert_eq!(p.views[0].def.sql, "SELECT 2");

        p.upsert_table(table_def("ORDERS"));
        let names: Vec<&str> = p.tables.iter().map(|t| t.def.name.as_str()).collect();
        assert_eq!(
            names,
            ["ORDERS", "users"],
            "replaced in place, still sorted"
        );

        // And a drop names the same row a save would have replaced.
        p.remove_view("orders_daily");
        p.remove_table("orders");
        assert!(p.views.is_empty());
        assert_eq!(p.tables.len(), 1);
    }

    /// A landing answer replaces `Loading` in place — the second half of the round trip, so
    /// the reset isn't a one-way door into a permanently unanswered catalog.
    #[test]
    fn a_reloaded_row_lands_its_new_answer() {
        let mut p = settled();
        p.reload_tables();

        p.table_registered(
            "users",
            TableMeta {
                columns: Vec::new(),
                rows: Some(3),
            },
        );

        assert_eq!(p.tables[1].def.name, "users");
        assert_eq!(p.tables[1].reg.ready().and_then(|m| m.rows), Some(3));
        // Its neighbour is still awaiting its own answer — rows land one at a time.
        assert!(matches!(p.tables[0].reg, Reg::Loading));
    }
}
