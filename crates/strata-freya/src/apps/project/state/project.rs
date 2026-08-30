//! The per-window **Project** store (Radio): the open project's catalog — the *save
//! targets* (state-arch §2). Each row wraps a pure persisted def ([`TableDef`] /
//! [`ViewDef`]) with what engine registration *learned* about it, so the durable and the
//! derived can't blur: `defs()` is a projection, not a clone-and-hope.
//!
//! **Whether a def registered is not here.** That is an outcome the engine decided, so the
//! engine retains it ([`Registrations`](strata_engine::Registrations)) and a row is the def
//! this store holds *joined* with that answer — the desired state is the store's authority,
//! the observed state is the engine's, and neither is copied into the other. What a row keeps
//! is only what registration **learned**: a table's [`TableMeta`], a view's [`ViewMeta`], both
//! absent until one has landed and dropped again by a failure, which learns nothing.
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
//! view's dependency lists are the exception on the read side: they come back from the *planner*,
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
use strata_core::project::{self as project_io, name_ord, ProjectDefs};
use strata_engine::register::view_order;
use strata_engine::{RegStatus, Registrations, TableMeta, ViewMeta};
use strata_model::{CatalogKind, SavedQuery, SourceDef, TableDef, ViewDef};
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
    /// The project's data sources (W7 · DB-02) — object stores *and* databases, on their own
    /// channel for the same reason the sections have theirs: connecting one must not wake the
    /// TABLES section. Since DB-05 they are nodes of the data-sources tree rather than a pane
    /// beside it, which changes who subscribes and not why the channel is separate.
    Sources,
    Tables,
    Views,
    /// Notified by save-as-query (⌘S on a scratch / saved-query tab); subscribed by
    /// the sidebar QUERIES section (Phase 3).
    Queries,
}

impl RadioChannel<ProjectState> for ProjChan {}

/// One catalog table: its persisted def + what registering it learned.
pub struct TableRow {
    pub def: TableDef,
    /// What the last successful registration inferred — columns and the free row count. `None`
    /// until one lands, and `None` again after a refusal, which infers nothing. **Whether the
    /// table is registered right now is not this field**: that is
    /// [`Registrations`](strata_engine::Registrations)'s to answer, and a row asks it by name.
    pub meta: Option<TableMeta>,
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
            meta: None,
            profile: None,
        }
    }

    /// The row's summary label ("6 cols · 2 partitions") — derived, never stored. Read by the
    /// sidebar rows and by the command palette's TABLES rows, so both say the same thing about
    /// a table.
    ///
    /// `status` is the engine's answer for this row, from the join. It decides the two labels
    /// that are not counts, and it is asked **before** the columns: a table that registered and
    /// then failed still holds the shape the earlier pass inferred, and a row that answered "6
    /// cols" about a def the engine has just refused would be the one lie this join could tell.
    pub fn meta_label(&self, status: Option<&RegStatus>) -> String {
        match (status, &self.meta) {
            (None, _) => "loading…".into(),
            (Some(RegStatus::Failed { .. }), _) => "failed".into(),
            (Some(RegStatus::Ready), None) => "loading…".into(),
            (Some(RegStatus::Ready), Some(m)) if self.def.partition_cols.is_empty() => {
                format!("{} cols", m.columns.len())
            }
            (Some(RegStatus::Ready), Some(m)) => format!(
                "{} cols · {} partitions",
                m.columns.len(),
                self.def.partition_cols.len()
            ),
        }
    }
}

/// One catalog view: its persisted def + what creating it learned.
pub struct ViewRow {
    pub def: ViewDef,
    /// What the last successful creation learned — the engine's [`ViewMeta`] whole, exactly as
    /// [`TableRow::meta`] holds a table's, and see that field for why whether the view is
    /// *registered* is not here.
    ///
    /// Which surface reads which of its three lists: [`view_problem`](ProjectState::view_problem)
    /// and [`dependent_views`](ProjectState::dependent_views) read `tables`, the drop confirm's
    /// view case reads `views`, and `remote` is read by neither — both reconcile against the
    /// project's own rows, and a relation in a data source's catalog has none.
    pub info: Option<ViewMeta>,
    /// The profile scan asked for on this view — see [`TableRow::profile`]. A view is where a
    /// scan buys the most: it has no files under it, so it reports nothing for free.
    pub profile: Option<ScanId>,
}

impl ViewRow {
    fn new(def: ViewDef) -> Self {
        Self {
            def,
            info: None,
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
    /// The sources the project reads through — **the defs and nothing else**, connecting
    /// learning nothing a row could carry. Kept sorted by address like every other section is
    /// sorted by name; **identity is the data source's own name**, which is what an engine
    /// answer is addressed by. Two data sources may share an address (`s3://lake` read two ways,
    /// one server reached as two roles) and are then simply neighbours in the sort.
    pub sources: Vec<SourceDef>,
    pub tables: Vec<TableRow>,
    pub views: Vec<ViewRow>,
    pub saved_queries: Vec<SavedQuery>,
}

/// One def the engine refused — a row of the Problems drawer's **Project** tab.
///
/// Kept as a projection rather than a stored list, because a registration failure already *is*
/// live state: the engine's own answer for the def, joined onto its row. Re-deriving it is what
/// makes the drawer retract the row the moment a re-scan fixes the def, with nothing to
/// invalidate — the same property the SQL diagnostics have, reached a different way.
#[derive(Clone, PartialEq, Debug)]
pub struct RegistrationFault {
    /// What kind of def it is — the row says which rather than making the user infer it from
    /// the name.
    pub kind: FaultKind,
    pub name: String,
    /// What the engine said. P3-07's wording, the same text the catalog row's triangle carries.
    pub why: String,
}

/// What kind of def a [`RegistrationFault`] is about.
///
/// Its own closed vocabulary rather than [`CatalogKind`], because a **data source** is not one:
/// it registers beside the catalog and fails in exactly the same shape, but it is an object
/// store rather than a member of the SQL namespace, so it has no place in the enum that
/// `dependent_views` and `name_in_use` dispatch on. Saved queries go the other way — they are
/// a `CatalogKind` and can never be a fault, because a stored string is never registered.
///
/// A type and not a noun string: the set is closed, the drawer dispatches on it, and a
/// `&'static str` would put the rendering of these three words at whichever call site got there
/// first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaultKind {
    Source,
    Table,
    View,
}

impl FaultKind {
    /// How a row refers to it — the Problems drawer's trailing tag.
    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "data source",
            Self::Table => "table",
            Self::View => "view",
        }
    }
}

impl ProjectState {
    /// Every def the engine refused, **in registration order** — data sources, then tables, then
    /// views — so anything broken *by* something above it reads below its cause.
    ///
    /// A refused **data source** (W7) is here for the reason the probe behind it exists
    /// (`CONNECTIONS_SPEC.md` §3): a bucket with no usable credentials takes every table over
    /// it down with it, so without this row the drawer fills with signing failures on the
    /// tables and says nothing about the one thing that is actually wrong. It is named by the
    /// data source's own name rather than by its address, which is the only form that tells two
    /// data sources over one bucket apart.
    ///
    /// Saved queries can't appear: they are stored strings that are never registered, so there is
    /// no engine answer for them to have failed.
    ///
    /// The refusals are the **engine's**, joined onto this store's rows by name: the drawer
    /// lists the project's own defs, in the project's own order, and says about each one what the
    /// engine last said. Walking the ledger instead would list a name whose def this store no
    /// longer holds.
    pub fn registration_faults(&self, registrations: &Registrations) -> Vec<RegistrationFault> {
        let fault = |kind: FaultKind, name: String, why: Option<&str>| {
            why.map(|why| RegistrationFault {
                kind,
                name,
                why: why.to_string(),
            })
        };
        let sources = self.sources.iter().filter_map(|def| {
            let name = def.named();
            fault(
                FaultKind::Source,
                name.clone(),
                registrations.sources.problem(&name),
            )
        });
        let tables = self.tables.iter().filter_map(|r| {
            fault(
                FaultKind::Table,
                r.def.name.clone(),
                registrations.workspace.problem(&r.def.name),
            )
        });
        let views = self.views.iter().filter_map(|r| {
            fault(
                FaultKind::View,
                r.def.name.clone(),
                registrations.workspace.problem(&r.def.name),
            )
        });
        sources.chain(tables).chain(views).collect()
    }

    /// How many defs the engine refused — [`registration_faults`](Self::registration_faults)'s
    /// length without building it.
    ///
    /// Separate because the count is read on **every** render of the rail badge and the drawer's
    /// scope strip, where the list itself is built only when the Project scope is actually on
    /// screen. Going through the list would clone two `String`s per failed def just to drop them
    /// again a line later.
    pub fn registration_fault_count(&self, registrations: &Registrations) -> usize {
        let workspace = self
            .tables
            .iter()
            .map(|r| r.def.name.as_str())
            .chain(self.views.iter().map(|r| r.def.name.as_str()))
            .filter(|name| registrations.workspace.problem(name).is_some())
            .count();
        let sources = self
            .sources
            .iter()
            .filter(|def| registrations.sources.problem(&def.named()).is_some())
            .count();
        workspace + sources
    }

    /// The store for a project loaded (or scaffolded) from `root` — the defs, with nothing
    /// learned about any of them until the registration pass answers.
    pub fn from_defs(defs: ProjectDefs, root: PathBuf) -> Self {
        Self {
            name: defs.name,
            root,
            sources: defs.sources,
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
            sources: self.sources.clone(),
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

    /// Why `name` cannot be taken, if something already has it — [`name_in_use`](Self::name_in_use)
    /// as the sentence a form shows.
    ///
    /// One wording, because two surfaces ask this: the Configure window's footer (P4-11) and the
    /// empty-table panel (IT-01). A name is free or it is not, and a user who saw the two
    /// surfaces disagree about how to say so would reasonably wonder whether they were being
    /// told the same thing.
    pub fn name_taken(&self, name: &str) -> Option<String> {
        let kind = self.name_in_use(name)?;
        Some(format!(
            "'{name}' is already the name of a {}.",
            match kind {
                CatalogKind::Table => "table",
                CatalogKind::View => "view",
                CatalogKind::Query => "saved query",
            }
        ))
    }

    /// Land a table registration answer on its row.
    ///
    /// The one funnel every table answer arrives through — project open, a catalog re-scan
    /// (P3-03), and a table-config save (P4-11) all land here — which is why it is also where
    /// the **profile is invalidated** (P3-09): a landing answer means the files may have moved
    /// under the row, and that is exactly when a cached full scan becomes a lie.
    /// Matched with [`same_name`](Self::same_name) for the reason
    /// [`reload_table`](Self::reload_table) is: an answer does not always arrive under the def's
    /// own spelling. An `INSERT`'s row-count refresh names the table the *planner* resolved
    /// (ED-05), which folds an unquoted identifier while the def keeps whatever was typed.
    pub fn table_registered(&mut self, name: &str, meta: TableMeta) {
        self.land_table(name, Some(meta));
    }

    /// Land a **refused** table registration: the row keeps its def and drops what an earlier
    /// pass had inferred, a refusal having learned nothing. Why it was refused is the engine's
    /// ([`Registrations`]), not a second copy here.
    pub fn table_failed(&mut self, name: &str) {
        self.land_table(name, None);
    }

    /// What a table registration answer leaves on its row, either way — see
    /// [`table_registered`](Self::table_registered).
    fn land_table(&mut self, name: &str, meta: Option<TableMeta>) {
        if let Some(r) = self
            .tables
            .iter_mut()
            .find(|r| Self::same_name(&r.def.name, name))
        {
            r.meta = meta;
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
    /// The answer lands **whole**: it arrives already resolved, so there is nothing to filter
    /// here. Every name in it still compares with [`same_name`](Self::same_name) wherever these
    /// lists are *read*, because they come back from the planner — which folds unquoted
    /// identifiers — while def names carry whatever the user typed.
    pub fn view_registered(&mut self, name: &str, meta: ViewMeta) {
        self.land_view(name, Some(meta));
    }

    /// Land a **refused** view creation: the row drops what an earlier creation learned, for
    /// [`table_failed`](Self::table_failed)'s reason.
    pub fn view_failed(&mut self, name: &str) {
        self.land_view(name, None);
    }

    /// What a view creation answer leaves on its row, either way.
    fn land_view(&mut self, name: &str, info: Option<ViewMeta>) {
        if let Some(v) = self.views.iter_mut().find(|v| v.def.name == name) {
            v.info = info;
            v.profile = None;
        }
    }

    /// A view's problem, if any:
    ///
    /// - the **hard** failure the engine reported — the SQL didn't plan (a syntax error,
    ///   or a base table already missing when the view was created); or
    /// - a **missing dependency** — a base table it reads is gone from the catalog, or is
    ///   itself failing to register. [`ViewMeta::tables`] is the *transitive* base-table set
    ///   (the planner inlines nested views at creation), so this reaches through a
    ///   view-of-a-view, and it catches a table dropped **after** the view registered
    ///   cleanly, which raises no event of its own.
    ///
    /// A cross-source view's **remote** reads ([`ViewMeta::remote`]) are deliberately not
    /// checked here, and the reason is what this check *is*: a reconciliation against the
    /// project's own rows. A relation in a data source's catalog has no row, so every
    /// answer this loop could give about one would be "not in the catalog" — a triangle on every
    /// working cross-source view. Whether the data source still has it is the data source's
    /// answer, and it lands the only way it can: the view fails to re-plan at the next
    /// registration pass, in `catalog::view_error`'s words.
    ///
    /// Note what the triangle does *not* claim. Verified against DataFusion 54: dropping a
    /// base table does **not** break the view's live plan — that plan captured each source
    /// by `Arc` at creation and never re-resolves the name, so `SELECT * FROM the_view`
    /// still answers. What is true is that the view will not survive a reload, which is
    /// why it is flagged as *left invalid*. It is also why validity is derived from `tables`
    /// rather than by re-issuing `CREATE OR REPLACE VIEW`: a re-plan catches a directly
    /// missing base table but a view-of-a-view masks it behind the same live `Arc`.
    pub fn view_problem(&self, row: &ViewRow, registrations: &Registrations) -> Option<String> {
        let answers = &registrations.workspace;
        if let Some(why) = answers.problem(&row.def.name) {
            return Some(why.to_string());
        }
        let info = row.info.as_ref()?;
        info.tables.iter().find_map(|dep| {
            match self
                .tables
                .iter()
                .find(|t| Self::same_name(&t.def.name, dep))
            {
                None => Some(format!("Reads {dep}, which is no longer in the catalog.")),
                Some(t) if answers.problem(&t.def.name).is_some() => {
                    Some(format!("Reads {dep}, which failed to load."))
                }
                Some(_) => None,
            }
        })
    }

    /// The scan asked for on `name`, if any — what the inspector subscribes to and what the
    /// sidebar row spins about. `None` is the un-profiled state (the zone's scan card).
    pub fn profile_scan(&self, kind: CatalogKind, name: &str) -> Option<ScanId> {
        match kind {
            CatalogKind::View => self
                .views
                .iter()
                .find(|v| Self::same_name(&v.def.name, name))
                .and_then(|v| v.profile),
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

    /// The views a drop of `name` would leave **invalid**, alphabetically (rows are kept
    /// sorted) — the other direction of the same deps [`view_problem`](Self::view_problem)
    /// reads (D10). The drop confirm's consequence line and its name chips.
    ///
    /// Which list answers depends on what is being dropped. [`ViewMeta::tables`] is the *base
    /// tables* a view reads — transitive, because the planner inlines nested views at creation —
    /// so it reaches a view-of-a-view over the dropped table; [`ViewMeta::views`] is the *views*
    /// it reads, the only thing that can answer the view case. A saved query is not a SQL object, so
    /// it has no dependents. Names fold case, because deps come back from the planner and the
    /// dropped name comes from a def.
    ///
    /// **Left invalid, not broken.** A dependent view's live plan captured its sources by `Arc` at
    /// creation and never re-resolves their names, so it keeps answering after the drop and fails
    /// only on the next reload. The confirm says exactly what the row's triangle will say.
    ///
    /// A view with no landed answer is **not** listed: there is no dependency information to read
    /// off it, and a row that never registered is already flagged on its own account.
    pub fn dependent_views(&self, kind: CatalogKind, name: &str) -> Vec<String> {
        self.views
            .iter()
            .filter(|v| !Self::same_name(&v.def.name, name))
            .filter(|v| {
                let Some(info) = v.info.as_ref() else {
                    return false;
                };
                match kind {
                    CatalogKind::Table => &info.tables,
                    CatalogKind::View => &info.views,
                    CatalogKind::Query => return false,
                }
                .iter()
                .any(|dep| Self::same_name(dep, name))
            })
            .map(|v| v.def.name.clone())
            .collect()
    }

    /// Every view a **Refresh** of the table `name` must re-create, in dependency order.
    ///
    /// Two sets, for two different reasons:
    ///
    /// - the views that **read** it ([`ViewMeta::tables`], so transitively through a
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
    pub fn views_to_refresh(&self, name: &str, registrations: &Registrations) -> Vec<String> {
        let mut views = self.dependent_views(CatalogKind::Table, name);
        for v in &self.views {
            let failing = registrations.workspace.problem(&v.def.name).is_some();
            if failing && !views.iter().any(|n| n == &v.def.name) {
                views.push(v.def.name.clone());
            }
        }
        self.refresh_order(views)
    }

    /// Order `views` so that a view is re-created **after** every view it reads — the
    /// store's projection over [`view_order`] (`strata-engine`, beside the pass it
    /// orders): each view's known dependencies are its landed
    /// [`ViewMeta::views`], and a view with no landed answer carries none, so it
    /// sorts wherever it falls — at project open that is every view, which is why the
    /// scan keeps its fixed-point retry as well.
    pub fn refresh_order(&self, views: Vec<String>) -> Vec<String> {
        view_order(views, |name| {
            self.views
                .iter()
                .find(|v| Self::same_name(&v.def.name, name))
                .and_then(|v| v.info.as_ref())
                .map(|info| info.views.clone())
                .unwrap_or_default()
        })
    }

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

    /// Insert-or-replace a data source def (the editor's Save), at its address-sorted slot. Resets
    /// the row to `Loading`, like every other upsert here — the data source has to be registered
    /// again before the row may claim anything.
    ///
    /// **Matched on the name, inserted by `address`**, and the two being different keys is the
    /// whole of what this method has to get right. The list is sorted by address and identified by
    /// name, so two data sources over one bucket are neighbours in the sort and two different
    /// rows: replacing on the address would let saving one silently take out the other,
    /// deregistering nothing and leaving a live store with no def.
    ///
    /// It does **not** deregister anything. An edit that moves the address or the kind changes
    /// what the data source registered under, and that store survives this write untouched;
    /// dropping it is `Sources::disconnect`, owed by the gesture that knows both names.
    pub fn upsert_source(&mut self, def: SourceDef) {
        let name = def.named();
        self.sources.retain(|c| c.named() != name);
        let at = self
            .sources
            .partition_point(|c| name_ord(c.setting("address"), def.setting("address")).is_lt());
        self.sources.insert(at, def);
    }

    /// Save a data source that was **renamed**: the row moves, and every table reading through it
    /// moves with it.
    ///
    /// A table names the data source it reads through, so a rename that left those references
    /// behind would point them at a data source the project no longer has — the tables would fail
    /// to register with "no suitable object store", naming a data source the user can see is
    /// there under its new name.
    ///
    /// **In one settle**, and here rather than in the window that pressed Save: what a rename
    /// costs is a property of how a data source is referenced, and a surface that had to remember
    /// to fix the references would be one that could forget.
    pub fn rename_source(&mut self, from: &str, def: SourceDef) {
        let to = def.named();
        self.remove_source(from);
        self.upsert_source(def);
        if from == to {
            return;
        }
        for table in &mut self.tables {
            if table.def.source.as_deref() == Some(from) {
                table.def.source = Some(to.clone());
            }
        }
    }

    /// Edit a data source's def **in place** — the schemas picker's write.
    ///
    /// Legitimate exactly because the field it exists for is **display-only**: registration
    /// exposes every schema a data source can reach and the def's own `schemas` scopes what
    /// Strata shows, so what the last pass answered about this data source is still true after
    /// the write, and nothing here asks the engine to answer again.
    ///
    /// `edit` must not move the def's identity — the row keeps its slot and its key, so a URL or
    /// address change here would leave the list sorted wrong and the engine registered under a
    /// URL no def names. That edit is the data source editor's, and it goes through `upsert`.
    pub fn update_source_def(&mut self, name: &str, edit: impl FnOnce(&mut SourceDef)) {
        let Some(def) = self.sources.iter_mut().find(|c| c.named() == name) else {
            return;
        };
        edit(def);
    }

    /// Forget the data source called `name` — the store half of the pane's Forget. Hands back the
    /// row and its slot, like [`remove_view`](Self::remove_view).
    ///
    /// Matched on the name exactly, which is where it parts company with every other remover
    /// here: those fold, because a table or a view name is one the user may also write in SQL.
    /// A data source is addressed by gesture rather than by typing — the callers pass back the
    /// spelling they read off the row — so there is no second spelling to reconcile with.
    pub fn remove_source(&mut self, name: &str) -> Option<(usize, SourceDef)> {
        let at = self.sources.iter().position(|c| c.named() == name)?;
        Some((at, self.sources.remove(at)))
    }

    /// Put a def back where [`remove_source`](Self::remove_source) took it from.
    pub fn restore_source(&mut self, at: usize, def: SourceDef) {
        self.sources.insert(at.min(self.sources.len()), def);
    }
}

/// **The engine's half of a catalog row, composed by hand**: what the ledger says about each
/// name, without an engine to answer for it.
///
/// Here rather than in one test module because five surfaces render this join — the tree, the
/// palette, the inspector, the Problems drawer and the two editor windows — and each of them has
/// to be able to put a def into every state the ledger can report it in.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct Answered {
    workspace: std::collections::BTreeMap<String, RegStatus>,
    sources: std::collections::BTreeMap<String, RegStatus>,
}

#[cfg(test)]
impl Answered {
    pub fn ready(mut self, name: &str) -> Self {
        self.workspace.insert(name.to_string(), RegStatus::Ready);
        self
    }

    pub fn failed(mut self, name: &str, why: &str) -> Self {
        self.workspace.insert(name.to_string(), refused(why));
        self
    }

    /// Take a name back out — a def the engine has not answered for, which is what a project
    /// open looks like before the pass reaches it and what the ledger's *absence* means.
    pub fn unanswered(mut self, name: &str) -> Self {
        self.workspace.remove(name);
        self
    }

    pub fn source_ready(mut self, name: &str) -> Self {
        self.sources.insert(name.to_string(), RegStatus::Ready);
        self
    }

    pub fn source_failed(mut self, name: &str, why: &str) -> Self {
        self.sources.insert(name.to_string(), refused(why));
        self
    }

    pub fn read(&self) -> Registrations {
        let stamp = strata_engine::CatalogGen::default();
        Registrations {
            workspace: strata_engine::Answers::recorded(self.workspace.clone(), stamp),
            sources: strata_engine::Answers::recorded(self.sources.clone(), stamp),
            generation: stamp,
        }
    }
}

#[cfg(test)]
fn refused(why: &str) -> RegStatus {
    RegStatus::Failed {
        reason: why.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_engine::sources::postgres::Pg;
    use strata_engine::{Answers, CatalogGen, SourceKind};
    use strata_model::{SourceDef, SourceFormat, TableOrigin};

    fn table_def(name: &str) -> TableDef {
        TableDef {
            name: name.into(),
            format: SourceFormat::Parquet,
            source: None,
            paths: vec![format!("{name}.parquet")],
            partition_cols: vec![("year".into(), "Int32".into())],
            origin: TableOrigin::External,
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
            ..Default::default()
        };
        let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-reload-test"));
        p.table_registered(
            "orders",
            TableMeta {
                columns: Vec::new(),
                rows: Some(10),
            },
        );
        p.table_failed("users");
        p.view_registered(
            "orders_daily",
            ViewMeta {
                columns: Vec::new(),
                tables: vec!["orders".into()],
                remote: Vec::new(),
                views: Vec::new(),
            },
        );
        p
    }

    /// What the engine answered about [`settled`]'s defs.
    fn answered() -> Answered {
        Answered::default()
            .ready("orders")
            .failed("users", "no such file")
            .ready("orders_daily")
    }

    /// **A row keeps only what registration learned, and a refusal learned nothing.** The
    /// verdict itself is not here at all — it is the engine's, joined on by name — so the store
    /// has no state that can disagree with it, and the defs are untouched either way.
    #[test]
    fn a_row_keeps_what_was_learned_and_a_refusal_drops_it() {
        let mut p = settled();
        assert!(p.tables[0].meta.is_some(), "the settled table's shape");
        assert!(
            p.tables[1].meta.is_none(),
            "the refused one learned nothing"
        );
        assert!(p.views[0].info.is_some());
        let before = p.defs();

        p.table_failed("orders");
        p.view_failed("orders_daily");

        assert!(p.tables[0].meta.is_none(), "dropped, not kept stale");
        assert!(p.views[0].info.is_none());
        assert_eq!(before.tables[0].paths, p.defs().tables[0].paths);
        assert_eq!(before.views[0].sql, p.defs().views[0].sql);
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

        p.remove_view("orders_daily");
        p.remove_table("orders");
        assert!(p.views.is_empty());
        assert_eq!(p.tables.len(), 1);
    }

    /// A landing answer lands on the row it names and on no other.
    #[test]
    fn a_landing_answer_lands_on_its_own_row() {
        let mut p = settled();

        p.table_registered(
            "users",
            TableMeta {
                columns: Vec::new(),
                rows: Some(3),
            },
        );

        assert_eq!(p.tables[1].def.name, "users");
        assert_eq!(p.tables[1].meta.as_ref().and_then(|m| m.rows), Some(3));
        assert_eq!(
            p.tables[0].meta.as_ref().and_then(|m| m.rows),
            Some(10),
            "its neighbour is untouched"
        );
    }

    fn view_meta(deps: &[&str]) -> ViewMeta {
        ViewMeta {
            columns: Vec::new(),
            tables: deps.iter().map(|d| (*d).to_string()).collect(),
            remote: Vec::new(),
            views: Vec::new(),
        }
    }

    /// A table says what the engine said, and *only* when the engine has refused it. The
    /// unanswered case is the one worth pinning: a def the ledger has no entry for is one no
    /// pass has reached, and treating that as a problem would put a triangle on the whole
    /// catalog at every open.
    #[test]
    fn a_table_is_invalid_only_once_registration_has_actually_failed() {
        let answers = answered().read();

        assert_eq!(answers.workspace.problem("orders"), None, "registered");
        assert_eq!(
            answers.workspace.problem("users"),
            Some("no such file"),
            "refused, carrying the engine's own reason"
        );
        assert_eq!(
            Registrations::default().workspace.problem("users"),
            None,
            "an unanswered def is not a broken one"
        );
    }

    /// A view's hard failure — the SQL never planned — is reported verbatim, and short-circuits
    /// the dependency walk (there are no deps to walk: nothing landed).
    #[test]
    fn a_view_reports_the_failure_the_engine_gave_it() {
        let mut p = settled();
        p.view_failed("orders_daily");
        let answers = answered()
            .failed("orders_daily", "Schema error: No field named x")
            .read();

        assert_eq!(
            p.view_problem(&p.views[0], &answers).as_deref(),
            Some("Schema error: No field named x")
        );
    }

    /// The derived half: a view that registered *cleanly* turns invalid when a base table it
    /// reads leaves the catalog. Dropping a table raises no event of the view's own — this is
    /// the only thing that notices.
    #[test]
    fn a_view_over_a_dropped_table_is_invalid() {
        let mut p = settled();
        let answers = answered().read();
        assert_eq!(
            p.view_problem(&p.views[0], &answers),
            None,
            "healthy to begin with"
        );

        p.remove_table("orders");

        assert_eq!(
            p.view_problem(&p.views[0], &answers).as_deref(),
            Some("Reads orders, which is no longer in the catalog.")
        );
    }

    /// A base table that is *present but broken* is just as fatal to the view, and says so in
    /// its own words — "failed to load" points at the table, not at the view's SQL.
    #[test]
    fn a_view_over_a_failed_table_is_invalid() {
        let mut p = settled();
        p.table_failed("orders");
        let answers = answered()
            .failed("orders", "No such file or directory (os error 2)")
            .read();

        assert_eq!(
            p.view_problem(&p.views[0], &answers).as_deref(),
            Some("Reads orders, which failed to load.")
        );
    }

    /// A dep the engine has not answered for is not a problem. At project open that is every
    /// table for a moment, and flagging then would strobe the whole VIEWS section.
    #[test]
    fn a_view_whose_base_table_has_not_been_answered_for_is_not_flagged() {
        let p = settled();
        let answers = answered().unanswered("orders").read();

        assert_eq!(p.view_problem(&p.views[0], &answers), None);
    }

    /// Nothing is stored, so nothing has to be invalidated: the answer follows the catalog.
    /// Re-register the table and the view is simply valid again on the next read.
    #[test]
    fn a_views_validity_heals_when_its_base_table_comes_back() {
        let p = settled();
        assert!(p
            .view_problem(&p.views[0], &answered().failed("orders", "gone").read())
            .is_some());

        assert_eq!(p.view_problem(&p.views[0], &answered().read()), None);
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
        p.view_registered("orders_weekly", view_meta(&["orders"]));

        p.remove_table("orders");

        let outer = p
            .views
            .iter()
            .find(|v| v.def.name == "orders_weekly")
            .expect("the nested view is in the catalog");
        assert_eq!(
            p.view_problem(outer, &answered().ready("orders_weekly").read())
                .as_deref(),
            Some("Reads orders, which is no longer in the catalog.")
        );
    }

    /// A view meta that reads views as well as tables — what the engine lands for a view over
    /// a view: the base tables inlined at the leaves, and the views it reads resolved beside them.
    fn view_meta_over(tables: &[&str], views: &[&str]) -> ViewMeta {
        ViewMeta {
            columns: Vec::new(),
            tables: tables.iter().map(|d| (*d).to_string()).collect(),
            remote: Vec::new(),
            views: views.iter().map(|d| (*d).to_string()).collect(),
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

    /// Dropping a **view** is the other lookup — `views`, not `tables`. Asserting both
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
    /// The guard is held at both layers, and this pins the **lookup's**. The row is written
    /// directly rather than through `view_registered`, because the engine holds a view's own name
    /// back when it resolves the list — a self-reference is a state only a fixture can reach,
    /// which is what makes the lookup's own guard worth pinning.
    #[test]
    fn a_view_is_never_listed_as_its_own_dependent() {
        let mut p = settled();
        p.views[0].info = Some(ViewMeta {
            columns: Vec::new(),
            tables: Vec::new(),
            remote: Vec::new(),
            views: vec!["orders_daily".into()],
        });
        assert_eq!(
            p.views[0].def.name, "orders_daily",
            "the self-referencing row"
        );

        assert!(p
            .dependent_views(CatalogKind::View, "orders_daily")
            .is_empty());
    }

    /// The view→view direction folds case at **lookup**: the engine resolves these names off the
    /// planner, which lower-cases an unquoted identifier, while def names keep the user's
    /// capitals. An exact compare here would answer that nothing reads a view that something
    /// does — right before a destructive drop.
    #[test]
    fn view_dependencies_fold_case_at_the_lookup() {
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
            ..Default::default()
        };
        let mut p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-viewdeps-fold-test"));
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

        p.views[0].info = None;

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
            p.views_to_refresh("orders", &answered().ready("user_signups").read()),
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

        assert_eq!(
            p.views_to_refresh("orders", &answered().ready("a_orders_weekly").read()),
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
        p.view_failed("user_signups");
        let answers = answered()
            .failed("user_signups", "table 'users' not found")
            .read();

        let refreshed = p.views_to_refresh("orders", &answers);
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
        p.views[0].info = None;

        let names: Vec<String> = p.views.iter().map(|v| v.def.name.clone()).collect();
        assert_eq!(p.refresh_order(names.clone()), names);
    }

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
        assert!(p.request_profile(CatalogKind::Table, "ORDERS").is_some());
        assert!(p.request_profile(CatalogKind::Table, "nope").is_none());
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

        p.request_profile(CatalogKind::Table, "orders");
        p.table_failed("orders");
        assert_eq!(p.profile_scan(CatalogKind::Table, "orders"), None);

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
            ..Default::default()
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

        assert_eq!(
            p.view_problem(
                &p.views[0],
                &Answered::default()
                    .ready("Orders")
                    .ready("orders_daily")
                    .read()
            ),
            None
        );
    }

    /// Two data sources over **one bucket** — the pair every data source lookup has to tell apart,
    /// and the one a bucket-keyed store would land both answers on.
    fn two_stores_one_bucket() -> ProjectState {
        let defs = ProjectDefs {
            name: "test".into(),
            sources: vec![
                SourceDef {
                    config: [("address".to_string(), "lake".into())]
                        .into_iter()
                        .collect(),
                    name: "lake".into(),
                    kind: "s3".into(),
                    ..Default::default()
                },
                SourceDef {
                    config: [("address".to_string(), "lake".into())]
                        .into_iter()
                        .collect(),
                    name: "lake2".into(),
                    kind: "gcs".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-sources-store-test"))
    }

    /// **A name is unique across the whole project, catalogs notwithstanding.**
    ///
    /// A store data source's catalog is *placement*, not a namespace liberalization: a bucket
    /// table's provider lives in its source's catalog, and the name it lives under is still the
    /// project's own, checked against every table, view and saved query regardless of what any
    /// of them reads through. Asserted explicitly because the placement is exactly the change
    /// that would tempt a per-catalog answer, and a bare name would then be ambiguous.
    #[test]
    fn a_name_is_taken_across_the_whole_project_whatever_it_reads_through() {
        let over = |name: &str, source: &str| TableDef {
            name: name.into(),
            format: SourceFormat::from_name("csv"),
            source: Some(source.into()),
            paths: vec!["data/".into()],
            partition_cols: Vec::new(),
            origin: TableOrigin::External,
        };
        let defs = ProjectDefs {
            name: "test".into(),
            tables: vec![over("regions", "lake"), over("events", "lake2")],
            ..Default::default()
        };
        let p = ProjectState::from_defs(defs, PathBuf::from("/tmp/strata-global-names"));

        for name in ["regions", "events"] {
            assert_eq!(
                p.name_in_use(name),
                Some(CatalogKind::Table),
                "'{name}' is taken whichever data source it reads through"
            );
        }
        assert!(
            p.name_taken("REGIONS").is_some(),
            "and case-insensitively, like every other name in the catalog"
        );
        assert_eq!(p.name_in_use("unused"), None);
    }

    /// Forget takes the row it was asked for and **only** that one. Keyed on the name, so the
    /// address the two share is not what is matched — an address-keyed remover would take
    /// whichever sorted first and leave the user's own data source gone.
    #[test]
    fn remove_source_matches_the_name_and_not_the_address() {
        let mut p = two_stores_one_bucket();

        let (at, def) = p.remove_source("lake2").expect("the GCS data source");
        assert_eq!(def.named(), "lake2");
        assert_eq!(
            p.sources.iter().map(SourceDef::named).collect::<Vec<_>>(),
            ["lake"],
            "the S3 data source over the same bucket is untouched"
        );

        p.restore_source(at, def);
        assert_eq!(
            p.sources.iter().map(SourceDef::named).collect::<Vec<_>>(),
            ["lake", "lake2"]
        );
    }

    /// The editor's Save **replaces on the name and sorts on the address** — the two keys the two
    /// halves of this method use, and the pair that makes it worth a test.
    ///
    /// Saving over `lake2` must leave `lake` exactly where it was. Replacing on the address
    /// instead (the sort's key, and the tempting one) takes out a data source the user never
    /// touched, deregisters nothing, and leaves a live object store with no def behind it.
    /// **A rename takes the tables with it.** A table names the data source it reads through, so
    /// the references move in the same settle — left behind, they would point at a data source the
    /// project no longer has while the user can see it under its new name.
    #[test]
    fn a_rename_moves_the_tables_that_read_through_it() {
        let mut p = two_stores_one_bucket();
        p.upsert_table(TableDef {
            source: Some("lake".into()),
            ..table_def("events")
        });
        p.upsert_table(TableDef {
            source: Some("lake2".into()),
            ..table_def("shipments")
        });
        p.upsert_table(table_def("local"));

        let renamed = SourceDef {
            name: "depot".into(),
            ..p.sources
                .iter()
                .find(|c| c.named() == "lake")
                .expect("the S3 data source")
                .clone()
        };
        p.rename_source("lake", renamed);

        let over = |table: &str| {
            p.tables
                .iter()
                .find(|t| t.def.name == table)
                .and_then(|t| t.def.source.clone())
        };
        assert_eq!(over("events").as_deref(), Some("depot"), "it moved with it");
        assert_eq!(
            over("shipments").as_deref(),
            Some("lake2"),
            "and the other data source's tables did not"
        );
        assert_eq!(
            over("local"),
            None,
            "nor did a local one gain a data source"
        );
        assert!(
            p.sources.iter().any(|c| c.named() == "depot")
                && !p.sources.iter().any(|c| c.named() == "lake"),
            "the def itself moved"
        );
    }

    #[test]
    fn upsert_source_replaces_the_name_and_sorts_by_it() {
        let mut p = two_stores_one_bucket();

        p.upsert_source(SourceDef {
            kind: "gcs".into(),
            name: "lake2".into(),
            config: [("address", "lake"), ("auth", "anonymous")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        });

        assert_eq!(p.sources.len(), 2, "the GCS row was replaced, not added");
        let gcs = p
            .sources
            .iter()
            .find(|c| c.named() == "lake2")
            .expect("the GCS data source");
        assert_eq!(gcs.kind, "gcs", "…and it carries what was saved");
        assert_eq!(
            gcs.config.get("auth").map(String::as_str),
            Some("anonymous")
        );
        assert!(
            p.sources.iter().any(|c| c.named() == "lake"),
            "the S3 data source over the same bucket is left alone"
        );
        p.upsert_source(SourceDef {
            config: [("address".to_string(), "acme".into())]
                .into_iter()
                .collect(),
            name: "acme".into(),
            kind: "s3".into(),
            ..Default::default()
        });
        assert_eq!(
            p.sources.iter().map(SourceDef::named).collect::<Vec<_>>(),
            ["acme", "lake2", "lake"],
            "sorted by the name (`name_ord`'s own order), which is what a source is addressed by"
        );
    }

    /// A URL nothing is registered under removes nothing, rather than taking the nearest row.
    #[test]
    fn remove_source_ignores_a_name_it_does_not_hold() {
        let mut p = two_stores_one_bucket();
        assert!(p.remove_source("s3://other").is_none());
        assert_eq!(p.sources.len(), 2);
    }

    /// A refused **data source** is a project problem, and it leads the list: registration order
    /// is sources, then tables, then views, so anything broken *by* the data source reads
    /// below its cause. It is named by its URL, which is the only form that says which store.
    #[test]
    fn a_refused_source_leads_the_project_faults() {
        let mut p = two_stores_one_bucket();
        p.upsert_table(table_def("orders"));
        p.table_failed("orders");
        let answers = Registrations {
            workspace: Answers::recorded(
                [(
                    "orders".to_string(),
                    RegStatus::Failed {
                        reason: "No suitable object store found".into(),
                    },
                )],
                CatalogGen::default(),
            ),
            sources: Answers::recorded(
                [(
                    "lake".to_string(),
                    RegStatus::Failed {
                        reason: "This S3 data source needs a region.".into(),
                    },
                )],
                CatalogGen::default(),
            ),
            ..Default::default()
        };

        let faults = p.registration_faults(&answers);
        assert_eq!(
            faults
                .iter()
                .map(|f| (f.kind, f.name.as_str()))
                .collect::<Vec<_>>(),
            [(FaultKind::Source, "lake"), (FaultKind::Table, "orders")]
        );
        assert_eq!(faults[0].why, "This S3 data source needs a region.");
        assert_eq!(
            p.registration_fault_count(&answers),
            faults.len(),
            "the count the rail badge reads is the length of the list the drawer renders"
        );
    }

    /// A data source the pass has not answered for yet is **not** a problem — a project mid-scan
    /// must not flash every bucket it has not reached as a fault.
    #[test]
    fn an_unanswered_source_is_not_a_fault() {
        let p = two_stores_one_bucket();
        let none = Registrations::default();
        assert!(p.registration_faults(&none).is_empty());
        assert_eq!(p.registration_fault_count(&none), 0);
    }

    /// A **database** data source def, for the two questions only a database raises here.
    fn pg(database: &str, schemas: &[&str]) -> SourceDef {
        SourceDef {
            kind: Pg::NAME.to_string(),
            name: database.into(),
            config: [
                ("address", format!("db.internal:5432/{database}")),
                ("user", "reader".to_string()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
            schemas: schemas.iter().map(ToString::to_string).collect(),
            ..Default::default()
        }
    }

    /// **The schemas picker edits the def and nothing else.** The row it draws is that def
    /// joined with what the engine answered, and this write asks the engine nothing — which is
    /// legitimate exactly because the field is display-only.
    #[test]
    fn editing_a_source_def_in_place_touches_only_that_def() {
        let mut p = ProjectState::from_defs(
            ProjectDefs {
                name: "test".into(),
                sources: vec![pg("analytics", &["public"])],
                ..Default::default()
            },
            PathBuf::from("/tmp/strata-schemas-write"),
        );
        let name = p.sources[0].named();

        p.update_source_def(&name, |def| {
            def.schemas = vec!["public".into(), "warehouse".into()];
        });

        assert_eq!(p.sources[0].schemas, ["public", "warehouse"]);
        assert_eq!(p.sources.len(), 1, "edited in place, not inserted");
    }
}
