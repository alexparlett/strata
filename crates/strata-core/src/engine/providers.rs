//! The catalog list, catalog and schema **Strata owns** — identity and visibility, never
//! lifecycle.
//!
//! DataFusion 54's provider traits are resolution and enumeration interfaces:
//! `register_table` is sync and carries no caller identity, so nothing here could spool a
//! CTAS result or authorize a `DROP` (`docs/STATEMENTS_SPEC.md` §3 — settled; the lifecycle
//! lives in [`ddl`](super::ddl), in front of `ctx.sql`). What the traits *can* do is fix the
//! shape of the namespace and decide what enumerating it returns, which is exactly the two
//! jobs this module has:
//!
//! * **Identity** — **the workspace catalog** has one schema, with tables keyed by
//!   [`fold_ident`]. [`StrataCatalogProvider::register_schema`] refuses, so `CREATE SCHEMA x`
//!   fails at the provider even when a statement reaches `ctx.sql` with the router bypassed.
//!   (`CREATE DATABASE` cannot be stopped here: DataFusion's `create_catalog` registers into
//!   the [`CatalogProviderList`], whose `register_catalog` returns an `Option` and so has no
//!   way to refuse — `context/mod.rs:1030-1050`, and [`StrataCatalogList`] is no different.
//!   The router's `Blocked::CreateDatabase` is the only gate that can say no about it, and it
//!   is the first line for `CREATE SCHEMA` too.)
//!
//!   That scoping is load-bearing since the DB workstream: the session holds **N** catalogs —
//!   the workspace's plus one per database connection — and a remote catalog has as many
//!   schemas as the server does. What is one-catalog-one-schema is the *workspace*, whose
//!   flat, bare-name namespace is the deepest assumption in the app.
//! * **Removability** — [`StrataCatalogList`], which exists for one reason DataFusion cannot
//!   serve: `CatalogProviderList` has `register_catalog` and no counterpart, and
//!   `MemoryCatalogProviderList` is an insert-only map. Forgetting a database connection has
//!   to make its catalog stop resolving, or a removed source stays silently queryable until
//!   the window is re-opened — the exact inverse of the catalog-is-the-store rule.
//! * **Visibility** — [`StrataSchemaProvider::table_names`] drops the `__snap_`-prefixed
//!   result snapshots while `table()` still resolves them. Every `information_schema` view
//!   and every `SHOW` form enumerates through `table_names()`
//!   (`datafusion-catalog-54.0.0/src/information_schema.rs:96-216`), so one filter hides the
//!   spool from all of them — and `__strata_ord`, a column only a snapshot carries, never
//!   reaches `information_schema.columns`. Paging, chart, export and retirement all address a
//!   snapshot **by name**, so none of them notices.
//!
//! The prefix itself is [`is_snapshot_name`], next to the function that mints the names: the
//! hiding rule and the naming rule are one definition, and cannot drift apart. The filter is
//! **this** provider's and a database connection's is deliberately without one
//! ([`db::DbSchemaProvider`](super::db)): the namespace is the workspace catalog's, so a remote
//! relation a server happens to call `__snap_x` is an ordinary table that means nothing here —
//! which is the same scoping [`is_snapshot_ref`](super::query::is_snapshot_ref) applies to the
//! refusal, off [`in_workspace`].
//!
//! Everything else delegates to the map verbatim, `MemorySchemaProvider`'s semantics included
//! — the duplicate-name error and all — so every existing reader, `find_and_deregister`,
//! validation's `table_exist` and snapshot retirement keep working with no call-site changes.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use async_trait::async_trait;
use datafusion::catalog::{CatalogProvider, CatalogProviderList, SchemaProvider, TableProvider};
use datafusion::common::{exec_err, DataFusionError, Result};
use datafusion::prelude::SessionContext;
use datafusion::sql::TableReference;

use super::query::is_snapshot_name;
use super::{fold_ident, CATALOG, SCHEMA};

/// Whether `name` addresses **the workspace catalog's one schema** — the three spellings of
/// one place (`orders`, `public.orders`, `strata.public.orders`), and nothing else.
///
/// One predicate rather than the test written out per caller, because two rules turn on it and
/// they must not drift: what an intercepted statement may create, drop or write
/// ([`ddl::bare_name`](super::ddl::bare_name)), and what the `__snap_` namespace covers
/// ([`is_snapshot_ref`](super::query::is_snapshot_ref)). Since the DB workstream the session
/// holds more than one catalog, so "is this name ours" is a real question rather than a
/// formality — a database connection's catalog has as many schemas as the server does, and a
/// relation in one is neither Strata's to manage nor part of Strata's reserved namespace.
///
/// Reference-shaped, so it is asked of the same value DataFusion resolved: a `Partial` whose
/// schema is not `public` names a schema the workspace catalog cannot have
/// (`StrataCatalogProvider::schema`), which is why it answers false rather than looking at the
/// catalog list.
///
/// **Each part is compared the way the thing that resolves it compares**, and the two halves
/// differ on purpose. [`StrataCatalogList`] keys catalogs by [`fold_ident`], so `"STRATA"` —
/// quoted, and therefore carried verbatim past the parser's own folding — resolves to the
/// workspace catalog and has to answer true here; comparing it raw let that spelling out of the
/// workspace, and with it out of the `__snap_` namespace, which is a way to read another tab's
/// snapshot. [`StrataCatalogProvider::schema`] compares its one schema **exactly**, so a
/// `"PUBLIC"` resolves to nothing at all and answering false about it is the honest answer.
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
/// connection could be forgotten and go on answering `pg.public.orders` for the life of the
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
    /// connection deliberately registers the user's own spelling.
    catalogs: RwLock<BTreeMap<String, Registered>>,
}

/// One entry: the name it was registered under, and the provider. See [`StrataCatalogList`].
type Registered = (String, Arc<dyn CatalogProvider>);

impl StrataCatalogList {
    /// Take the catalog registered under `name` back out — the half `CatalogProviderList` is
    /// missing. `None` when nothing was registered under it, which is the ordinary case for a
    /// connection that never connected and is not a fault.
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

/// Register a catalog shaped the way a **database connection's** is — one schema, some
/// relations — so a test can ask what the app does about a name inside one without a server.
///
/// A `MemoryCatalogProvider` stands in exactly, because every rule under test reads the
/// **catalog list** and nothing more: `ddl::bare_name`'s refusal, `catalog::view_error`'s
/// diagnosis, `plan_deps`' qualified recording and `Engine::describe_remote` all ask whether a
/// catalog of that name is registered and then work off the resolved reference. What
/// `db::DbCatalogProvider` adds on top — lazily built federated providers and a `table_type`
/// that costs no round trip — is what the *integration* test exercises against a real server
/// (`tests/postgres_federation.rs`), and nothing here can stand in for that.
///
/// Two columns rather than one, so a test can join a remote relation to a workspace table on
/// `id` and still have something to project.
#[cfg(test)]
pub(crate) fn fake_database(ctx: &SessionContext, catalog: &str, relations: &[&str]) {
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::catalog::{MemoryCatalogProvider, MemorySchemaProvider};
    use datafusion::datasource::empty::EmptyTable;

    let schema = Arc::new(MemorySchemaProvider::new());
    for relation in relations {
        let arrow = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("total", DataType::Int64, true),
        ]));
        schema
            .register_table((*relation).to_string(), Arc::new(EmptyTable::new(arrow)))
            .expect("fake relation");
    }
    let provider = Arc::new(MemoryCatalogProvider::new());
    provider
        .register_schema(SCHEMA, schema)
        .expect("fake schema");
    ctx.register_catalog(catalog, provider);
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

/// The one schema every Strata table, view and result snapshot lives in.
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
        // `MemorySchemaProvider`'s answer, kept: every Strata registration deregisters first
        // (`catalog::register_external`, snapshot retirement), so a collision here is a bug
        // and must not read as a silent replacement.
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
    use datafusion::prelude::SessionContext;

    use super::*;
    use crate::engine::query::snapshot_name;
    use crate::engine::{build_context, Engine, RunTag, WsId, CATALOG};
    use strata_model::SnapshotId;

    /// A context shaped like a live engine's: one user table, one view, one registered
    /// result snapshot carrying the ordinal column every real snapshot has.
    async fn live_context() -> SessionContext {
        let ctx = build_context(&BTreeMap::new());
        let ids: ArrayRef = Arc::new(Int32Array::from(vec![1, 2]));
        let user = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)])),
            vec![Arc::clone(&ids)],
        )
        .expect("batch");
        // A snapshot carries the ordinal column, and only a snapshot does — which is what
        // makes `__strata_ord` the tell that the filter is working.
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
        let eng = Engine::new(BTreeMap::new());
        eng.query(WsId(1), RunTag(1), "SELECT 1 AS n".into(), 10)
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

    /// A user's own `datafusion.catalog.information_schema = false` still wins: Strata's
    /// default is a default, not an owned key.
    #[tokio::test]
    async fn the_information_schema_default_is_overridable() {
        let off = BTreeMap::from([(
            "datafusion.catalog.information_schema".to_string(),
            "false".to_string(),
        )]);
        assert!(build_context(&off).sql("SHOW TABLES").await.is_err());
    }

    /// Structural, not policy: the router refuses `CREATE SCHEMA` first, and this is what
    /// answers when something reaches `ctx.sql` without asking it.
    #[tokio::test]
    async fn a_second_schema_cannot_be_created() {
        let ctx = build_context(&BTreeMap::new());
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

    /// The namespace is case-insensitive at the map, not only at the callers that happen to
    /// fold before asking.
    #[tokio::test]
    async fn registration_is_keyed_case_insensitively() {
        let ctx = build_context(&BTreeMap::new());
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(Int32Array::from(vec![1]))]).expect("batch");
        ctx.register_batch("Foo", batch).expect("table");

        assert!(ctx.table("foo").await.is_ok(), "'Foo' resolves as 'foo'");
        assert!(ctx.sql("SELECT id FROM \"FOO\"").await.is_ok());
    }
}
