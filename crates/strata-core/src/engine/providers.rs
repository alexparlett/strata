//! The catalog and schema **Strata owns** — identity and visibility, never lifecycle.
//!
//! DataFusion 54's provider traits are resolution and enumeration interfaces:
//! `register_table` is sync and carries no caller identity, so nothing here could spool a
//! CTAS result or authorize a `DROP` (`docs/STATEMENTS_SPEC.md` §3 — settled; the lifecycle
//! lives in [`ddl`](super::ddl), in front of `ctx.sql`). What the traits *can* do is fix the
//! shape of the namespace and decide what enumerating it returns, which is exactly the two
//! jobs this module has:
//!
//! * **Identity** — one catalog, one schema, tables keyed by [`fold_ident`].
//!   [`StrataCatalogProvider::register_schema`] refuses, so `CREATE SCHEMA x` fails at the
//!   provider even when a statement reaches `ctx.sql` with the router bypassed. (`CREATE
//!   DATABASE` cannot be stopped here: DataFusion's `create_catalog` registers into the
//!   `CatalogProviderList`, whose `register_catalog` returns an `Option` and so has no way to
//!   refuse — `context/mod.rs:1030-1050`. The router's `Blocked::CreateDatabase` is the only
//!   gate that can say no about it, and it is the first line for `CREATE SCHEMA` too.)
//! * **Visibility** — [`StrataSchemaProvider::table_names`] drops the `__snap_`-prefixed
//!   result snapshots while `table()` still resolves them. Every `information_schema` view
//!   and every `SHOW` form enumerates through `table_names()`
//!   (`datafusion-catalog-54.0.0/src/information_schema.rs:96-216`), so one filter hides the
//!   spool from all of them — and `__strata_ord`, a column only a snapshot carries, never
//!   reaches `information_schema.columns`. Paging, chart, export and retirement all address a
//!   snapshot **by name**, so none of them notices.
//!
//! The prefix itself is [`is_snapshot_name`], next to the function that mints the names: the
//! hiding rule and the naming rule are one definition, and cannot drift apart.
//!
//! Everything else delegates to the map verbatim, `MemorySchemaProvider`'s semantics included
//! — the duplicate-name error and all — so every existing reader, `find_and_deregister`,
//! validation's `table_exist` and snapshot retirement keep working with no call-site changes.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use async_trait::async_trait;
use datafusion::catalog::{CatalogProvider, SchemaProvider, TableProvider};
use datafusion::common::{exec_err, DataFusionError, Result};

use super::query::is_snapshot_name;
use super::{fold_ident, SCHEMA};

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
