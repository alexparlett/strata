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
//! landing engine answers matches exactly (round-trips of our own strings). A view's
//! `deps` are the exception on the read side: they come back from the *planner*, off the
//! user's SQL rather than our strings, so [`ProjectState::view_problem`] looks them up
//! case-insensitively too.
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
    /// The base tables it reads (transitive — the planner inlines nested views). Read by
    /// [`ProjectState::view_problem`] (P3-04) and, from the other direction, by
    /// [`ProjectState::dependent_views`] (P3-05); profile invalidation takes the same list
    /// (P3-09).
    pub deps: Vec<String>,
    /// The views it reads (transitive), resolved from the engine's raw aliases. The view
    /// half of the drop warning: `deps` is base tables *by construction*, so it can answer
    /// "which views read this table" but never "which views read this view" (DEV_TASKS D10
    /// records that limit) — this list is what answers it.
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

    // --- validity (P3-04) ----------------------------------------------------------
    //
    // The catalog is *definitions*, not a mirror of DataFusion: a row can exist and not
    // work. So validity is **derived on read**, never stored — a table's straight off the
    // answer the engine already gave, a view's against the live table rows its `deps`
    // name. There is no flag to invalidate, which is what makes it self-heal: put the
    // file back, re-scan, and the triangle is gone on the next render.

    /// A table's problem, if any — the engine refused its def (missing file, bad path).
    ///
    /// Takes a row rather than `&self` because a table's validity is entirely local: it
    /// is the answer already landed on the row. An **unanswered** row is not a broken one,
    /// so `Loading` reports nothing — otherwise a re-scan would flash a triangle over
    /// every table while it retried them.
    pub fn table_problem(row: &TableRow) -> Option<String> {
        row.reg.error().map(str::to_owned)
    }

    /// A view's problem, if any:
    ///
    /// - the **hard** failure the engine reported — the SQL didn't plan (a syntax error,
    ///   or a base table already missing when the view was created); or
    /// - a **missing dependency** — a base table it reads is gone from the catalog, or is
    ///   itself failing to register. [`ViewInfo::deps`] is the *transitive* base-table set
    ///   (the planner inlines nested views at creation), so this reaches through a
    ///   view-of-a-view, and it catches a table dropped **after** the view registered
    ///   cleanly, which raises no event of its own.
    ///
    /// Note what the triangle does *not* claim. Verified against DataFusion 54: dropping a
    /// base table does **not** break the view's live plan — that plan captured each source
    /// by `Arc` at creation and never re-resolves the name, so `SELECT * FROM the_view`
    /// still answers. What is true is that the view will not survive a reload, which is
    /// why it is flagged as *left invalid*. It is also why validity is derived from `deps`
    /// rather than by re-issuing `CREATE OR REPLACE VIEW`: a re-plan catches a directly
    /// missing base table but a view-of-a-view masks it behind the same live `Arc`.
    pub fn view_problem(&self, row: &ViewRow) -> Option<String> {
        let info = match &row.reg {
            Reg::Failed(e) => return Some(e.clone()),
            // Unanswered: not broken — and there are no deps to check yet either.
            Reg::Loading => return None,
            Reg::Ready(info) => info,
        };
        info.deps.iter().find_map(|dep| {
            // `deps` are engine-landed names, but DataFusion folds unquoted identifiers, so
            // match them the way the catalog's own name rule does — `name_in_use` already
            // stops two rows that fold together from coexisting, so this can't widen onto
            // the wrong row, while an exact compare could miss the right one and cry wolf.
            match self
                .tables
                .iter()
                .find(|t| Self::same_name(&t.def.name, dep))
            {
                None => Some(format!("Reads {dep}, which is no longer in the catalog.")),
                Some(t) if matches!(t.reg, Reg::Failed(_)) => {
                    Some(format!("Reads {dep}, which failed to load."))
                }
                // Ready, or still loading — a dep mid-re-scan is not a problem.
                Some(_) => None,
            }
        })
    }

    // --- dependents (P3-05) --------------------------------------------------------

    /// The views a drop of `name` would leave **invalid**, alphabetically (rows are kept
    /// sorted) — the other direction of the same deps [`view_problem`](Self::view_problem)
    /// reads (D10). The drop confirm's consequence line and its name chips.
    ///
    /// Which list answers depends on what is being dropped, and the two are not
    /// interchangeable. [`ViewInfo::deps`] is the *base tables* a view reads — transitive,
    /// because the planner inlines nested views at creation — so it reaches a view-of-a-view
    /// over the dropped table. [`ViewInfo::view_deps`] is the *views* it reads, which is the
    /// only thing that can answer the view case at all. A saved query is not a SQL object:
    /// nothing can read it, so it has no dependents.
    ///
    /// Names fold case for the same reason `view_problem`'s lookup does — deps come back
    /// from the planner, the dropped name comes from a def.
    ///
    /// **Left invalid, not broken.** Verified against DataFusion 54: a dependent view's live
    /// plan captured its sources by `Arc` at creation and never re-resolves their names, so
    /// it keeps answering after the drop and only fails on the next reload, when its SQL
    /// re-plans against something that is gone. The confirm says exactly what the row's
    /// triangle will say (P3-04) — *left invalid* — not "will stop working".
    ///
    /// A view with no landed answer is **not** listed: there is no dependency information to
    /// read off it, and a row that never registered is already flagged on its own account —
    /// this drop is not what would invalidate it.
    pub fn dependent_views(&self, kind: CatalogKind, name: &str) -> Vec<String> {
        self.views
            .iter()
            // Tables and views share one namespace, but a view can't read itself — and a
            // view being dropped is not left invalid by its own drop.
            .filter(|v| !Self::same_name(&v.def.name, name))
            .filter(|v| {
                let Some(info) = v.reg.ready() else {
                    return false;
                };
                match kind {
                    CatalogKind::Table => &info.deps,
                    CatalogKind::View => &info.view_deps,
                    CatalogKind::Query => return false,
                }
                .iter()
                .any(|dep| Self::same_name(dep, name))
            })
            .map(|v| v.def.name.clone())
            .collect()
    }

    // --- re-scan (P3-03) -----------------------------------------------------------

    /// Reset every table row to `Loading` — the start of a catalog re-scan. The defs are
    /// untouched; only what the engine last said is dropped, because that is the truth: mid-re-scan
    /// the store has no verdict on this row, and keeping the old error here would make `Failed`
    /// mean two different things.
    ///
    /// Display continuity is the *sidebar's* problem, not the store's, and it solves it without
    /// lying: the row's status slot goes on showing the last verdict it had until a new one lands
    /// (see `views/sidebar/catalog/entry.rs`), so ↻ on a broken row doesn't blink its triangle off
    /// and back on. Un-flagging it here would have been the blink; keeping the error here would
    /// have been a claim we can't make.
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
    pub fn upsert_view(&mut self, def: ViewDef) {
        self.views.retain(|x| x.def.name != def.name);
        let at = self
            .views
            .partition_point(|x| name_ord(&x.def.name, &def.name).is_lt());
        self.views.insert(at, ViewRow::new(def));
    }

    /// Drop the view named `name`.
    pub fn remove_view(&mut self, name: &str) {
        self.views.retain(|v| v.def.name != name);
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
    pub fn remove_table(&mut self, name: &str) {
        self.tables.retain(|t| t.def.name != name);
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

    // --- validity (P3-04) --------------------------------------------------------------

    fn view_meta(deps: &[&str]) -> ViewMeta {
        ViewMeta {
            columns: Vec::new(),
            tables: deps.iter().map(|d| (*d).to_string()).collect(),
            aliases: Vec::new(),
        }
    }

    /// A table says what the engine said, and *only* when the engine has refused it. The
    /// unanswered case is the one worth pinning: a re-scan resets every row to `Loading`, and
    /// treating that as a problem would put a triangle on the whole catalog while it retried.
    #[test]
    fn a_table_is_invalid_only_once_registration_has_actually_failed() {
        let p = settled();

        assert_eq!(ProjectState::table_problem(&p.tables[0]), None, "Ready");
        assert_eq!(
            ProjectState::table_problem(&p.tables[1]).as_deref(),
            Some("no such file"),
            "Failed carries the engine's own reason"
        );

        let mut p = p;
        p.reload_tables();
        assert!(
            p.tables
                .iter()
                .all(|t| ProjectState::table_problem(t).is_none()),
            "an unanswered row is not a broken one"
        );
    }

    /// A view's hard failure — the SQL never planned — is reported verbatim, and short-circuits
    /// the dependency walk (there are no deps to walk: nothing landed).
    #[test]
    fn a_view_reports_the_failure_the_engine_gave_it() {
        let mut p = settled();
        p.view_failed("orders_daily", "Schema error: No field named x".into());

        assert_eq!(
            p.view_problem(&p.views[0]).as_deref(),
            Some("Schema error: No field named x")
        );
    }

    /// The derived half: a view that registered *cleanly* turns invalid when a base table it
    /// reads leaves the catalog. Dropping a table raises no event of the view's own — this is
    /// the only thing that notices.
    #[test]
    fn a_view_over_a_dropped_table_is_invalid() {
        let mut p = settled();
        assert_eq!(p.view_problem(&p.views[0]), None, "healthy to begin with");

        p.remove_table("orders");

        assert_eq!(
            p.view_problem(&p.views[0]).as_deref(),
            Some("Reads orders, which is no longer in the catalog.")
        );
    }

    /// A base table that is *present but broken* is just as fatal to the view, and says so in
    /// its own words — "failed to load" points at the table, not at the view's SQL.
    #[test]
    fn a_view_over_a_failed_table_is_invalid() {
        let mut p = settled();
        p.table_failed("orders", "No such file or directory (os error 2)".into());

        assert_eq!(
            p.view_problem(&p.views[0]).as_deref(),
            Some("Reads orders, which failed to load.")
        );
    }

    /// A dep that is merely *unanswered* is not a problem. Mid-re-scan every table is
    /// `Loading` for a moment, and flagging then would strobe the whole VIEWS section.
    #[test]
    fn a_view_whose_base_table_is_still_loading_is_not_flagged() {
        let mut p = settled();
        p.reload_tables();

        assert_eq!(p.view_problem(&p.views[0]), None);
    }

    /// Nothing is stored, so nothing has to be invalidated: the answer follows the catalog.
    /// Re-register the table and the view is simply valid again on the next read.
    #[test]
    fn a_views_validity_heals_when_its_base_table_comes_back() {
        let mut p = settled();
        p.table_failed("orders", "gone".into());
        assert!(p.view_problem(&p.views[0]).is_some());

        p.table_registered(
            "orders",
            TableMeta {
                columns: Vec::new(),
                rows: Some(10),
            },
        );

        assert_eq!(p.view_problem(&p.views[0]), None);
    }

    /// Deps are transitive base tables (the planner inlines a view-of-a-view at creation), so
    /// a missing table deep under a nested view still reaches the outer view's row — and the
    /// message names the *table*, which is the thing the user has to fix.
    #[test]
    fn a_nested_views_missing_base_table_still_reaches_the_outer_view() {
        let mut p = settled();
        p.upsert_view(ViewDef {
            name: "orders_weekly".into(),
            sql: "SELECT * FROM orders_daily".into(),
        });
        // What the planner lands for a view over a view: the *base* tables, inlined.
        p.view_registered("orders_weekly", view_meta(&["orders"]));

        p.remove_table("orders");

        let outer = p
            .views
            .iter()
            .find(|v| v.def.name == "orders_weekly")
            .expect("the nested view is in the catalog");
        assert_eq!(
            p.view_problem(outer).as_deref(),
            Some("Reads orders, which is no longer in the catalog.")
        );
    }

    // --- dependents (P3-05) ------------------------------------------------------------

    /// A view meta that reads views as well as tables — what the planner lands for a view over
    /// a view (the base tables inlined, plus the view names among its raw aliases).
    fn view_meta_over(tables: &[&str], views: &[&str]) -> ViewMeta {
        ViewMeta {
            columns: Vec::new(),
            tables: tables.iter().map(|d| (*d).to_string()).collect(),
            aliases: views.iter().map(|d| (*d).to_string()).collect(),
        }
    }

    /// The headline: dropping a table names the views that read it, and only those. This is what
    /// the confirm dialog states before the drop turns them all into triangles.
    #[test]
    fn dropping_a_table_names_the_views_that_read_it() {
        let mut p = settled();
        p.upsert_view(ViewDef {
            name: "user_signups".into(),
            sql: "SELECT * FROM users".into(),
        });
        p.view_registered("user_signups", view_meta(&["users"]));

        assert_eq!(
            p.dependent_views(CatalogKind::Table, "orders"),
            vec!["orders_daily".to_string()]
        );
        assert_eq!(
            p.dependent_views(CatalogKind::Table, "users"),
            vec!["user_signups".to_string()]
        );
    }

    /// A table nothing reads has no consequence line at all — the dialog must not manufacture
    /// one, so "nothing depends on this" has to come back empty rather than as a zero.
    #[test]
    fn a_table_no_view_reads_has_no_dependents() {
        let p = settled();

        assert!(p.dependent_views(CatalogKind::Table, "users").is_empty());
    }

    /// Deps are transitive base tables, so a view *of a view* over the dropped table is named
    /// too — it is just as invalid, and it is the case a hand-rolled "which views mention this
    /// name" scan would miss.
    #[test]
    fn a_table_drop_reaches_through_a_nested_view() {
        let mut p = settled();
        p.upsert_view(ViewDef {
            name: "orders_weekly".into(),
            sql: "SELECT * FROM orders_daily".into(),
        });
        // The planner inlines the inner view: base tables in `tables`, the view in `aliases`.
        p.view_registered(
            "orders_weekly",
            view_meta_over(&["orders"], &["orders_daily"]),
        );

        assert_eq!(
            p.dependent_views(CatalogKind::Table, "orders"),
            vec!["orders_daily".to_string(), "orders_weekly".to_string()],
            "both the direct reader and the view over it"
        );
    }

    /// Dropping a **view** is the other lookup — `view_deps`, not `deps`. Asserting both
    /// directions off one store is the point: `orders_daily` reads `orders` and is read by
    /// `orders_weekly`, and neither question may be answered with the other's list.
    #[test]
    fn dropping_a_view_names_the_views_that_read_it_not_its_base_tables_readers() {
        let mut p = settled();
        p.upsert_view(ViewDef {
            name: "orders_weekly".into(),
            sql: "SELECT * FROM orders_daily".into(),
        });
        p.view_registered(
            "orders_weekly",
            view_meta_over(&["orders"], &["orders_daily"]),
        );

        assert_eq!(
            p.dependent_views(CatalogKind::View, "orders_daily"),
            vec!["orders_weekly".to_string()]
        );
        assert!(
            p.dependent_views(CatalogKind::View, "orders_weekly")
                .is_empty(),
            "nothing reads the outer view"
        );
    }

    /// The row being dropped is never listed as its own dependent — a confirm warning that a
    /// view will invalidate itself is noise. Held at both layers, which is why the meta landed
    /// here names the view itself: `view_registered` drops a self-alias, and the lookup would
    /// not list it even if one got through.
    #[test]
    fn a_view_is_never_listed_as_its_own_dependent() {
        let mut p = settled();
        p.view_registered("orders_daily", view_meta_over(&[], &["orders_daily"]));

        assert!(p
            .dependent_views(CatalogKind::View, "orders_daily")
            .is_empty());
    }

    /// Nothing can read a saved query — it is a stored string, not a SQL object — so a delete
    /// has no dependents to warn about however the catalog is shaped.
    #[test]
    fn a_saved_query_has_no_dependents() {
        let p = settled();

        assert!(p
            .dependent_views(CatalogKind::Query, "orders by region")
            .is_empty());
        assert!(p.dependent_views(CatalogKind::Query, "orders").is_empty());
    }

    /// A view with no landed answer carries no dependency information, so it can't be counted.
    /// Mid-re-scan that is the whole catalog — a confirm that claimed every view would be left
    /// invalid, or none, purely on scan timing would be worthless.
    #[test]
    fn an_unanswered_view_is_not_counted_as_a_dependent() {
        let mut p = settled();
        assert_eq!(p.dependent_views(CatalogKind::Table, "orders").len(), 1);

        p.reload_views();

        assert!(p.dependent_views(CatalogKind::Table, "orders").is_empty());
    }

    /// The same case fold as everywhere else: the dep names come from the planner, the dropped
    /// name from the def the user is right-clicking.
    #[test]
    fn dependents_fold_case_like_the_rest_of_the_catalog() {
        let mut p = settled();
        p.view_registered("orders_daily", view_meta(&["ORDERS"]));

        assert_eq!(
            p.dependent_views(CatalogKind::Table, "orders"),
            vec!["orders_daily".to_string()]
        );
    }

    /// Dep names come back from the planner while def names come from the user, and DataFusion
    /// folds unquoted identifiers — so the lookup folds case too. Matching exactly would put a
    /// "no longer in the catalog" triangle on a view whose table is sitting right there.
    #[test]
    fn dependency_lookup_folds_case_like_the_rest_of_the_catalog() {
        let defs = ProjectDefs {
            name: "test".into(),
            tables: vec![table_def("Orders")],
            views: vec![ViewDef {
                name: "orders_daily".into(),
                sql: "SELECT 1".into(),
            }],
            saved_queries: Vec::new(),
        };
        let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-validity-test"));
        p.table_registered(
            "Orders",
            TableMeta {
                columns: Vec::new(),
                rows: None,
            },
        );
        p.view_registered("orders_daily", view_meta(&["orders"]));

        assert_eq!(p.view_problem(&p.views[0]), None);
    }
}
