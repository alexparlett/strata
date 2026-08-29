//! The catalog list, catalog and schema **Strata owns** — identity and visibility, never
//! lifecycle.
//!
//! DataFusion's provider traits are resolution and enumeration interfaces: `register_table` is sync
//! and carries no caller identity, so nothing here could spool a CTAS result or authorize a `DROP`.
//! Lifecycle lives in [`statements::arms`](super::statements::arms), in front of `ctx.sql`. What the traits *can* do is fix
//! the namespace's shape and decide what enumerating it returns — this module's two jobs:
//!
//! * **Identity** — the workspace catalog has one schema, tables keyed by [`fold_ident`].
//!   [`StrataCatalogProvider::register_schema`] refuses, so `CREATE SCHEMA x` fails at the provider
//!   even with the router bypassed. `CREATE DATABASE` cannot be stopped here — `register_catalog`
//!   returns an `Option` and has no way to refuse — so the classifier's `Fault::CreateDatabase` is
//!   its only gate. The scoping is load-bearing since the DB workstream: the session holds one
//!   catalog per data source too, and it is the *workspace* whose flat bare-name namespace
//!   is one-catalog-one-schema.
//! * **Removability** — [`StrataCatalogList`] exists for the one thing DataFusion cannot serve:
//!   `CatalogProviderList` has `register_catalog` and no counterpart. Forgetting a database
//!   data source has to make its catalog stop resolving, or a removed source stays queryable until
//!   the window is re-opened.
//! * **Replaceability** — [`StrataSchemaProvider::replace`] is the other operation the traits do
//!   not have. `register_table` refuses a name that is already there, so a re-registration needs
//!   a swap; held under the map's own lock it has no window where the name resolves to nothing.
//! * **Visibility** — [`StrataSchemaProvider::table_names`] drops the `__snap_`-prefixed snapshots
//!   while `table()` still resolves them. Every `information_schema` view and `SHOW` form
//!   enumerates through `table_names()`, so one filter hides the spool from all of them, and
//!   `__strata_ord` never reaches `information_schema.columns`. Every other reader addresses a
//!   snapshot **by name**, so none notices. The prefix is [`is_snapshot_name`], beside the function
//!   that mints the names, so the hiding rule and the naming rule cannot drift.
//!
//! That filter is **this** provider's, and a data source's
//! ([`SourceSchemaProvider`](super::sources::providers::SourceSchemaProvider)) deliberately has
//! none: the namespace is the workspace
//! catalog's, so a remote relation a server happens to call `__snap_x` is an ordinary table — the
//! same scoping [`is_snapshot_ref`](super::snapshots::is_snapshot_ref) applies to the refusal, off
//! [`in_workspace`].
//!
//! Everything else delegates to the map verbatim, `MemorySchemaProvider`'s duplicate-name error
//! included, so every existing reader keeps working with no call-site changes.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use async_trait::async_trait;
use datafusion::catalog::{CatalogProvider, CatalogProviderList, SchemaProvider, TableProvider};
use datafusion::common::{exec_err, DataFusionError, Result};
use datafusion::prelude::SessionContext;
use datafusion::sql::TableReference;

use super::snapshots::is_snapshot_name;
use super::sources::providers::SourceCatalogProvider;
use super::{fold_ident, CATALOG, SCHEMA};

/// Whether `name` addresses **the workspace catalog's one schema** — the three spellings of
/// one place (`orders`, `public.orders`, `strata.public.orders`), and nothing else.
///
/// One predicate rather than the test written per caller, because two rules turn on it and must not
/// drift: what an intercepted statement may create, drop or write
/// ([`resolve_target`](super::statements::resolve_target)), and what the `__snap_` namespace covers
/// ([`is_snapshot_ref`](super::snapshots::is_snapshot_ref)). Since the DB workstream the session holds
/// more than one catalog, so "is this name ours" is a real question.
///
/// Reference-shaped, so it is asked of the value DataFusion resolved: a `Partial` whose schema is
/// not `public` names a schema the workspace catalog cannot have, which is why it answers false
/// rather than consulting the catalog list.
///
/// **Each part is compared the way the thing that resolves it compares.** [`StrataCatalogList`]
/// keys catalogs by [`fold_ident`], so a quoted `"STRATA"` reaches the workspace catalog and must
/// answer true — comparing it raw let that spelling out of the `__snap_` namespace, which is a way
/// to read another tab's snapshot. [`StrataCatalogProvider::schema`] compares its one schema
/// **exactly**, so `"PUBLIC"` resolves to nothing and false is the honest answer.
pub(super) fn in_workspace(name: &TableReference) -> bool {
    match name {
        TableReference::Bare { .. } => true,
        TableReference::Partial { schema, .. } => schema.as_ref() == SCHEMA,
        TableReference::Full {
            catalog, schema, ..
        } => fold_ident(catalog) == CATALOG && schema.as_ref() == SCHEMA,
    }
}

/// The engine's **catalog list**: DataFusion's, plus the one operation it does not have.
///
/// `MemoryCatalogProviderList` can register a catalog and never remove one, so a database
/// data source could be forgotten and go on answering `pg.public.orders` for the life of the
/// window. This is the same map with [`deregister`](Self::deregister) on it — installed at
/// [`build_context`](super::build_context) time via `SessionStateBuilder::with_catalog_list`,
/// so the builder registers the workspace catalog into *this* list and nothing else moves.
///
/// Keyed by [`fold_ident`] because catalog names are unquoted identifiers and DataFusion looks
/// one up already folded (`TableReference::resolve`) — the same rule, in the same place, as
/// [`StrataSchemaProvider`]'s table names.
///
/// It refuses nothing: `register_catalog` returns an `Option` by DataFusion's own signature, so
/// there is no "no" to say here and `CREATE DATABASE`'s gate stays the router's, exactly as
/// before.
#[derive(Debug, Default)]
pub struct StrataCatalogList {
    /// Keyed by [`fold_ident`], valued by the name it was **registered under** beside the
    /// provider: DataFusion enumerates catalogs through [`catalog_names`](Self::catalog_names)
    /// and stamps whatever it answers into `information_schema.tables.table_catalog` and every
    /// `SHOW` form, so folding the *enumeration* — which `MemoryCatalogProviderList` does not do
    /// — would print a catalog name no def, no surface and no user ever wrote. A database
    /// data source deliberately registers the user's own spelling.
    catalogs: RwLock<BTreeMap<String, Registered>>,
}

/// One entry: the name it was registered under, and the provider. See [`StrataCatalogList`].
type Registered = (String, Arc<dyn CatalogProvider>);

impl StrataCatalogList {
    /// Take the catalog registered under `name` back out — the half `CatalogProviderList` is
    /// missing. `None` when nothing was registered under it, which is the ordinary case for a
    /// data source that never connected and is not a fault.
    pub fn deregister(&self, name: &str) -> Option<Arc<dyn CatalogProvider>> {
        self.catalogs
            .write()
            .unwrap()
            .remove(&fold_ident(name))
            .map(|(_, catalog)| catalog)
    }
}

impl CatalogProviderList for StrataCatalogList {
    fn register_catalog(
        &self,
        name: String,
        catalog: Arc<dyn CatalogProvider>,
    ) -> Option<Arc<dyn CatalogProvider>> {
        self.catalogs
            .write()
            .unwrap()
            .insert(fold_ident(&name), (name, catalog))
            .map(|(_, catalog)| catalog)
    }

    /// The names catalogs were **registered under**, not the folded keys — see the field.
    fn catalog_names(&self) -> Vec<String> {
        self.catalogs
            .read()
            .unwrap()
            .values()
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn catalog(&self, name: &str) -> Option<Arc<dyn CatalogProvider>> {
        self.catalogs
            .read()
            .unwrap()
            .get(&fold_ident(name))
            .map(|(_, catalog)| Arc::clone(catalog))
    }
}

/// The namespaces `catalog` shows, or `None` — the workspace's own catalog, or a test's
/// stand-in — which a caller reads as "no scoping to apply".
///
/// The downcast is DataFusion's own pattern for a custom provider ([`deregister_catalog`] is the
/// other one). It lives here rather than beside the type it downcasts to because its caller is
/// the name resolver, and [`sql`](crate::sql) and [`sources`](crate::sources) are peers that may
/// not reach into each other — where "what does DataFusion's catalog list answer" is exactly this
/// module's question.
pub(crate) fn shown_schemas(catalog: &dyn CatalogProvider) -> Option<BTreeSet<String>> {
    let source: &SourceCatalogProvider = (catalog as &dyn Any).downcast_ref()?;
    Some(source.shown())
}

/// Remove the catalog registered under `name` from `ctx` — [`StrataCatalogList::deregister`]
/// reached through the session, which is all a caller holds.
///
/// `None` when nothing was registered under that name. The downcast is DataFusion's own
/// documented pattern for a custom list (`impl dyn CatalogProviderList`), and it cannot miss on
/// an engine this crate built: [`build_context`](super::build_context) installs the list before
/// anything can replace it.
pub fn deregister_catalog(ctx: &SessionContext, name: &str) -> Option<Arc<dyn CatalogProvider>> {
    let list = Arc::clone(ctx.state_ref().read().catalog_list());
    list.downcast_ref::<StrataCatalogList>()?.deregister(name)
}

/// Register a catalog shaped the way a live source's is — one namespace, some relations — so a
/// test can ask what the app does about a name inside one without a server.
///
/// The providers are the real pair a data source registers, over a source that speaks no SQL, so
/// what these tests drive is the thing itself rather than a stand-in for it. The connect is
/// skipped because their subject is the **catalog list**: each asks whether a catalog of that
/// name is registered and then works off the resolved reference.
#[cfg(test)]
pub(crate) fn fake_source(ctx: &SessionContext, catalog: &str, relations: &[&str]) {
    use crate::sources::fake::Rows;
    use crate::sources::providers::SourceCatalogProvider;

    let (handle, listing) = Rows::catalog(relations);
    let provider = SourceCatalogProvider::new(
        catalog.to_string(),
        format!("test-doc://{catalog}-fixture"),
        handle,
        &listing,
        Arc::new(RwLock::new(["public".to_string()].into_iter().collect())),
    );
    ctx.register_catalog(catalog, Arc::new(provider));
}

/// Strata's catalog: exactly one schema, [`SCHEMA`], for the whole life of the engine.
///
/// A second schema is not merely unsupported, it is unrepresentable — which is what makes
/// `CREATE SCHEMA` a structural impossibility rather than a policy the router has to keep
/// remembering.
#[derive(Debug, Default)]
pub struct StrataCatalogProvider {
    schema: Arc<StrataSchemaProvider>,
}

impl CatalogProvider for StrataCatalogProvider {
    fn schema_names(&self) -> Vec<String> {
        vec![SCHEMA.to_string()]
    }

    fn schema(&self, name: &str) -> Option<Arc<dyn SchemaProvider>> {
        (name == SCHEMA).then(|| Arc::clone(&self.schema) as Arc<dyn SchemaProvider>)
    }

    fn register_schema(
        &self,
        _name: &str,
        _schema: Arc<dyn SchemaProvider>,
    ) -> Result<Option<Arc<dyn SchemaProvider>>> {
        exec_err!("Strata has one schema, '{SCHEMA}'. New schemas cannot be created")
    }

    fn deregister_schema(
        &self,
        _name: &str,
        _cascade: bool,
    ) -> Result<Option<Arc<dyn SchemaProvider>>> {
        exec_err!("Strata has one schema, '{SCHEMA}'. It cannot be dropped")
    }
}

/// A **store data source's catalog**: one schema, [`SCHEMA`], holding a provider per table def
/// that reads through it.
///
/// Registered under the data source's own name while it is live, which is what makes forgetting
/// one *structural* rather than a warning in a confirm dialog: the catalog comes off the list and
/// its tables stop resolving with it, instead of a deregistration per table that a failure
/// half-finishes.
///
/// **Def-fed, and it enumerates nothing.** A bucket cannot say what its tables are, so what this
/// holds is derived from the project's own rows — the same `ListingTable`s
/// [`register_external`](super::catalog::register_external) has always built, placed here rather
/// than in the workspace. Catalog-is-the-store is intact: the store answers no question about
/// membership, the defs do.
///
/// **Placement, not a namespace.** Table names stay unique across the whole project, so
/// `lake.strata.sales` and a bare `sales` are the same name reached two ways — which is why
/// [`resolve_target`](super::statements::resolve_target) answers `Workspace` for both, and why
/// this catalog needs no rule of its own about what may be created in it.
pub struct StoreCatalogProvider {
    /// The name the data source registered under — what a refusal from here names.
    catalog: String,
    schema: Arc<StrataSchemaProvider>,
}

impl StoreCatalogProvider {
    /// An empty catalog for the data source called `catalog`. The registration pass fills it:
    /// sources register before tables, so it is in place before the first def that names it.
    pub fn new(catalog: String) -> Self {
        StoreCatalogProvider {
            catalog,
            schema: Arc::new(StrataSchemaProvider::default()),
        }
    }
}

impl fmt::Debug for StoreCatalogProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreCatalogProvider")
            .field("catalog", &self.catalog)
            .finish_non_exhaustive()
    }
}

impl CatalogProvider for StoreCatalogProvider {
    fn schema_names(&self) -> Vec<String> {
        vec![SCHEMA.to_string()]
    }

    fn schema(&self, name: &str) -> Option<Arc<dyn SchemaProvider>> {
        (name == SCHEMA).then(|| Arc::clone(&self.schema) as Arc<dyn SchemaProvider>)
    }

    fn register_schema(
        &self,
        _name: &str,
        _schema: Arc<dyn SchemaProvider>,
    ) -> Result<Option<Arc<dyn SchemaProvider>>> {
        exec_err!(
            "'{}' holds the tables this project reads through it, in one schema, '{SCHEMA}'. \
             New schemas cannot be created in it",
            self.catalog
        )
    }

    fn deregister_schema(
        &self,
        _name: &str,
        _cascade: bool,
    ) -> Result<Option<Arc<dyn SchemaProvider>>> {
        exec_err!(
            "'{}' holds the tables this project reads through it, in one schema, '{SCHEMA}'. \
             It cannot be dropped",
            self.catalog
        )
    }
}

/// Whether `catalog` is a **store** data source's — a name whose relations are the project's own
/// table defs rather than a server's.
///
/// Asked of the session because that is what resolves the name, and answered by the provider's
/// own type: a store catalog is registered exactly while its data source is live.
pub(crate) fn is_store_catalog(ctx: &SessionContext, catalog: &str) -> bool {
    ctx.catalog(catalog)
        .is_some_and(|provider| (provider.as_ref() as &dyn Any).is::<StoreCatalogProvider>())
}

/// The catalog a table def registers **into**: the store data source it reads through while that
/// source is live, and the workspace otherwise.
///
/// The fallback is not a defeat — it is every local table, and it is also a def whose source
/// failed to connect, which registers into the workspace and then fails on its paths with the
/// sentence that names the bucket. Asking the session rather than taking a parameter keeps this
/// one decision in one place: the sources phase runs first, so by the time a def is registered
/// the answer is already true.
pub(crate) fn def_home(ctx: &SessionContext, source: Option<&str>) -> String {
    match source.filter(|name| is_store_catalog(ctx, name)) {
        Some(name) => name.to_string(),
        None => CATALOG.to_string(),
    }
}

/// Every catalog on this session whose relations are def-backed: the workspace, then each live
/// store data source's, in the spelling each is registered under.
///
/// What [`registered`](super::catalog::registered) walks, so the pass's removal diff sees a
/// table wherever it was placed.
pub(crate) fn def_catalogs(ctx: &SessionContext) -> Vec<String> {
    let mut names = vec![CATALOG.to_string()];
    names.extend(
        ctx.catalog_names()
            .into_iter()
            .filter(|name| is_store_catalog(ctx, name)),
    );
    names
}

/// The reference that **resolves** the project's own bare `name` — a plain name in the workspace,
/// and the full three-part address when a live store data source's catalog holds it.
///
/// Every programmatic lookup goes through this rather than handing `ctx.table` a bare string:
/// bare resolution is `datafusion.catalog.default_catalog`'s, so a table placed in a store
/// catalog would simply not be found. SQL the user writes needs none of it — `sql::qualify`
/// already rewrites a bare name across catalogs — so this is the same rule for the callers that
/// never parse a statement.
///
/// A name nothing holds answers bare, which is the honest address for a table that is not
/// registered: the caller's own error is the one worth reading. A name that is **already**
/// qualified is parsed and handed back untouched — a database relation addresses itself, and
/// there is no project row to look for.
pub(crate) fn def_ref(ctx: &SessionContext, name: &str) -> TableReference {
    let parsed = TableReference::parse_str(name);
    let TableReference::Bare { table } = &parsed else {
        return parsed;
    };
    let name = table.as_ref();
    for catalog in def_catalogs(ctx).into_iter().skip(1) {
        let holds = ctx
            .catalog(&catalog)
            .and_then(|held| held.schema(SCHEMA))
            .is_some_and(|schema| schema.table_exist(name));
        if holds {
            return TableReference::full(catalog, SCHEMA, name.to_string());
        }
    }
    parsed
}

/// Whether `name` addresses a relation this project has a **def** for — the workspace's one
/// schema, or a store data source's catalog.
///
/// The **checkability** split, and deliberately not [`in_workspace`]: a view's recorded
/// dependencies are two lists because only one of them can be reconciled against the project's
/// rows, and a bucket table is a row. `in_workspace` answers a different question — whose
/// *namespace* a name is in, which is what reserves `__snap_` — and a store catalog is not the
/// workspace's namespace even though its tables are the project's.
pub(crate) fn def_backed(ctx: &SessionContext, name: &TableReference) -> bool {
    if in_workspace(name) {
        return true;
    }
    match name {
        TableReference::Full {
            catalog, schema, ..
        } => schema.as_ref() == SCHEMA && is_store_catalog(ctx, catalog),
        _ => false,
    }
}

/// A schema **Strata owns**: a fold-keyed table map with the one operation the trait lacks.
///
/// It serves two catalogs of the same shape — the workspace's one schema, holding its tables,
/// its views and the result spool, and each store data source's
/// ([`StoreCatalogProvider`]), holding the providers its table defs registered. Both are
/// flat, both are keyed by the project's own names, and neither enumerates anything: what is
/// in the map is what a registration put there.
///
/// Keyed by [`fold_ident`] — the same fold `TableReference::parse_str` applies on the way in,
/// so this changes no identity that ever worked; it is depth for the fact that the namespace
/// is case-insensitive, applied at the map rather than trusted to every caller. A `BTreeMap`
/// rather than DataFusion's `DashMap` because the only contention here is registration, and
/// sorted keys make `table_names()` — hence `SHOW TABLES` — deterministic for free.
#[derive(Debug, Default)]
pub struct StrataSchemaProvider {
    tables: RwLock<Tables>,
}

/// The map behind [`StrataSchemaProvider`], spelled once.
type Tables = BTreeMap<String, Arc<dyn TableProvider>>;

impl StrataSchemaProvider {
    fn read(&self) -> RwLockReadGuard<'_, Tables> {
        self.tables.read().unwrap()
    }

    fn write(&self) -> RwLockWriteGuard<'_, Tables> {
        self.tables.write().unwrap()
    }

    /// Puts `table` under `name`, replacing whatever was there, and returns what it displaced.
    ///
    /// The operation `SchemaProvider` does not have: `register_table` refuses a name that already
    /// exists, so a re-registration would otherwise have to deregister first and leave the name
    /// resolving to nothing while the new provider is built. The map's lock is held across this
    /// write, so a concurrent [`table`](Self::table) sees the old provider or the new one.
    pub fn replace(
        &self,
        name: &str,
        table: Arc<dyn TableProvider>,
    ) -> Option<Arc<dyn TableProvider>> {
        self.write().insert(fold_ident(name), table)
    }
}

/// Swaps `table` into the **workspace** catalog under `name` — [`StrataSchemaProvider::replace`]
/// reached through the session, which is all a caller holds.
///
/// The downcast is DataFusion's own pattern for a custom provider, as [`deregister_catalog`] is,
/// and cannot miss on an engine this crate built: [`build_context`](super::build_context)
/// installs [`StrataCatalogProvider`] before anything can replace it. Reported rather than
/// unwrapped, so a session assembled elsewhere gets a sentence instead of a panic.
///
/// # Errors
///
/// The workspace catalog is not this crate's.
pub(crate) fn replace_table(
    ctx: &SessionContext,
    catalog: &str,
    name: &str,
    table: Arc<dyn TableProvider>,
) -> Result<Option<Arc<dyn TableProvider>>, String> {
    with_schema(ctx, catalog, |schema| Ok(schema.replace(name, table)))
}

/// Takes `name` out of `catalog`'s one schema, answering what was there.
///
/// The counterpart to [`replace_table`], and reached for the same reason: `SessionContext`'s own
/// `deregister_table` resolves against the *default* catalog, so it cannot take a table out of
/// the store data source it was placed in.
pub(crate) fn remove_table(
    ctx: &SessionContext,
    catalog: &str,
    name: &str,
) -> Result<Option<Arc<dyn TableProvider>>, String> {
    with_schema(ctx, catalog, |schema| {
        schema.deregister_table(name).map_err(|e| e.to_string())
    })
}

/// Runs `f` against the [`StrataSchemaProvider`] behind `catalog`'s one schema — the workspace's,
/// or a store data source's.
///
/// A closure rather than a returned reference because the provider is reached through an `Arc`
/// the session hands out by value: borrowing out of that temporary is what the borrow checker
/// (rightly) refuses, and holding it for the call is all any caller needs.
///
/// The downcast is DataFusion's own pattern for a custom provider, as [`deregister_catalog`] is,
/// and cannot miss on a catalog this crate registered: [`build_context`](super::build_context)
/// installs the workspace's before anything can replace it, and a store data source's is
/// [`StoreCatalogProvider`]'s own. Reported rather than unwrapped, so a session assembled
/// elsewhere gets a sentence instead of a panic.
///
/// # Errors
///
/// There is no such catalog on this session, or its schema is not one of ours.
fn with_schema<T>(
    ctx: &SessionContext,
    catalog: &str,
    f: impl FnOnce(&StrataSchemaProvider) -> Result<T, String>,
) -> Result<T, String> {
    let schema = ctx
        .catalog(catalog)
        .and_then(|held| held.schema(SCHEMA))
        .ok_or_else(|| format!("This session has no '{catalog}.{SCHEMA}' to register into"))?;
    let owned: &StrataSchemaProvider = (schema.as_ref() as &dyn Any)
        .downcast_ref()
        .ok_or_else(|| format!("'{catalog}.{SCHEMA}' is not Strata's own schema"))?;
    f(owned)
}

#[async_trait]
impl SchemaProvider for StrataSchemaProvider {
    /// The catalog as everything that *enumerates* it sees it: user tables and views, never
    /// the result spool. Resolution is unaffected — see [`table`](Self::table).
    fn table_names(&self) -> Vec<String> {
        self.read()
            .keys()
            .filter(|name| !is_snapshot_name(name))
            .cloned()
            .collect()
    }

    /// Resolves **everything**, snapshots included: a hidden table is still a real one, and
    /// the paging, chart and export reads that name `__snap_N` go through here.
    async fn table(&self, name: &str) -> Result<Option<Arc<dyn TableProvider>>, DataFusionError> {
        Ok(self.read().get(&fold_ident(name)).cloned())
    }

    fn register_table(
        &self,
        name: String,
        table: Arc<dyn TableProvider>,
    ) -> Result<Option<Arc<dyn TableProvider>>> {
        if self.table_exist(name.as_str()) {
            return exec_err!("The table {name} already exists");
        }
        Ok(self.write().insert(fold_ident(&name), table))
    }

    fn deregister_table(&self, name: &str) -> Result<Option<Arc<dyn TableProvider>>> {
        Ok(self.write().remove(&fold_ident(name)))
    }

    fn table_exist(&self, name: &str) -> bool {
        self.read().contains_key(&fold_ident(name))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use datafusion::arrow::array::{Array, ArrayRef, Int32Array, StringArray};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;

    use super::*;
    use crate::builder::test_context;
    use crate::snapshots::snapshot_name;
    use crate::{Engine, RunTag, WsId, CATALOG};
    use strata_model::SnapshotId;

    /// A context shaped like a live engine's: one user table, one view, one registered
    /// result snapshot carrying the ordinal column every real snapshot has.
    async fn live_context() -> SessionContext {
        let ctx = test_context(&BTreeMap::new());
        let ids: ArrayRef = Arc::new(Int32Array::from(vec![1, 2]));
        let user = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)])),
            vec![Arc::clone(&ids)],
        )
        .expect("batch");
        let snapshot = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int32, false),
                Field::new("__strata_ord", DataType::Int32, false),
            ])),
            vec![ids, Arc::new(Int32Array::from(vec![0, 1]))],
        )
        .expect("batch");
        ctx.register_batch("events", user).expect("table");
        ctx.register_batch(snapshot_name(SnapshotId(3)).as_str(), snapshot)
            .expect("snapshot");
        ctx.sql("CREATE VIEW recent AS SELECT id FROM events")
            .await
            .expect("view");
        ctx
    }

    /// Run `sql` and read its `column` as strings — every introspection surface here answers
    /// in `Utf8` (`information_schema.rs:579`, `:759`).
    async fn column(ctx: &SessionContext, sql: &str, column: &str) -> Vec<String> {
        let batches = ctx
            .sql(sql)
            .await
            .expect("plan")
            .collect()
            .await
            .expect("run");
        let mut out = Vec::new();
        for batch in &batches {
            let idx = batch.schema().index_of(column).expect("column");
            let values = batch
                .column(idx)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("utf8 column");
            out.extend((0..values.len()).map(|i| values.value(i).to_string()));
        }
        out
    }

    /// The whole point of the provider: a snapshot is registered, resolvable, and invisible
    /// to every surface that enumerates the catalog — in one assertion set, because these are
    /// one rule (`table_names()` is the only enumeration path DataFusion has).
    #[tokio::test]
    async fn a_live_snapshot_resolves_by_name_and_appears_in_no_enumeration() {
        let ctx = live_context().await;
        let snap = snapshot_name(SnapshotId(3));

        assert!(
            ctx.table(snap.as_str()).await.is_ok(),
            "the paging, chart and export reads address a snapshot by name"
        );

        for sql in [
            "SHOW TABLES",
            "SELECT table_name FROM information_schema.tables",
            "SELECT table_name FROM information_schema.views",
            "SELECT table_name FROM information_schema.columns",
        ] {
            let names = column(&ctx, sql, "table_name").await;
            assert!(
                !names.contains(&snap),
                "'{sql}' listed the result snapshot: {names:?}"
            );
            assert!(
                names.contains(&"events".to_string()) && names.contains(&"recent".to_string()),
                "'{sql}' lost the user's own entries: {names:?}"
            );
        }

        let columns = column(
            &ctx,
            "SELECT column_name FROM information_schema.columns",
            "column_name",
        )
        .await;
        assert!(
            !columns.contains(&"__strata_ord".to_string()),
            "the snapshot's ordinal column leaked through information_schema.columns: {columns:?}"
        );
    }

    /// The same rule against a snapshot a **Run** minted, so the filter is not a property of
    /// the fixture: a real snapshot arrives through `register_listing_table` over a spooled
    /// Arrow file, and that is the path a user's `SHOW TABLES` has to survive.
    #[tokio::test]
    async fn the_snapshot_a_run_mints_is_hidden_and_still_readable() {
        let eng = Engine::builder().build();
        eng.ws(WsId(1))
            .query(RunTag(1), "SELECT 1 AS n".into(), 10)
            .await
            .expect("run");

        let names = column(&eng.ctx, "SHOW TABLES", "table_name").await;
        assert!(
            !names.iter().any(|name| is_snapshot_name(name)),
            "SHOW TABLES listed a live snapshot: {names:?}"
        );
        assert!(
            eng.ctx
                .table(snapshot_name(SnapshotId(1)).as_str())
                .await
                .is_ok(),
            "and the page reads still resolve it by name"
        );
    }

    /// The two introspection forms the router lets through, both working on a fresh project
    /// with no overrides. Only `SHOW TABLES` needed the default flip — it rewrites to
    /// `SELECT * FROM information_schema.tables` and errors outright when the key is off
    /// (`datafusion-sql-54.0.0/src/statement.rs:1627`). `DESCRIBE` never did:
    /// `describe_table_to_plan` (`:1638`) goes straight to `get_table_source`, so what this
    /// half pins is that it resolves through `StrataSchemaProvider`, not the flag.
    #[tokio::test]
    async fn introspection_works_on_a_fresh_context() {
        let ctx = live_context().await;
        assert!(column(&ctx, "SHOW TABLES", "table_name")
            .await
            .contains(&"recent".to_string()));
        assert!(ctx.sql("DESCRIBE events").await.is_ok());
    }

    /// **A store data source's catalog is placement, not a namespace** (EA-25 item 3).
    ///
    /// The five decisions the placement rests on, asked of a registered `StoreCatalogProvider`
    /// with no data source and no network behind it:
    ///
    /// * a def naming a live store source homes there, and one naming nothing homes in the
    ///   workspace — including a def whose source never connected, which is the honest fallback;
    /// * a bare project name **resolves** to the full address, so every programmatic lookup
    ///   finds a table wherever it was put;
    /// * the name is `def_backed`, which is what keeps a view over it checkable, while
    ///   `in_workspace` still answers false — the `__snap_` namespace is not widened;
    /// * deregistering the catalog takes its tables with it, which is what makes a Forget
    ///   structural.
    #[tokio::test]
    async fn a_store_catalog_holds_the_defs_that_read_through_it() {
        let ctx = test_context(&BTreeMap::new());
        ctx.register_catalog("lake", Arc::new(StoreCatalogProvider::new("lake".into())));

        assert_eq!(
            def_home(&ctx, Some("lake")),
            "lake",
            "a def homes in its source"
        );
        assert_eq!(
            def_home(&ctx, None),
            CATALOG,
            "a local def homes in the workspace"
        );
        assert_eq!(
            def_home(&ctx, Some("never_connected")),
            CATALOG,
            "and so does one whose data source is not live"
        );

        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let table = Arc::new(MemTable::try_new(Arc::clone(&schema), vec![vec![]]).unwrap());
        replace_table(&ctx, "lake", "regions", table).expect("the store schema takes it");

        assert_eq!(
            def_ref(&ctx, "regions"),
            TableReference::full("lake", SCHEMA, "regions"),
            "a bare project name resolves to where the def was placed"
        );
        assert_eq!(
            def_ref(&ctx, "absent"),
            TableReference::bare("absent"),
            "and a name nothing holds stays bare, so the caller's own error is the one read"
        );

        let at = TableReference::full("lake", SCHEMA, "regions");
        assert!(
            def_backed(&ctx, &at),
            "a bucket table is one of the project's rows"
        );
        assert!(
            !in_workspace(&at),
            "but it is not in the workspace's namespace, which is what reserves '__snap_'"
        );

        assert!(ctx.table(at.clone()).await.is_ok(), "and it resolves");
        deregister_catalog(&ctx, "lake").expect("the catalog was registered");
        assert!(
            ctx.table(at).await.is_err(),
            "forgetting the data source takes its tables with it, rather than one deregistration \
             per table that a failure could half-finish"
        );
    }

    /// A user's own `datafusion.catalog.information_schema = false` still wins: Strata's
    /// default is a default, not an owned key.
    #[tokio::test]
    async fn the_information_schema_default_is_overridable() {
        let off = BTreeMap::from([(
            "datafusion.catalog.information_schema".to_string(),
            "false".to_string(),
        )]);
        assert!(test_context(&off).sql("SHOW TABLES").await.is_err());
    }

    /// Structural, not policy: the router refuses `CREATE SCHEMA` first, and this is what
    /// answers when something reaches `ctx.sql` without asking it.
    #[tokio::test]
    async fn a_second_schema_cannot_be_created() {
        let ctx = test_context(&BTreeMap::new());
        let err = ctx
            .sql("CREATE SCHEMA extra")
            .await
            .expect_err("the provider refuses")
            .to_string();
        assert!(err.contains("one schema"), "unexpected refusal: {err}");
        assert_eq!(
            ctx.catalog(CATALOG).expect("our catalog").schema_names(),
            vec![SCHEMA.to_string()]
        );
    }

    /// A swap has no gap in it — the name resolves to the old provider, then to the new one, and
    /// to nothing in between — which is the property the registration path leans on.
    #[tokio::test]
    async fn replacing_a_table_never_leaves_the_name_unresolved() {
        let ctx = test_context(&BTreeMap::new());
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(Int32Array::from(vec![1]))]).expect("batch");
        ctx.register_batch("Events", batch).expect("table");

        let wider = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("label", DataType::Utf8, true),
        ]));
        let replacement = Arc::new(MemTable::try_new(Arc::clone(&wider), vec![vec![]]).unwrap());

        let displaced =
            replace_table(&ctx, CATALOG, "events", replacement).expect("the workspace schema");

        assert!(
            displaced.is_some(),
            "the swap answered with what it took out"
        );
        assert_eq!(
            ctx.table("events").await.expect("still resolves").schema().fields().len(),
            2,
            "and the name resolves to the new provider — keyed case-insensitively, so 'Events'              was replaced rather than shadowed"
        );
        assert_eq!(
            ctx.catalog(CATALOG)
                .and_then(|c| c.schema(SCHEMA))
                .expect("the workspace schema")
                .table_names(),
            vec!["events".to_string()],
            "one entry, not two"
        );
    }

    /// The namespace is case-insensitive at the map, not only at the callers that happen to
    /// fold before asking.
    #[tokio::test]
    async fn registration_is_keyed_case_insensitively() {
        let ctx = test_context(&BTreeMap::new());
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(Int32Array::from(vec![1]))]).expect("batch");
        ctx.register_batch("Foo", batch).expect("table");

        assert!(ctx.table("foo").await.is_ok(), "'Foo' resolves as 'foo'");
        assert!(ctx.sql("SELECT id FROM \"FOO\"").await.is_ok());
    }
}
