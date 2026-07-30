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
//! Only **landing engine answers** matches exactly (round-trips of our own strings). A
//! view's `deps` are the exception on the read side: they come back from the *planner*,
//! off the user's SQL rather than our strings, so [`ProjectState::view_problem`] looks
//! them up case-insensitively too.
//!
//! Rows also carry the **profile request** (P3-09) — which scan the user asked for, never its
//! numbers: those live in the freya-query cache under that request's id. So the store still
//! holds no query results, and invalidating a profile is dropping the request.
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

use crate::apps::project::query::ScanId;

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
    /// The profile scan the user has asked for on this table, if any (P3-09).
    ///
    /// A **request**, not a result — the scan's facts live in the freya-query cache under this
    /// very id ([`use_profile`](crate::apps::project::query::use_profile)), like a Run's rows
    /// under its `QuerySpec`. So the store still holds no query results, and dropping this
    /// field *is* invalidating the profile.
    pub profile: Option<ScanId>,
}

impl TableRow {
    fn new(def: TableDef) -> Self {
        Self {
            def,
            reg: Reg::Loading,
            profile: None,
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
    /// The profile scan asked for on this view — see [`TableRow::profile`]. A view is where a
    /// scan buys the most: it has no files under it, so it reports nothing for free.
    pub profile: Option<ScanId>,
}

impl ViewRow {
    fn new(def: ViewDef) -> Self {
        Self {
            def,
            reg: Reg::Loading,
            profile: None,
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

/// One def the engine refused — a row of the Problems drawer's **Project** tab.
///
/// Kept as a projection rather than a stored list, because a registration failure already *is*
/// live state: `Reg::Failed` on the row. Re-deriving it is what makes the drawer retract the row
/// the moment a re-scan fixes the def, with nothing to invalidate — the same property the SQL
/// diagnostics have, reached a different way.
#[derive(Clone, PartialEq, Debug)]
pub struct RegistrationFault {
    /// Which section it sits in — the row says "table" or "view" rather than making the user
    /// infer it from the name.
    pub kind: CatalogKind,
    pub name: String,
    /// What the engine said. P3-07's wording, the same text the catalog row's triangle carries.
    pub why: String,
}

impl ProjectState {
    /// Every def the engine refused, tables before views (the order they register in, so a view
    /// broken *by* a broken table reads below its cause).
    ///
    /// Saved queries can't appear: they are stored strings that are never registered, so there is
    /// no engine answer for them to have failed.
    pub fn registration_faults(&self) -> Vec<RegistrationFault> {
        let tables = self.tables.iter().filter_map(|r| {
            r.reg.error().map(|why| RegistrationFault {
                kind: CatalogKind::Table,
                name: r.def.name.clone(),
                why: why.to_string(),
            })
        });
        let views = self.views.iter().filter_map(|r| {
            r.reg.error().map(|why| RegistrationFault {
                kind: CatalogKind::View,
                name: r.def.name.clone(),
                why: why.to_string(),
            })
        });
        tables.chain(views).collect()
    }

    /// How many defs the engine refused — [`registration_faults`](Self::registration_faults)'s
    /// length without building it.
    ///
    /// Separate because the count is read on **every** render of the rail badge and the drawer's
    /// scope strip, where the list itself is built only when the Project scope is actually on
    /// screen. Going through the list would clone two `String`s per failed def just to drop them
    /// again a line later.
    pub fn registration_fault_count(&self) -> usize {
        self.tables
            .iter()
            .filter(|r| r.reg.error().is_some())
            .count()
            + self
                .views
                .iter()
                .filter(|r| r.reg.error().is_some())
                .count()
    }

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
    /// (P3-03), and a table-config save (P4-11) all land here — which is why it is also where
    /// the **profile is invalidated** (P3-09): a landing answer means the files may have moved
    /// under the row, and that is exactly when a cached full scan becomes a lie.
    pub fn table_registered(&mut self, name: &str, meta: TableMeta) {
        if let Some(r) = self.tables.iter_mut().find(|r| r.def.name == name) {
            r.reg = Reg::Ready(meta);
            r.profile = None;
        }
        self.invalidate_readers(name);
    }

    /// Land a failed table registration on its row.
    pub fn table_failed(&mut self, name: &str, error: String) {
        if let Some(r) = self.tables.iter_mut().find(|r| r.def.name == name) {
            r.reg = Reg::Failed(error);
            r.profile = None;
        }
        self.invalidate_readers(name);
    }

    /// Drop the profile request of every view that **reads** `table` — the half of "cached
    /// until it changes" a view cannot get from its own row (D10): a view's numbers came from
    /// the tables underneath it, and re-registering one of those makes them stale even though
    /// nothing about the view's own def moved.
    ///
    /// Usually a no-op, and deliberately so. Every path that re-registers a table also
    /// re-creates the views over it ([`views_to_refresh`](Self::views_to_refresh)) — their rows
    /// are already `Loading`, so `dependent_views` sees none of them, and their requests are
    /// dropped by [`view_registered`](Self::view_registered) moments later *on the views
    /// channel*, where the inspector is listening. This is here for the landing path that does
    /// **not** re-create them, so a stale profile can't outlive its data by omission.
    ///
    /// Which is why it leads with the cheap question. This runs once per table on **every**
    /// project open and every re-scan, while `dependent_views` is an O(views × deps) walk that
    /// allocates — and at project open the answer cannot matter, because nothing has had the
    /// chance to ask for a scan yet.
    fn invalidate_readers(&mut self, table: &str) {
        if self.views.iter().all(|v| v.profile.is_none()) {
            return;
        }
        let readers = self.dependent_views(CatalogKind::Table, table);
        for v in &mut self.views {
            if readers.iter().any(|r| r == &v.def.name) {
                v.profile = None;
            }
        }
    }

    /// Land a view creation answer on its row.
    ///
    /// The engine's `aliases` are raw — inlined view names mixed with table-alias /
    /// CTE noise it can't tell apart from a view inline. Keep only the ones that are
    /// actually views (a view can't reference itself, and every view has a row from
    /// load, so the filter sees them all regardless of registration order).
    ///
    /// Matched with [`same_name`](Self::same_name), not `==`: aliases come back from the
    /// **planner**, which folds unquoted identifiers to lower case, while def names carry
    /// whatever the user typed. An exact compare drops every alias of a view named with any
    /// upper case at all, leaving `view_deps` empty — and since P3-05 an empty list is a
    /// *claim*, rendered as "nothing reads this view" right before a destructive drop.
    /// Folding here is what makes `dependent_views`' own fold reachable.
    pub fn view_registered(&mut self, name: &str, meta: ViewMeta) {
        let view_deps: Vec<String> = meta
            .aliases
            .into_iter()
            .filter(|a| {
                self.views
                    .iter()
                    .any(|v| Self::same_name(&v.def.name, a) && !Self::same_name(&v.def.name, name))
            })
            .collect();
        if let Some(v) = self.views.iter_mut().find(|v| v.def.name == name) {
            v.reg = Reg::Ready(ViewInfo {
                columns: meta.columns,
                deps: meta.tables,
                view_deps,
            });
            // Re-created means re-planned, against whatever the tables underneath it now hold —
            // so a scan of the old plan describes nothing that is still true.
            //
            // **Its own row, and deliberately no cascade to the views that read it** — unlike
            // [`table_registered`](Self::table_registered), which has [`invalidate_readers`]. The
            // asymmetry is not an oversight. `CREATE OR REPLACE VIEW` inlines the body of every
            // view it reads *at that moment* (see [`refresh_order`](Self::refresh_order)), so a
            // view B over this view holds an inlined copy of the **old** body and goes on
            // answering with it — B's scanned numbers still describe exactly what B returns, and
            // dropping them would put the scan card over a view whose data has not moved.
            //
            // The invariant that makes this self-maintaining: a profile dies when its view's plan
            // is *re-created*, and this method is that event. So if a later task does start
            // re-creating dependent views on a single-view edit, their profiles are already
            // handled — by their own landing answer, on their own channel.
            v.profile = None;
        }
    }

    /// Land a failed view creation on its row.
    pub fn view_failed(&mut self, name: &str, error: String) {
        if let Some(v) = self.views.iter_mut().find(|v| v.def.name == name) {
            v.reg = Reg::Failed(error);
            v.profile = None;
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

    // --- profile requests (P3-09) --------------------------------------------------
    //
    // A scan is asked for by the user (the inspector's card, a row's menu) and answered by the
    // freya-query cache under the request's own id. So the store's whole part in profiling is
    // this one field per row: whether a scan has been asked for, and which one. There is no
    // result here, nothing to keep in step, and invalidation is a `None`.

    /// The scan asked for on `name`, if any — what the inspector subscribes to and what the
    /// sidebar row spins about. `None` is the un-profiled state (the zone's scan card).
    pub fn profile_scan(&self, kind: CatalogKind, name: &str) -> Option<ScanId> {
        match kind {
            CatalogKind::View => self
                .views
                .iter()
                .find(|v| Self::same_name(&v.def.name, name))
                .and_then(|v| v.profile),
            // A saved query is a stored string — nothing to scan, so nothing to find.
            CatalogKind::Query => None,
            CatalogKind::Table => self
                .tables
                .iter()
                .find(|t| Self::same_name(&t.def.name, name))
                .and_then(|t| t.profile),
        }
    }

    /// Ask for a scan of `name`, returning the request's id (`None` when there is no such row).
    ///
    /// Always a **fresh** id, so this is both "profile it" and "re-scan it": the id is the cache
    /// key, so a new one is a new execution rather than a read of the numbers it is replacing. The
    /// engine supersedes the scan in flight, if there is one.
    pub fn request_profile(&mut self, kind: CatalogKind, name: &str) -> Option<ScanId> {
        let scan = ScanId::new();
        let slot = match kind {
            CatalogKind::View => self
                .views
                .iter_mut()
                .find(|v| Self::same_name(&v.def.name, name))
                .map(|v| &mut v.profile),
            CatalogKind::Query => None,
            CatalogKind::Table => self
                .tables
                .iter_mut()
                .find(|t| Self::same_name(&t.def.name, name))
                .map(|t| &mut t.profile),
        }?;
        *slot = Some(scan);
        Some(scan)
    }

    /// Drop the scan request on `name` — a cancel, or an invalidation. The zone goes back to
    /// offering the scan, which is the honest state: there is no result to show.
    pub fn clear_profile(&mut self, kind: CatalogKind, name: &str) {
        match kind {
            CatalogKind::View => {
                if let Some(v) = self
                    .views
                    .iter_mut()
                    .find(|v| Self::same_name(&v.def.name, name))
                {
                    v.profile = None;
                }
            }
            CatalogKind::Query => {}
            CatalogKind::Table => {
                if let Some(t) = self
                    .tables
                    .iter_mut()
                    .find(|t| Self::same_name(&t.def.name, name))
                {
                    t.profile = None;
                }
            }
        }
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

    /// Reset **one** table row to `Loading` — the start of a row's own Refresh (P3-06). Same
    /// reasoning as [`reload_tables`](Self::reload_tables), scoped to the row the user asked
    /// about; every other row keeps the verdict it already has.
    pub fn reload_table(&mut self, name: &str) {
        if let Some(t) = self.tables.iter_mut().find(|t| t.def.name == name) {
            t.reg = Reg::Loading;
        }
    }

    /// Reset **one** view row to `Loading` — the views a single-table Refresh re-creates
    /// ([`views_to_refresh`](Self::views_to_refresh)).
    pub fn reload_view(&mut self, name: &str) {
        if let Some(v) = self.views.iter_mut().find(|v| v.def.name == name) {
            v.reg = Reg::Loading;
        }
    }

    /// Every view a **Refresh** of the table `name` must re-create, in dependency order.
    ///
    /// Two sets, for two different reasons:
    ///
    /// - the views that **read** it ([`ViewInfo::deps`], so transitively through a
    ///   view-of-a-view). Re-registering a table does not break a view over it — worse, the
    ///   view goes on scanning the *old* provider with the *old* inferred schema, because its
    ///   plan captured that provider by `Arc` at creation and never re-resolves the name
    ///   (verified against DataFusion 54, D10/D11). Only re-issuing `CREATE OR REPLACE VIEW`
    ///   re-plans it against what the refresh just found. This is the same decision P3-03 made
    ///   for the whole-catalog ↻, narrowed to one table.
    /// - every view that is currently **failing**. A failed view has no dependency record at
    ///   all, so nothing can say whether this table was the thing it was missing — and the
    ///   case Refresh most needs to serve is exactly that: the user fixes a path, refreshes the
    ///   row, and the views that couldn't plan over it come back. Re-planning is cheap and
    ///   idempotent, so trying is strictly better than leaving them broken.
    pub fn views_to_refresh(&self, name: &str) -> Vec<String> {
        let mut views = self.dependent_views(CatalogKind::Table, name);
        for v in &self.views {
            if matches!(v.reg, Reg::Failed(_)) && !views.iter().any(|n| n == &v.def.name) {
                views.push(v.def.name.clone());
            }
        }
        self.refresh_order(views)
    }

    /// Order `views` so that a view is re-created **after** every view it reads.
    ///
    /// `CREATE OR REPLACE VIEW` inlines the plan of any view it reads *at that moment*, so
    /// re-creating an outer view before its inner one inlines the stale inner plan — and with
    /// it the very provider the refresh is replacing. Alphabetical order (which is how the defs
    /// are stored) gets this right only by luck.
    ///
    /// Kahn's algorithm over [`ViewInfo::view_deps`], restricted to the set being refreshed:
    /// dependencies *outside* the set are already current, so they can't order anything. A view
    /// with no landed answer carries no dependency information and simply sorts wherever it
    /// falls — at project open that is every view, which is why the scan keeps its fixed-point
    /// retry as well. A cycle is impossible (a view can't read itself, and DataFusion refuses
    /// mutual recursion), but a residue is appended rather than dropped: a surprise can cost
    /// ordering, never a re-create.
    pub fn refresh_order(&self, views: Vec<String>) -> Vec<String> {
        let mut remaining = views;
        let mut ordered = Vec::with_capacity(remaining.len());
        while !remaining.is_empty() {
            let (ready, blocked): (Vec<String>, Vec<String>) =
                remaining.iter().cloned().partition(|name| {
                    let deps = self
                        .views
                        .iter()
                        .find(|v| Self::same_name(&v.def.name, name))
                        .and_then(|v| v.reg.ready())
                        .map(|info| info.view_deps.as_slice())
                        .unwrap_or_default();
                    !deps
                        .iter()
                        .any(|d| remaining.iter().any(|r| Self::same_name(r, d)))
                });
            if ready.is_empty() {
                ordered.extend(blocked);
                break;
            }
            ordered.extend(ready);
            remaining = blocked;
        }
        ordered
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
    /// Hands back the row it took, and where it sat — so a caller whose write then fails can put
    /// it exactly back (P4-15 item 4). Returning it rather than cloning the section keeps `Clone`
    /// off `ViewRow` and its `Reg`, and restores the registration state too: a view that comes
    /// back as `Loading` would spin forever, because nothing is going to answer for it.
    pub fn remove_view(&mut self, name: &str) -> Option<(usize, ViewRow)> {
        let at = self
            .views
            .iter()
            .position(|v| Self::same_name(&v.def.name, name))?;
        Some((at, self.views.remove(at)))
    }

    /// Put a row back where [`remove_view`](Self::remove_view) took it from.
    pub fn restore_view(&mut self, at: usize, row: ViewRow) {
        self.views.insert(at.min(self.views.len()), row);
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
    /// Hands back the query it took and its slot — see [`remove_view`](Self::remove_view).
    pub fn remove_saved_query(&mut self, id: Uuid) -> Option<(usize, SavedQuery)> {
        let at = self.saved_queries.iter().position(|q| q.id == id)?;
        Some((at, self.saved_queries.remove(at)))
    }

    /// Put a saved query back where [`remove_saved_query`](Self::remove_saved_query) took it.
    pub fn restore_saved_query(&mut self, at: usize, query: SavedQuery) {
        self.saved_queries
            .insert(at.min(self.saved_queries.len()), query);
    }

    /// Relabel the saved query `id`, moving it to the alphabetical slot of its new name
    /// (P3-06's row rename). A blank name is refused — the row would have nothing to show.
    ///
    /// **A rename is free here, and only here.** A saved query is addressed by its stable
    /// `id`, so nothing points at the old label: no tab origin to rewrite (unlike a view
    /// rename, which moves the row's SQL identity), and no collision rule to enforce — ⌘S
    /// already mints saved queries under whatever the tab is called, so two rows can wear the
    /// same label today and neither is ambiguous to anything that addresses them.
    pub fn rename_saved_query(&mut self, id: Uuid, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let Some(pos) = self.saved_queries.iter().position(|q| q.id == id) else {
            return;
        };
        let mut query = self.saved_queries.remove(pos);
        query.name = name.to_string();
        self.upsert_saved_query(query);
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
    /// Hands back the row it took and its slot — see [`remove_view`](Self::remove_view).
    pub fn remove_table(&mut self, name: &str) -> Option<(usize, TableRow)> {
        let at = self
            .tables
            .iter()
            .position(|t| Self::same_name(&t.def.name, name))?;
        Some((at, self.tables.remove(at)))
    }

    /// Put a row back where [`remove_table`](Self::remove_table) took it from.
    pub fn restore_table(&mut self, at: usize, row: TableRow) {
        self.tables.insert(at.min(self.tables.len()), row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_model::SourceFormat;

    fn table_def(name: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::Parquet,
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
    /// view will invalidate itself is noise.
    ///
    /// The guard is held at both layers, and this pins the **lookup's**, so the row is written
    /// past `view_registered` (which already strips a self-alias): going through the landing path
    /// would leave `view_deps` empty and the test would pass without the filter existing at all.
    #[test]
    fn a_view_is_never_listed_as_its_own_dependent() {
        let mut p = settled();
        p.views[0].reg = Reg::Ready(ViewInfo {
            columns: Vec::new(),
            deps: Vec::new(),
            view_deps: vec!["orders_daily".into()],
        });
        assert_eq!(
            p.views[0].def.name, "orders_daily",
            "the self-referencing row"
        );

        assert!(p
            .dependent_views(CatalogKind::View, "orders_daily")
            .is_empty());
    }

    /// The view→view direction folds case too, and it has to fold at **landing**: the planner
    /// hands back lower-cased aliases while def names keep the user's capitals, so an exact
    /// filter in `view_registered` would leave `view_deps` empty and the drop confirm would
    /// state that nothing reads a view that something does.
    #[test]
    fn view_dependencies_fold_case_when_the_alias_lands() {
        let defs = ProjectDefs {
            name: "test".into(),
            tables: Vec::new(),
            views: vec![
                ViewDef {
                    name: "Orders_Daily".into(),
                    sql: "SELECT 1".into(),
                },
                ViewDef {
                    name: "orders_weekly".into(),
                    sql: "SELECT * FROM Orders_Daily".into(),
                },
            ],
            saved_queries: Vec::new(),
        };
        let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-viewdeps-fold-test"));
        // What the planner lands: the alias folded to lower case.
        p.view_registered("orders_weekly", view_meta_over(&[], &["orders_daily"]));

        assert_eq!(
            p.dependent_views(CatalogKind::View, "Orders_Daily"),
            vec!["orders_weekly".to_string()]
        );
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

    // --- single-row refresh (P3-06) ----------------------------------------------------

    /// A row's Refresh touches **that row**, and the whole point is that nothing else moves:
    /// its neighbour keeps the verdict it already had, so the pane doesn't read as a full
    /// re-scan when the user asked about one table.
    #[test]
    fn refreshing_one_table_leaves_every_other_rows_verdict_alone() {
        let mut p = settled();

        p.reload_table("orders");

        assert!(
            matches!(p.tables[0].reg, Reg::Loading),
            "`orders` is asked again"
        );
        assert_eq!(
            p.tables[1].reg.error(),
            Some("no such file"),
            "`users` still says what it said"
        );
        assert!(
            p.views[0].reg.ready().is_some(),
            "and the views are untouched"
        );
    }

    /// The headline: refreshing a table re-creates the views that **read** it. A view's plan
    /// captured the old provider by `Arc`, so leaving it alone would leave it scanning the
    /// files the refresh just replaced — silently, with the old schema.
    #[test]
    fn refreshing_a_table_re_creates_the_views_that_read_it() {
        let mut p = settled();
        p.upsert_view(ViewDef {
            name: "user_signups".into(),
            sql: "SELECT * FROM users".into(),
        });
        p.view_registered("user_signups", view_meta(&["users"]));

        assert_eq!(
            p.views_to_refresh("orders"),
            vec!["orders_daily".to_string()],
            "the reader of `orders`, not the reader of `users`"
        );
    }

    /// Nested readers come too — `deps` is the *transitive* base-table set, so a view over a
    /// view over the refreshed table re-plans as well. It is exactly as stale as the direct
    /// reader, and one `CREATE OR REPLACE` on the inner view does not reach it.
    #[test]
    fn a_table_refresh_reaches_through_a_nested_view_in_dependency_order() {
        let mut p = settled();
        p.upsert_view(ViewDef {
            name: "a_orders_weekly".into(),
            sql: "SELECT * FROM orders_daily".into(),
        });
        p.view_registered(
            "a_orders_weekly",
            view_meta_over(&["orders"], &["orders_daily"]),
        );

        // Deliberately named so that *alphabetical* order is the wrong order: the outer view
        // sorts first, and re-creating it first would inline the stale inner plan.
        assert_eq!(
            p.views_to_refresh("orders"),
            vec!["orders_daily".to_string(), "a_orders_weekly".to_string()],
            "the view that is read is re-created before the view that reads it"
        );
    }

    /// A view that is **failing** is retried by any table refresh: it has no dependency record,
    /// so nothing can say whether this table was the thing it was missing — and "I fixed the
    /// path, refresh the row" is the case Refresh exists for.
    #[test]
    fn a_table_refresh_retries_every_failing_view() {
        let mut p = settled();
        p.upsert_view(ViewDef {
            name: "user_signups".into(),
            sql: "SELECT * FROM users".into(),
        });
        p.view_failed("user_signups", "table 'users' not found".into());

        let refreshed = p.views_to_refresh("orders");
        assert!(
            refreshed.contains(&"user_signups".to_string()),
            "a broken view is retried even though it never recorded a dep: {refreshed:?}"
        );
        assert!(
            refreshed.contains(&"orders_daily".to_string()),
            "alongside the actual readers"
        );
    }

    /// The ordering rule on its own, over a three-deep chain: every view is re-created after
    /// the view it reads, whatever order it was asked for in.
    #[test]
    fn refresh_order_puts_a_view_after_everything_it_reads() {
        let mut p = settled();
        for name in ["mid", "outer"] {
            p.upsert_view(ViewDef {
                name: name.into(),
                sql: "SELECT 1".into(),
            });
        }
        // `mid` reads `orders_daily`; `outer` reads both (the planner inlines transitively).
        p.view_registered("mid", view_meta_over(&["orders"], &["orders_daily"]));
        p.view_registered(
            "outer",
            view_meta_over(&["orders"], &["orders_daily", "mid"]),
        );

        let ordered = p.refresh_order(vec!["outer".into(), "mid".into(), "orders_daily".into()]);

        assert_eq!(
            ordered,
            vec![
                "orders_daily".to_string(),
                "mid".to_string(),
                "outer".to_string()
            ]
        );
    }

    /// At project open no view has answered yet, so there is no dependency information to order
    /// by — the pass must still hand back every view (the scan's own fixed-point retry is what
    /// resolves order then). Dropping the unanswered ones here would skip them entirely.
    #[test]
    fn refresh_order_keeps_views_that_have_not_answered_yet() {
        let mut p = settled();
        p.reload_views();

        let names: Vec<String> = p.views.iter().map(|v| v.def.name.clone()).collect();
        assert_eq!(p.refresh_order(names.clone()), names);
    }

    // --- profile requests (P3-09) ------------------------------------------------------

    /// Asking for a scan records the request and nothing else, and a re-scan is a **new**
    /// request: the id is the cache key, so re-using it would read the old numbers back.
    #[test]
    fn a_scan_request_is_a_fresh_id_every_time() {
        let mut p = settled();
        assert_eq!(p.profile_scan(CatalogKind::Table, "orders"), None);

        let first = p
            .request_profile(CatalogKind::Table, "orders")
            .expect("the row is there");
        assert_eq!(p.profile_scan(CatalogKind::Table, "orders"), Some(first));

        let again = p.request_profile(CatalogKind::Table, "orders").unwrap();
        assert_ne!(again, first, "a re-scan is a new execution");
        assert_eq!(p.profile_scan(CatalogKind::Table, "orders"), Some(again));

        p.clear_profile(CatalogKind::Table, "orders");
        assert_eq!(
            p.profile_scan(CatalogKind::Table, "orders"),
            None,
            "cancelling puts the row back to offering the scan"
        );
        // Names fold like every other def-identity decision, and a name with no row is simply
        // not a request — never a panic and never a row invented for it.
        assert!(p.request_profile(CatalogKind::Table, "ORDERS").is_some());
        assert!(p.request_profile(CatalogKind::Table, "nope").is_none());
        // A saved query is a stored string: there is nothing to scan.
        assert!(p
            .request_profile(CatalogKind::Query, "orders by region")
            .is_none());
    }

    /// **The invalidation rule.** A landing registration answer means the files may have moved,
    /// so the cached scan is a lie — for the table *and* for the views that read it, which is
    /// the half a view can't derive from its own row (D10). Both arms of the answer count: a
    /// refusal invalidates just as surely as a success.
    #[test]
    fn a_landing_registration_answer_invalidates_the_profiles_it_makes_stale() {
        let mut p = settled();
        p.request_profile(CatalogKind::Table, "orders");
        p.request_profile(CatalogKind::View, "orders_daily");
        p.request_profile(CatalogKind::Table, "users");

        p.table_registered(
            "orders",
            TableMeta {
                columns: Vec::new(),
                rows: Some(11),
            },
        );

        assert_eq!(p.profile_scan(CatalogKind::Table, "orders"), None);
        assert_eq!(
            p.profile_scan(CatalogKind::View, "orders_daily"),
            None,
            "the view reads `orders`, so its numbers went with it"
        );
        assert!(
            p.profile_scan(CatalogKind::Table, "users").is_some(),
            "an unrelated table's scan is untouched"
        );

        // The refusal arm.
        p.request_profile(CatalogKind::Table, "orders");
        p.table_failed("orders", "no such file".into());
        assert_eq!(p.profile_scan(CatalogKind::Table, "orders"), None);

        // And a view being re-created drops its own, on the views channel where the inspector
        // is listening.
        p.request_profile(CatalogKind::View, "orders_daily");
        p.view_registered("orders_daily", view_meta(&["orders"]));
        assert_eq!(p.profile_scan(CatalogKind::View, "orders_daily"), None);
    }

    /// A request is runtime state, so it must not reach the project file — and a dropped row
    /// takes its request with it.
    #[test]
    fn a_scan_request_is_neither_persisted_nor_outlives_its_row() {
        let mut p = settled();
        p.request_profile(CatalogKind::Table, "orders");
        let defs = p.defs();
        assert_eq!(defs.tables.len(), 2, "the defs are the defs");

        p.remove_table("orders");
        assert_eq!(p.profile_scan(CatalogKind::Table, "orders"), None);
    }

    // --- saved-query rename (P3-06) ----------------------------------------------------

    /// A rename is a **label** change: the id is untouched (so any tab bound to it stays bound)
    /// and the row moves to its new alphabetical slot, because the section renders in store
    /// order.
    #[test]
    fn renaming_a_saved_query_keeps_its_id_and_re_sorts_it() {
        let mut p = settled();
        p.upsert_saved_query(SavedQuery {
            id: Uuid::from_u128(2),
            name: "zebra".into(),
            sql: "SELECT 9".into(),
            meta: "—".into(),
        });
        assert_eq!(p.saved_queries[1].name, "zebra");

        p.rename_saved_query(Uuid::from_u128(2), "  aardvark  ");

        assert_eq!(p.saved_queries[0].name, "aardvark", "trimmed and re-sorted");
        assert_eq!(p.saved_queries[0].id, Uuid::from_u128(2), "same query");
        assert_eq!(p.saved_queries[0].sql, "SELECT 9", "same SQL");
        assert_eq!(p.saved_queries.len(), 2, "a rename is not an insert");
    }

    /// A blank name is refused rather than stored — the row has nothing else to show.
    #[test]
    fn renaming_a_saved_query_to_nothing_is_refused() {
        let mut p = settled();
        let id = p.saved_queries[0].id;

        p.rename_saved_query(id, "   ");

        assert_eq!(p.saved_queries[0].name, "orders by region");
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
