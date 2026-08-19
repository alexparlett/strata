//! Reading a SQL-speaking source: the federation stack, assembled for a backend that composes it.
//!
//! A backend builds its `SqlTable`, describes it in a [`SqlSpec`] and hands that to [`federated`];
//! what comes back is a provider whose scans leave as one statement in the source's own SQL. A
//! backend whose source speaks something else never names this module, and
//! [`SourceCatalog`](super::source::SourceCatalog) demands nothing of it.
//!
//! The stack is assembled a level below `datafusion-table-providers`' own factory, which leaves
//! every one of `datafusion-federation`'s rewrite hooks at its `None` default
//! (`datafusion-federation#129` asks for exactly this pattern). Those hooks are on the executor,
//! so [`AnalyzedExecutor`] is where the source's AST rewrite reaches the statement going out, its
//! recoding reaches the error coming back, and the connection's identity is stamped as the fusion
//! key — stamped here, because a source that forgot it would federate two connections' relations
//! into one statement sent to whichever executor won.

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
use datafusion::sql::unparser::dialect::Dialect;
use datafusion_federation::sql::{
    AstAnalyzer, LogicalOptimizer, RemoteTableRef, SQLExecutor, SQLFederationProvider,
    SQLTableSource,
};
use datafusion_federation::{default_optimizer_rules, FederatedTableProviderAdaptor};
use futures::TryStreamExt;

use crate::sources::source::{Located, SourceCatalog};

/// The name `datafusion-federation` gives its rule, which is how it is found in the list.
const FEDERATION_RULE: &str = "federation_optimizer_rule";

/// DataFusion's optimizer rules with federation among them — the crate's own list, with its
/// federation rule wrapped so a **write** node is never federated whole.
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
/// A `CopyTo` or a `Dml` at the root of a plan whose every scan is one connection's is federated
/// whole by the crate, and then unparsed, and `plan_to_sql` has no arm for a write. What comes
/// back is several hundred characters of `LogicalPlan` debug where the rows should be.
///
/// The crate already draws this line and stops two nodes short of it: `LogicalPlan::Analyze` is
/// exempted in the same recursion for the same reason, with "cannot be converted to SQL by the
/// Unparser" written beside it. This adds the nodes the exemption is missing, from outside, and is
/// the whole of `UPSTREAM_REPORTS.md`'s `datafusion-federation` entry.
///
/// **`Dml` is named even though the `INSERT` arms reach no optimizer**, driving the target's own
/// sink over the DML's input instead ([`sink::append_rows`](crate::sink::append_rows)). The
/// predicate is "a node that writes", and leaving one of the two out would make it a rule that
/// happens to hold rather than one that does.
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

/// Builds a backend's AST rewrite.
///
/// A factory rather than the rewrite itself: `AstAnalyzer` is a `FnMut` taken by value every time
/// a plan is unparsed, so there is one per statement rather than one per provider.
pub type AstRewrite = Arc<dyn Fn() -> AstAnalyzer + Send + Sync>;

/// What a SQL-speaking source brings to [`federated`].
///
/// The fields come from one object in practice, a `SqlTable` being both provider and executor,
/// and are named separately because nothing here requires that.
pub struct SqlSpec {
    /// The unparser dialect the plan is rendered in: the source's SQL, not DataFusion's.
    pub dialect: Arc<dyn Dialect>,
    /// What sends the rendered statement and streams back what it answers.
    pub executor: Arc<dyn SQLExecutor>,
    /// The provider a scan the federation rule does not take falls back to, which a mixed join's
    /// local side reads through.
    pub provider: Arc<dyn TableProvider>,
    /// The source's own rewrite of the statement about to leave, where it has one.
    ///
    /// What a dialect override cannot express: a UDF call unparses by name, and a source that
    /// spells the same operation as an operator expression needs the shape changed, not the word.
    pub analyzer: Option<AstRewrite>,
}

/// Returns the federated read provider for one relation.
///
/// The whole body of a SQL-speaking source's
/// [`table_provider`](super::source::SourceCatalog::table_provider).
pub fn federated(
    source: Arc<dyn SourceCatalog>,
    spec: SqlSpec,
    at: &Located,
) -> Arc<dyn TableProvider> {
    let executor = Arc::new(AnalyzedExecutor {
        inner: spec.executor,
        dialect: spec.dialect,
        analyzer: spec.analyzer,
        source,
        connection: at.connection.clone(),
        context: at.url.clone(),
    });
    let source = Arc::new(SQLTableSource::new_with_schema(
        Arc::new(SQLFederationProvider::new(executor)),
        RemoteTableRef::from(at.relation.clone()),
        spec.provider.schema(),
    ));
    Arc::new(FederatedTableProviderAdaptor::new_with_provider(
        source,
        spec.provider,
    ))
}

/// The source's executor, with the things the assembly owes every source that composes it.
///
/// Held as `Arc<dyn SQLExecutor>` rather than a concrete provider type, whose generic parameters
/// are a pooled connection and a driver's parameter type: a signature naming them would tie this
/// module to one driver. Every method delegates but the four that are the point.
struct AnalyzedExecutor {
    inner: Arc<dyn SQLExecutor>,
    dialect: Arc<dyn Dialect>,
    analyzer: Option<AstRewrite>,
    source: Arc<dyn SourceCatalog>,
    /// The catalog the connection registered under — what a refusal names.
    connection: String,
    /// [`Located::url`], as the fusion key.
    context: String,
}

#[async_trait]
impl SQLExecutor for AnalyzedExecutor {
    fn name(&self) -> &str {
        self.inner.name()
    }

    /// The connection's identity, which is what decides whether two relations federate into one
    /// statement. Two connections to one server may authenticate as different roles and see
    /// different relations, so fusing across them sends a statement to whichever executor won.
    fn compute_context(&self) -> Option<String> {
        Some(self.context.clone())
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        Arc::clone(&self.dialect)
    }

    /// Delegated, because a wrapper that answers for an executor has to answer for all three
    /// rewrite hooks: taking the trait's default would silently drop a rewrite the wrapped
    /// executor supplies.
    ///
    /// Note what it is *not* good for: the plan it receives is already wrapped in the federation
    /// crate's own extension node, so an optimizer rule run here sees an opaque root and rewrites
    /// nothing. A plan that has to be simplified before it can be unparsed must be simplified
    /// before the federation rule runs — see [`sink::append_rows`](crate::sink::append_rows).
    fn logical_optimizer(&self) -> Option<LogicalOptimizer> {
        self.inner.logical_optimizer()
    }

    fn ast_analyzer(&self) -> Option<AstAnalyzer> {
        self.analyzer.as_ref().map(|rewrite| rewrite())
    }

    fn execute(
        &self,
        query: &str,
        schema: SchemaRef,
        filters: &[Arc<dyn PhysicalExpr>],
    ) -> DfResult<SendableRecordBatchStream> {
        let source = Arc::clone(&self.source);
        let connection = self.connection.clone();
        let stream = self.inner.execute(query, Arc::clone(&schema), filters)?;
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            schema,
            stream.map_err(
                move |e| match source.remote_refusal(&e.to_string(), &connection) {
                    Some(reworded) => DataFusionError::Execution(reworded),
                    None => e,
                },
            ),
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
