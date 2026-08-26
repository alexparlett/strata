//! The workspace catalog: what is registered in it, and what a scan says about it.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Instant;

use strata_core::project::resolve_source;

use crate::catalog::{self, TableMeta, TableSpec, ViewMeta};
use crate::statements::arms::{self, stamped};
use crate::statements::report::StatementOutcome;
use crate::statements::{StatementReport, StmtKind, StoreEffect};
use crate::{
    fold_ident, profile, store, BackgroundGuard, Engine, ProfileRun, CANCELLED, SUPERSEDED_SCAN,
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
    /// (Re)register one external table from its spec, returning its inferred schema +
    /// free row count.
    ///
    /// Aborts the table's profile scan first: re-registration re-infers the schema from
    /// whatever is on disk *now*, so a scan in flight is computing numbers about files the
    /// register is replacing. Done here rather than left to the caller because it is engine
    /// truth, so every path that re-registers gets it, including ones written later.
    pub async fn register(self, spec: TableSpec) -> Result<TableMeta, String> {
        self.cancel_profile(&spec.name);
        let ctx = self.engine.ctx.clone();
        let (name, internal) = (spec.name.clone(), spec.internal);
        let meta = self
            .engine
            .rt()
            .spawn(async move { catalog::register_external(&ctx, &spec).await })
            .await
            .map_err(|e| format!("register task failed: {e}"))?;
        self.engine.note_origin(&name, internal && meta.is_ok());
        meta
    }

    /// Drop a registered table.
    pub fn deregister(self, table: &str) {
        self.cancel_profile(table);
        let _ = self.engine.ctx.deregister_table(table);
        self.engine.note_origin(table, false);
    }

    /// What `name`'s row says **now** — its columns and free row count — read from the files
    /// without re-registering the table.
    ///
    /// The answer an `INSERT` needs, and the reason it is not [`register`](Self::register):
    /// re-registering deregisters the provider and builds a fresh one, and **that** is what
    /// leaves every view above it holding a stale `Arc`. Views survive it only
    /// because the caller then re-creates them. An append cannot make them stale — the sink
    /// schema-checks before it writes, so the shape a view captured is the shape that is still
    /// there — so re-registering after one would break the views and repair them again for
    /// nothing, and re-infer a schema that could not have moved on the way.
    ///
    /// The count is still *read*, never added up from what a statement claimed: this re-LISTs
    /// the sources and totals the footers, of which only the appended file's is uncached.
    pub async fn table_meta(self, name: String) -> Result<TableMeta, String> {
        let ctx = self.engine.ctx.clone();
        self.engine
            .rt()
            .spawn(async move { catalog::table_meta(&ctx, &name).await })
            .await
            .map_err(|e| format!("table meta task failed: {e}"))?
    }

    /// The Hive partition keys under `paths`, outermost first — what the Configure window's
    /// Hive section offers. Listed through the session's object store, so it answers for a bucket
    /// as readily as for a local folder.
    ///
    /// `paths` are as a table def stores them: relative to `connection` where one is named, and to
    /// `root` otherwise. **Composing the address is this side's**, because the scheme a store is
    /// registered under is the registry's answer and a caller that composed one would be keeping a
    /// second copy of it.
    pub async fn detect_partitions(
        self,
        connection: Option<String>,
        root: Option<PathBuf>,
        paths: Vec<String>,
    ) -> Vec<String> {
        let prefix = connection
            .as_deref()
            .and_then(|named| self.engine.connections.identity(named))
            .and_then(|identity| store::store_prefix(&identity));
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

    /// Create (or redefine) the SQL view `name` over `sql`, returning its columns and
    /// what it reads — **the ⌘S gesture's entry into [`arms::create_view`]**, which a
    /// typed `CREATE VIEW` enters through [`Workspace::run`](crate::Workspace::run) instead.
    /// `CREATE OR REPLACE`: redefinition is the ⌘S-on-a-view path.
    pub async fn create_view(self, name: String, sql: String) -> Result<ViewMeta, String> {
        self.cancel_profile(&name);
        let ctx = self.engine.ctx.clone();
        self.engine
            .rt()
            .spawn(async move { arms::create_view(&ctx, &name, &sql).await })
            .await
            .map_err(|e| format!("create view task failed: {e}"))?
    }

    /// Drop the SQL view `name` (idempotent — `IF EXISTS`) — the catalog pane's entry into
    /// [`arms::drop_view`], as a typed `DROP VIEW` reaches it through
    /// [`Workspace::run`](crate::Workspace::run).
    ///
    /// Answers a [`StatementReport`] for [`drop_table`](Self::drop_table)'s reason: one answer
    /// shape, so a surface that folds one gesture's outcome folds the other's. The dependents a
    /// typed `DROP VIEW` names are the *statement's* — this gesture's confirm has already shown
    /// them from the store, before anything was destroyed.
    pub async fn drop_view(self, name: String) -> Result<StatementReport, String> {
        self.cancel_profile(&name);
        let start = Instant::now();
        let ctx = self.engine.ctx.clone();
        let dropped = name.clone();
        self.engine
            .rt()
            .spawn(async move { arms::drop_view(&ctx, &dropped).await })
            .await
            .map_err(|e| format!("drop view task failed: {e}"))??;
        let outcome = StatementOutcome {
            message: format!("View '{name}' dropped"),
            count: None,
            effect: Some(StoreEffect::ViewRemoved { name }),
        };
        let report = stamped(StmtKind::DropView, start, outcome);
        self.engine.settle_effect(report.effect.as_ref());
        Ok(report)
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
    ) -> Result<StatementReport, String> {
        let _deleting = BackgroundGuard::new(self.engine);
        let start = Instant::now();
        let ctx = self.engine.ctx.clone();
        let root = self.engine.data_root.lock().unwrap().clone();
        let internal = self.engine.internal.clone();
        let outcome = self
            .engine
            .rt()
            .spawn(async move { arms::drop_table(&ctx, &root, &internal, &name, if_exists).await })
            .await
            .map_err(|e| format!("drop table task failed: {e}"))??;
        let report = stamped(StmtKind::DropTable, start, outcome);
        self.engine.settle_effect(report.effect.as_ref());
        Ok(report)
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
    /// `Err("superseded by a newer scan")` rather than tearing down the entry the newer one now
    /// owns. Dedup is the *caller's* (freya-query keys the cache by the request), which is why
    /// two arrivals here mean two real requests.
    pub async fn profile(self, name: String) -> Result<profile::CatalogProfile, String> {
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
            Ok(res) if latest => res,
            Ok(_) => Err(SUPERSEDED_SCAN.into()),
            Err(join) if join.is_cancelled() => Err(CANCELLED.into()),
            Err(join) => Err(format!("profile task failed: {join}")),
        }
    }

    /// Abort the profile scan of `name`, if one is running — `true` when something was
    /// actually cancelled. The awaiting [`profile`](Self::profile) settles `Err("cancelled")`.
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
