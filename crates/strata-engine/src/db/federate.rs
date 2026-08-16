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
//!
//! And [`optimizer_rules`], which is the crate's rule list with the exemption it is missing.

use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::TableProvider;
use datafusion::common::tree_node::{Transformed, TreeNode};
use datafusion::common::{DataFusionError, Result as DfResult, Statistics};
use datafusion::logical_expr::LogicalPlan;
use datafusion::optimizer::{ApplyOrder, OptimizerConfig, OptimizerRule};
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{PhysicalExpr, SendableRecordBatchStream};
use datafusion::sql::unparser::dialect::{Dialect, PostgreSqlDialect};
use datafusion::sql::TableReference;
use datafusion_federation::sql::{
    AstAnalyzer, LogicalOptimizer, RemoteTableRef, SQLExecutor, SQLFederationProvider,
    SQLTableSource,
};
use datafusion_federation::{default_optimizer_rules, FederatedTableProviderAdaptor};
use datafusion_table_providers_common::sql::sql_provider_datafusion::SqlTable;
use datafusion_table_providers_postgres::pool::PostgresConnectionPool;
use datafusion_table_providers_postgres::DynPostgresConnectionPool;
use futures::TryStreamExt;

use super::json;

/// The name `datafusion-federation` gives its rule, which is how it is found in the list.
const FEDERATION_RULE: &str = "federation_optimizer_rule";

/// DataFusion's optimizer rules with federation among them — the crate's own list, with its
/// federation rule wrapped so a **write** node is never federated whole (DB-12).
///
/// **Panics if the rule is not in the list**, because the alternative is worse: an unwrapped list
/// is a working engine that has quietly lost the exemption, and what it loses is a CTAS or a
/// `COPY` over a database connection answering with a page of `LogicalPlan` debug. The crate takes
/// the same position one level down — `default_optimizer_rules` panics rather than return a list
/// it could not insert federation into. `the_federation_rule_is_still_named_what_we_look_for`
/// is what makes a dependency bump fail in CI rather than here.
pub(crate) fn optimizer_rules() -> Vec<Arc<dyn OptimizerRule + Send + Sync>> {
    let rules: Vec<Arc<dyn OptimizerRule + Send + Sync>> = default_optimizer_rules()
        .into_iter()
        .map(|rule| match rule.name() == FEDERATION_RULE {
            true => Arc::new(WritesStayHome(rule)) as Arc<dyn OptimizerRule + Send + Sync>,
            false => rule,
        })
        .collect();
    assert!(
        rules.iter().any(|rule| rule.name() == FEDERATION_RULE),
        "datafusion-federation no longer contributes a rule called '{FEDERATION_RULE}'"
    );
    rules
}

/// The federation rule, kept off a node that **writes**: its input is federated and the node is
/// rebuilt around the result.
///
/// A `CopyTo` or a `Dml` at the root of a plan whose every scan is one connection's — a CTAS
/// spooling a remote query into an internal table, a typed `COPY … TO` reading one, an `INSERT`
/// from one — is federated whole by the crate, and then unparsed, and `plan_to_sql` has no arm for
/// a write. What comes back is several hundred characters of `LogicalPlan` debug where the rows
/// should be.
///
/// The crate already draws this line and stops two nodes short of it: `LogicalPlan::Analyze` is
/// exempted in the same recursion for the same reason, with "cannot be converted to SQL by the
/// Unparser" written beside it. This adds the nodes the exemption is missing, from outside, and is
/// the whole of `UPSTREAM_REPORTS.md`'s `datafusion-federation` entry.
///
/// **`Dml` is named even though no `Dml` reaches the optimizer today** — both `INSERT` arms drive
/// the target's own sink over the DML's input ([`sink::append_rows`](crate::sink::append_rows)),
/// which is the better shape for its own reasons. The predicate here is "a node that writes", and
/// leaving one of the two out would make it a rule that happens to hold rather than one that does:
/// whatever plans a `Dml` next would meet DB-12 again, as a page of debug text.
#[derive(Debug)]
struct WritesStayHome(Arc<dyn OptimizerRule + Send + Sync>);

impl OptimizerRule for WritesStayHome {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn supports_rewrite(&self) -> bool {
        true
    }

    /// Delegated, so the wrapper is transparent about how it wants to be driven: `None` means the
    /// rule walks the plan itself and is handed the root once, which is what makes matching on
    /// `plan` here the same test the crate makes on its own root.
    fn apply_order(&self) -> Option<ApplyOrder> {
        self.0.apply_order()
    }

    fn rewrite(
        &self,
        plan: LogicalPlan,
        config: &dyn OptimizerConfig,
    ) -> DfResult<Transformed<LogicalPlan>> {
        if !matches!(plan, LogicalPlan::Copy(_) | LogicalPlan::Dml(_)) {
            return self.0.rewrite(plan, config);
        }
        plan.map_children(|input| self.0.rewrite(input, config))
    }
}

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
    /// before the federation rule runs — see [`sink::append_rows`](crate::sink::append_rows).
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

/// **The one thing [`optimizer_rules`] cannot check for itself at a useful moment.**
///
/// The wrap is found by name, and a name is a dependency's to change. Unwrapped, the engine still
/// builds and every query still runs — what stops working is a CTAS or a `COPY` over a database
/// connection, which only `tests/postgres_federation.rs` exercises, twelve minutes and a container
/// runtime away. This fails in the ordinary suite instead, naming what moved.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_federation_rule_is_still_named_what_we_look_for() {
        assert_eq!(
            default_optimizer_rules()
                .iter()
                .filter(|rule| rule.name() == FEDERATION_RULE)
                .count(),
            1,
            "datafusion-federation contributes exactly one rule under this name, and \
             optimizer_rules wraps it by that name"
        );
    }
}
