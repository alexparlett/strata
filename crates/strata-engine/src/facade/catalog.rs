//! The workspace catalog: what is registered in it, and what a scan says about it.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

use strata_core::project::resolve_source;
use strata_core::project::ProjectDefs;
use strata_model::TableDef;
use strata_model::ViewDef;

use crate::catalog::{self, deregister_anywhere, TableMeta, TableSpec, ViewMeta};
use crate::ident::fold_ident;
use crate::register::{self, CatalogSpec, PassReport, Stamped};
use crate::statements::arms::{self, unsettled};
use crate::statements::report::StatementOutcome;
use crate::statements::{StatementReport, StmtKind, StoreEffect};
use crate::{
    profile, BackgroundGuard, CatalogGen, Engine, EngineError, ProfileRun, RegStatus,
    Registrations, Scans, SourceDefs, StopReason,
};

/// This engine's workspace catalog, from [`Engine::catalog`].
///
/// The tables and views it has registered, plus the one expensive question asked of an entry:
/// its column statistics. Profiling is here rather than on a workspace because a profile is a
/// property of the data — keyed by the entry, and two tables profile concurrently.
#[derive(Clone, Copy)]
pub struct Catalog<'a> {
    pub(super) engine: &'a Engine,
}

impl Catalog<'_> {
    /// Makes this engine hold exactly the catalog `desired` describes, handing `settled` what it
    /// answered per entry.
    ///
    /// `desired` is the entire catalog, never a work list: what it does not name is taken out and
    /// reported. See [`register::sync`](crate::register::sync) for the rest of the contract.
    pub async fn sync(self, desired: CatalogSpec, settled: impl FnMut(Stamped)) -> PassReport {
        register::sync(self.engine, desired, settled).await
    }

    /// Which generation of this catalog names currently resolve against.
    ///
    /// Every registry write the engine makes moves it, and nothing else does; a consumer keeps
    /// the number it derived an answer against and re-derives when this stops matching it.
    /// One atomic load, so a render pass may ask it.
    pub fn generation(self) -> CatalogGen {
        self.engine.generation.current()
    }

    /// **What this engine last answered for each def it was asked to register** — the
    /// registration ledger, read as of one moment, in both its namespaces.
    ///
    /// The outcome of a registration is the engine's own decision, so the engine retains it and
    /// a host renders it rather than keeping its own copy: a row is the def the store holds
    /// joined with the answer here, keyed on the [`generation`](Self::generation) the read is
    /// stamped with. A def no pass has reached is **absent**, which is the stated staleness
    /// bound and not a state of its own.
    ///
    /// One read for the whole catalog, deliberately: a walk that asked per row would describe a
    /// different instant per row, and would reach the engine from inside a render pass. A caller
    /// that is already reading [`Sources::listing`](crate::Sources::listing) has a data source's
    /// answer on the listing beside it, from this same ledger.
    ///
    /// Costs no I/O.
    pub fn registrations(self) -> Registrations {
        self.engine.ledger.registrations(self.generation())
    }

    /// (Re)register one external table from its spec, returning its inferred schema +
    /// free row count.
    ///
    /// Aborts the table's profile scan first: re-registration re-infers the schema from
    /// whatever is on disk *now*, so a scan in flight is computing numbers about files the
    /// register is replacing. Done here rather than left to the caller because it is engine
    /// truth, so every path that re-registers gets it, including ones written later.
    ///
    /// The name resolves to the provider it already had until the new one is built and swapped
    /// in, so nothing observes it as absent.
    ///
    /// Moves the [`generation`](Self::generation) on either arm: a failed registration takes the
    /// old provider out, so a name that resolved no longer does.
    pub async fn register(self, spec: TableSpec) -> Result<TableMeta, EngineError> {
        self.cancel_profile(&spec.name);
        let ctx = self.engine.ctx.clone();
        let formats = self.engine.formats.clone();
        let tables = self.engine.tables.clone();
        let (name, internal) = (spec.name.clone(), spec.internal);
        let source = spec.source.clone();
        let meta = self
            .engine
            .rt()
            .spawn(async move { catalog::register_external(&ctx, &formats, &*tables, &spec).await })
            .await
            .map_err(|e| EngineError::task("register", e))?;
        self.engine.note_origin(&name, internal && meta.is_ok());
        self.engine.note_scans(&name, Some(Scans::Table(source)));
        self.engine.note_registration(&name, RegStatus::of(&meta));
        meta.map_err(EngineError::from)
    }

    /// Drops a registered table. Its data is untouched; deleting an internal table's files is
    /// [`drop_table`](Self::drop_table)'s.
    ///
    /// Moves the [`generation`](Self::generation) whether or not `table` was registered, and
    /// **answers the generation it moved to** — the number a host's view of the ledger is keyed
    /// on. `must_use` for that reason: a caller that drops it has made the engine answer
    /// differently and told nothing, which is a surface left showing the answer before this one.
    #[must_use]
    pub fn deregister(self, table: &str) -> CatalogGen {
        self.cancel_profile(table);
        deregister_anywhere(&self.engine.ctx, table);
        self.engine.note_origin(table, false);
        self.engine.note_scans(table, None);
        self.engine.forget_registration(table)
    }

    /// What `name`'s row says **now** — its columns and free row count — read from the files
    /// without re-registering the table.
    ///
    /// The answer an `INSERT` needs, and the reason it is not [`register`](Self::register):
    /// re-registering builds a fresh provider and swaps it in, and **that** is what
    /// leaves every view above it holding a stale `Arc`. Views survive it only
    /// because the caller then re-creates them. An append cannot make them stale — the sink
    /// schema-checks before it writes, so the shape a view captured is the shape that is still
    /// there — so re-registering after one would break the views and repair them again for
    /// nothing, and re-infer a schema that could not have moved on the way.
    ///
    /// The count is still *read*, never added up from what a statement claimed: this re-LISTs
    /// the sources and totals the footers, of which only the appended file's is uncached.
    pub async fn table_meta(self, name: String) -> Result<TableMeta, EngineError> {
        let ctx = self.engine.ctx.clone();
        self.engine
            .rt()
            .spawn(async move { catalog::table_meta(&ctx, &name).await })
            .await
            .map_err(|e| EngineError::task("table meta", e))?
            .map_err(EngineError::from)
    }

    /// The Hive partition keys under `paths`, outermost first — what the Configure window's
    /// Hive section offers. Listed through the session's object store, so it answers for a bucket
    /// as readily as for a local folder.
    ///
    /// `paths` are as a table def stores them: relative to `data source` where one is named, and to
    /// `root` otherwise. **Composing the address is this side's**, because the scheme a store is
    /// registered under is the registry's answer and a caller that composed one would be keeping a
    /// second copy of it.
    /// One table's registration spec, with its sources resolved against `root`.
    ///
    /// Composed by the engine for [`spec`](Self::spec)'s reason: turning a table's source into
    /// the `scheme://authority` its files hang off is a **registry** question — the scheme
    /// belongs to the kind — and the registry is the engine's.
    pub fn table_spec(self, root: &Path, def: &TableDef, sources: &SourceDefs) -> TableSpec {
        register::table_spec(root, def, sources, self.engine.registry())
    }

    /// The whole catalog `defs` describe, with every table's sources resolved against `root`.
    ///
    /// Composed by the engine rather than by the spec itself: a host with defs in hand and no
    /// engine cannot turn a kind into a scheme, and every host that syncs has one.
    pub fn spec(self, root: &Path, defs: &ProjectDefs) -> CatalogSpec {
        CatalogSpec::of_project(root, defs, self.engine.registry())
    }

    /// The Hive partition columns `paths` are laid out by, as a directory walk finds them.
    pub async fn detect_partitions(
        self,
        source: Option<String>,
        root: Option<PathBuf>,
        paths: Vec<String>,
    ) -> Vec<String> {
        let prefix = source.as_deref().and_then(|named| {
            self.engine
                .source_defs
                .prefix(&self.engine.registrants, named)
        });
        let root = root.unwrap_or_default();
        let resolved: Vec<String> = paths
            .iter()
            .map(|path| resolve_source(&root, prefix.as_deref(), path))
            .collect();
        let ctx = self.engine.ctx.clone();
        self.engine
            .rt()
            .spawn(async move { catalog::detect_partitions(&ctx, &resolved).await })
            .await
            .unwrap_or_default()
    }

    /// Creates or redefines the SQL view `name` over `sql` — the ⌘S gesture, where a typed
    /// `CREATE VIEW` goes through [`Workspace::run`](crate::Workspace::run). `CREATE OR REPLACE`:
    /// a view of that name is redefined rather than refused.
    ///
    /// The report's effect is [`StoreEffect::ViewUpserted`], the one a typed `CREATE VIEW`
    /// answers with, so one fold applies either; its message is worded for the gesture rather
    /// than for the statement.
    ///
    /// Moves the [`generation`](Self::generation) on either arm: a failed redefinition leaves
    /// the name resolving to a definition the caller has just been told is wrong. Its bookkeeping
    /// is [`register_view`](Self::register_view)'s, so there is no effect left for
    /// `settle_effect` to settle — the report is stamped here, after that call and never
    /// before it.
    pub async fn create_view(
        self,
        name: String,
        sql: String,
    ) -> Result<StatementReport, EngineError> {
        let start = Instant::now();
        let meta = self.register_view(name.clone(), sql.clone()).await?;
        Ok(unsettled(
            StmtKind::CreateView,
            start,
            StatementOutcome {
                message: format!("Saved view '{name}'"),
                count: None,
                effect: Some(StoreEffect::ViewUpserted {
                    def: ViewDef { name, sql },
                    meta,
                }),
            },
        )
        .at(self.generation()))
    }

    /// [`create_view`](Self::create_view) without the report — the registration pass's entry,
    /// which wants the [`ViewMeta`] for a [`RegOutcome::View`](crate::register::RegOutcome::View).
    ///
    /// The engine's own bookkeeping is here rather than in the report's
    /// [`settle_effect`](Engine::settle_effect), because a replay has no sentence to ask for and
    /// would otherwise leave the dependency map behind.
    pub(crate) async fn register_view(
        self,
        name: String,
        sql: String,
    ) -> Result<ViewMeta, EngineError> {
        self.cancel_profile(&name);
        let ctx = self.engine.ctx.clone();
        let created_as = name.clone();
        let created = self
            .engine
            .rt()
            .spawn(async move { arms::create_view(&ctx, &name, &sql).await })
            .await
            .map_err(|e| EngineError::task("create view", e))?;
        if let Ok(meta) = &created {
            self.engine.note_scans(
                &created_as,
                Some(Scans::View {
                    tables: meta.tables.clone(),
                    remote: meta.remote.clone(),
                }),
            );
        }
        self.engine
            .note_registration(&created_as, RegStatus::of(&created));
        created.map_err(EngineError::from)
    }

    /// Registers `tables`, then creates (or redefines) `views`, handing `settled` each answer as
    /// it lands — [`sync`](Self::sync)'s additive half, **without its reconciliation**.
    ///
    /// What a caller whose question is about *part* of the catalog asks: a row's Refresh
    /// re-registers one table and re-creates the views over it, and has no opinion about the rest
    /// of the catalog. `sync` would read the same work list as "the project is now this", and take
    /// everything else out.
    ///
    /// Views repeat in rounds until one makes no progress, so a view whose dependency is created
    /// earlier in the same call succeeds on a later round and the rest settle with the errors they
    /// last produced. Where the views may already exist, hand them in dependency order: every
    /// `CREATE OR REPLACE` then succeeds on the first round, and an outer view inlines the
    /// definition this call is replacing.
    ///
    /// Data sources are not among the phases, deliberately: connecting one is a whole-catalog
    /// gesture (`sync`'s first phase), and re-resolving a credential chain behind a question about
    /// one table's files would put a network round trip where none belongs.
    pub async fn refresh(
        self,
        tables: Vec<TableSpec>,
        views: Vec<ViewDef>,
        settled: impl FnMut(Stamped),
    ) {
        register::register_pass(self.engine, Vec::new(), tables, views, settled).await;
    }

    /// Drop the SQL view `name` (idempotent — `IF EXISTS`) — the catalog pane's entry into
    /// [`arms::drop_view`], as a typed `DROP VIEW` reaches it through
    /// [`Workspace::run`](crate::Workspace::run).
    ///
    /// Answers a [`StatementReport`] for [`drop_table`](Self::drop_table)'s reason: one answer
    /// shape, so a surface that folds one gesture's outcome folds the other's. The dependents a
    /// typed `DROP VIEW` names are the *statement's* — this gesture's confirm has already shown
    /// them from the store, before anything was destroyed.
    pub async fn drop_view(self, name: String) -> Result<StatementReport, EngineError> {
        self.cancel_profile(&name);
        let start = Instant::now();
        let ctx = self.engine.ctx.clone();
        let dropped = name.clone();
        self.engine
            .rt()
            .spawn(async move { arms::drop_view(&ctx, &dropped).await })
            .await
            .map_err(|e| EngineError::task("drop view", e))??;
        let outcome = StatementOutcome {
            message: format!("View '{name}' dropped"),
            count: None,
            effect: Some(StoreEffect::ViewRemoved { name }),
        };
        Ok(self
            .engine
            .settle_effect(unsettled(StmtKind::DropView, start, outcome)))
    }

    /// Drop the registered table `name` — **the one funnel both surfaces drop through**.
    ///
    /// A typed `DROP TABLE` reaches the same body through
    /// [`Workspace::run`](crate::Workspace::run)'s interception; the catalog pane's confirm
    /// reaches it here, after it has taken the def out of the store and written `project.json`
    /// (the store-first order `save_view` established — a drop the project file never heard
    /// about comes back on the next open). Two gestures, one implementation, because the
    /// difference between them is a *question asked of the user*, not a difference in what the
    /// drop does: an internal table's data directory goes with it on both paths, which is the
    /// whole reason this is not two calls.
    ///
    /// `if_exists` is the statement's clause. The pane passes `true`: the row it is dropping came
    /// out of the store, and a def whose registration failed has no provider to deregister.
    pub async fn drop_table(
        self,
        name: String,
        if_exists: bool,
    ) -> Result<StatementReport, EngineError> {
        let _deleting = BackgroundGuard::new(self.engine);
        let start = Instant::now();
        let ctx = self.engine.ctx.clone();
        let root = self.engine.data_root.lock().unwrap().clone();
        let internal = self.engine.internal.clone();
        let tables = self.engine.tables.clone();
        let outcome = self
            .engine
            .rt()
            .spawn(async move {
                arms::drop_table(&ctx, &root, &internal, &*tables, &name, if_exists).await
            })
            .await
            .map_err(|e| EngineError::task("drop table", e))??;
        Ok(self
            .engine
            .settle_effect(unsettled(StmtKind::DropTable, start, outcome)))
    }

    /// Whether `name` is a table whose data Strata owns — the one question the internal-name set
    /// exists to answer (see [`InternalTables`](crate::InternalTables)). `false` for an external
    /// table, a view, and a name that is not registered at all.
    pub fn is_internal(self, name: &str) -> bool {
        self.engine.internal.contains(name)
    }

    /// Profile the catalog entry `name` — **one full scan, one aggregate, every column at
    /// once** (see [`profile`]). Works for a table or a view: a view has no footer at all,
    /// so a scan is the only way it learns anything beyond a column's type.
    ///
    /// Deliberately expensive and deliberately opt-in: distinct counts can't be merged across
    /// files, so there is no cheaper form. The UI confirms before a first scan and
    /// caches the result until the entry changes.
    ///
    /// Superseded-by-dispatch like [`Workspace::query`](crate::Workspace::query): a re-scan
    /// aborts the scan it replaces, and the older call settles
    /// [`StopReason::SupersededScan`](crate::StopReason::SupersededScan) rather than tearing
    /// down the entry the newer one now owns. Dedup is the *caller's* (freya-query keys the
    /// cache by the request), which is why two arrivals here mean two real requests.
    pub async fn profile(self, name: String) -> Result<profile::CatalogProfile, EngineError> {
        let engine = self.engine;
        let key = fold_ident(&name);
        let dispatch = engine.dispatch_seq.fetch_add(1, Ordering::Relaxed);
        let task = {
            let mut lc = engine.lifecycle.lock().unwrap();
            if let Some(prev) = lc.profiles.remove(&key) {
                prev.abort.abort();
            }
            let ctx = engine.ctx.clone();
            let scanned = name.clone();
            let task = engine
                .rt()
                .spawn(async move { catalog::run_profile(&ctx, &scanned).await });
            lc.profiles.insert(
                key.clone(),
                ProfileRun {
                    dispatch,
                    abort: task.abort_handle(),
                },
            );
            engine.publish_inflight(&lc);
            task
        };

        let joined = task.await;

        let mut lc = engine.lifecycle.lock().unwrap();
        let latest = lc.profiles.get(&key).map(|p| p.dispatch) == Some(dispatch);
        if latest {
            lc.profiles.remove(&key);
        }
        engine.publish_inflight(&lc);
        match joined {
            Ok(res) if latest => res.map_err(EngineError::from),
            Ok(_) => Err(EngineError::Stopped(StopReason::SupersededScan)),
            Err(join) if join.is_cancelled() => Err(EngineError::Stopped(StopReason::Cancelled)),
            Err(join) => Err(EngineError::task("profile", join)),
        }
    }

    /// Abort the profile scan of `name`, if one is running — `true` when something was
    /// actually cancelled. The awaiting [`profile`](Self::profile) settles
    /// [`StopReason::Cancelled`](crate::StopReason::Cancelled).
    ///
    /// Unguarded by any nonce, unlike [`Workspace::cancel`](crate::Workspace::cancel): a scan
    /// is keyed by the entry, and every caller — the inspector's Cancel, and every catalog
    /// mutation that is about to make the result a lie — means "stop scanning *this entry*".
    pub fn cancel_profile(self, name: &str) -> bool {
        let mut lc = self.engine.lifecycle.lock().unwrap();
        let cancelled = match lc.profiles.remove(&fold_ident(name)) {
            Some(run) => {
                run.abort.abort();
                true
            }
            None => false,
        };
        self.engine.publish_inflight(&lc);
        cancelled
    }
}
