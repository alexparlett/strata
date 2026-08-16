//! **The federated table provider, built one level down so a rewrite can ride it** (DB-08).
//!
//! [`table_provider`] is `PostgresTableFactory::table_provider` with its three steps written out —
//! the `SqlTable`, the Postgres unparser dialect, the federation wrapper — because
//! `datafusion-table-providers` leaves every one of `datafusion-federation`'s rewrite hooks at its
//! `None` default (`datafusion-federation#129` is the open issue asking for exactly this pattern),
//! and the hooks are on the **executor** the federation provider is built over. Nothing about the
//! provider changes: same dialect, same wrapper, same lazily-built-and-cached provider per
//! relation, and [`DbSchemaProvider`](super::DbSchemaProvider) remains the one construction site.
//!
//! What is added is [`PgExecutor`], which is the crate's own executor plus the two things only
//! Strata can supply: the [`json`] rewrite in front of the statement that leaves, and the
//! connection's name in front of the error that comes back.

use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::TableProvider;
use datafusion::common::{DataFusionError, Result as DfResult, Statistics};
use datafusion::logical_expr::LogicalPlan;
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{PhysicalExpr, SendableRecordBatchStream};
use datafusion::sql::unparser::dialect::{Dialect, PostgreSqlDialect};
use datafusion::sql::TableReference;
use datafusion_federation::sql::{
    AstAnalyzer, LogicalOptimizer, RemoteTableRef, SQLExecutor, SQLFederationProvider,
    SQLTableSource,
};
use datafusion_federation::FederatedTableProviderAdaptor;
use datafusion_table_providers_common::sql::sql_provider_datafusion::SqlTable;
use datafusion_table_providers_postgres::pool::PostgresConnectionPool;
use datafusion_table_providers_postgres::DynPostgresConnectionPool;
use futures::TryStreamExt;

use super::json;

/// A federated provider for one remote relation, reading through `pool` and answering for the
/// connection registered as `connection`.
pub(super) async fn table_provider(
    pool: &Arc<PostgresConnectionPool>,
    connection: &str,
    relation: TableReference,
) -> Result<Arc<dyn TableProvider>, Box<dyn std::error::Error + Send + Sync>> {
    let cloned = Arc::clone(pool);
    let pool: Arc<DynPostgresConnectionPool> = cloned;
    let table = Arc::new(
        SqlTable::new("postgres", &pool, relation)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
            .with_dialect(Arc::new(PostgreSqlDialect {})),
    );
    let executor = Arc::new(PgExecutor {
        inner: Arc::clone(&table) as Arc<dyn SQLExecutor>,
        connection: connection.to_string(),
    });
    let source = Arc::new(SQLTableSource::new_with_schema(
        Arc::new(SQLFederationProvider::new(executor)),
        RemoteTableRef::from(table.table_reference.clone()),
        table.schema(),
    ));
    Ok(Arc::new(FederatedTableProviderAdaptor::new_with_provider(
        source, table,
    )))
}

/// The crate's executor, with the connection's name kept beside it.
///
/// Held as `Arc<dyn SQLExecutor>` rather than the concrete `SqlTable`, whose generic parameters are
/// the pooled connection and the driver's parameter type: nothing here needs to name them, and a
/// signature that did would tie this module to `tokio-postgres`.
///
/// Every method delegates. The two that do not are the point: an [`ast_analyzer`](Self) that
/// rewrites the statement about to leave, and an [`execute`](Self) that names the connection in the
/// one failure the server can report that the user cannot act on as written.
struct PgExecutor {
    inner: Arc<dyn SQLExecutor>,
    connection: String,
}

#[async_trait]
impl SQLExecutor for PgExecutor {
    fn name(&self) -> &str {
        self.inner.name()
    }

    /// **Delegated, and load-bearing.** This is what decides whether two relations federate into
    /// one statement, so the wrapper has to answer exactly as the pool does.
    fn compute_context(&self) -> Option<String> {
        self.inner.compute_context()
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        self.inner.dialect()
    }

    /// **Delegated even though the crate answers `None` today.** It is the third rewrite hook, and
    /// a wrapper that answers for the executor has to answer for all of them: taking the trait's
    /// default here would silently drop a logical rewrite a later `datafusion-table-providers`
    /// adds, with nothing to fail.
    ///
    /// Note what it is *not* good for: the plan it receives is already wrapped in the federation
    /// crate's own extension node, so an optimizer rule run here sees an opaque root and rewrites
    /// nothing. A plan that has to be simplified before it can be unparsed must be simplified
    /// before the federation analyzer runs — see [`db::write::append`](super::write).
    fn logical_optimizer(&self) -> Option<LogicalOptimizer> {
        self.inner.logical_optimizer()
    }

    fn ast_analyzer(&self) -> Option<AstAnalyzer> {
        let connection = self.connection.clone();
        Some(Box::new(move |statement| {
            json::push_down(statement, &connection)
        }))
    }

    fn execute(
        &self,
        query: &str,
        schema: SchemaRef,
        filters: &[Arc<dyn PhysicalExpr>],
    ) -> DfResult<SendableRecordBatchStream> {
        let connection = self.connection.clone();
        let stream = self.inner.execute(query, Arc::clone(&schema), filters)?;
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            schema,
            stream.map_err(move |e| {
                let raw = e.to_string();
                match json::lacks_the_name(&raw) {
                    true => DataFusionError::Execution(json::remote_refusal(&raw, &connection)),
                    false => e,
                }
            }),
        )))
    }

    async fn statistics(&self, plan: &LogicalPlan) -> DfResult<Statistics> {
        self.inner.statistics(plan).await
    }

    async fn table_names(&self) -> DfResult<Vec<String>> {
        self.inner.table_names().await
    }

    async fn get_table_schema(&self, table_name: &str) -> DfResult<SchemaRef> {
        self.inner.get_table_schema(table_name).await
    }

    fn metrics(&self) -> Option<MetricsSet> {
        self.inner.metrics()
    }
}
