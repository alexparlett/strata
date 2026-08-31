//! **The source's own executor: it names the projected columns before the statement leaves.**
//!
//! The provider crate reads a result back by matching the server's own column names against the
//! plan's field names, and *fails* when they differ (`arrow_sql_gen::rows_to_arrow`'s reorder →
//! `Projected schema field "…" not found in query result`). For a scan of bare columns the two
//! agree by construction. For anything else they cannot: DataFusion names an aggregate
//! `sum(o.total)` while the server names the column after the SQL it was handed —
//! ``sum(CAST(`o`.`total` AS SIGNED))``, in the unparser's own spelling. So **every** federated
//! aggregate, expression or cast over a `MySQL` source failed, which is the profile, the
//! GROUP BY and the count together.
//!
//! The fix is to say the names out loud: each projected item is given the alias the plan's schema
//! calls it, positionally, which is the identity the statement actually has — the executor sends
//! the projection the plan asked for, in the plan's order. Naming it keeps the crate's own
//! name-matched path working, and with it the per-column hints (`is_column_binary`, a decimal's
//! precision) that reading back with no projected schema would throw away.
//!
//! It is a **rewrite of the statement**, so it rides `SQLExecutor::execute`, the one hop that is
//! handed both the SQL and the schema it has to answer with. `ast_analyzer` is the natural home
//! for a source's recoding and is not usable here: it sees the statement and not the schema.
//!
//! Everything else delegates, through the seam `sources::sql` publishes — the assembly is that
//! module's and this one composes it. `UPSTREAM_REPORTS.md` carries the report.

use std::sync::Arc;

use crate::sources::sql::{AstAnalyzer, LogicalOptimizer, SQLExecutor};
use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::common::{Result as DfResult, Statistics};
use datafusion::logical_expr::LogicalPlan;
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion::physical_plan::{PhysicalExpr, SendableRecordBatchStream};
use datafusion::sql::sqlparser::ast::{Ident, SelectItem, SetExpr, Statement};
use datafusion::sql::sqlparser::dialect::MySqlDialect as MySqlSyntax;
use datafusion::sql::sqlparser::parser::Parser;
use datafusion::sql::unparser::dialect::Dialect;

/// The crate's executor, with every projected column named the way the plan names it.
pub(super) struct NamedProjection {
    pub(super) inner: Arc<dyn SQLExecutor>,
}

#[async_trait]
impl SQLExecutor for NamedProjection {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn compute_context(&self) -> Option<String> {
        self.inner.compute_context()
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        self.inner.dialect()
    }

    fn logical_optimizer(&self) -> Option<LogicalOptimizer> {
        self.inner.logical_optimizer()
    }

    fn ast_analyzer(&self) -> Option<AstAnalyzer> {
        self.inner.ast_analyzer()
    }

    /// The one method that is not a delegation.
    ///
    /// A statement this cannot name is sent exactly as it arrived: the rewrite is an improvement on
    /// a statement that would otherwise fail, never a precondition for one that would have worked.
    fn execute(
        &self,
        query: &str,
        schema: SchemaRef,
        filters: &[Arc<dyn PhysicalExpr>],
    ) -> DfResult<SendableRecordBatchStream> {
        let named = named_projection(query, &schema);
        self.inner
            .execute(named.as_deref().unwrap_or(query), schema, filters)
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

/// `query` with each projected item aliased to the field `schema` holds in that position, or
/// `None` where there is nothing to say.
///
/// `None` rather than an error for every shape this cannot name — a statement that will not parse,
/// a set operation, a wildcard, a projection whose length disagrees with the schema. All of them
/// mean "the positional claim is not established here", and the honest answer to that is the
/// statement the caller already had.
fn named_projection(query: &str, schema: &SchemaRef) -> Option<String> {
    let mut parsed = Parser::parse_sql(&MySqlSyntax {}, query).ok()?;
    let [statement] = parsed.as_mut_slice() else {
        return None;
    };
    let Statement::Query(body) = statement else {
        return None;
    };
    let SetExpr::Select(select) = body.body.as_mut() else {
        return None;
    };
    if select.projection.len() != schema.fields().len() {
        return None;
    }
    let projection = std::mem::take(&mut select.projection);
    let named: Option<Vec<SelectItem>> = projection
        .into_iter()
        .zip(schema.fields())
        .map(|(item, field)| {
            let (SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. }) = item
            else {
                return None;
            };
            Some(SelectItem::ExprWithAlias {
                expr,
                alias: Ident::with_quote('`', field.name()),
            })
        })
        .collect();
    select.projection = named?;
    Some(statement.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::datatypes::{DataType, Field, Schema};

    use super::*;

    fn schema(names: &[&str]) -> SchemaRef {
        Arc::new(Schema::new(
            names
                .iter()
                .map(|name| Field::new(*name, DataType::Int64, true))
                .collect::<Vec<_>>(),
        ))
    }

    /// **The case the whole module exists for**: an aggregate the server would have named after
    /// the SQL it was handed is named after the plan's own field instead.
    #[test]
    fn an_aggregate_is_named_the_way_the_plan_names_it() {
        assert_eq!(
            named_projection(
                "SELECT `c`.`name`, sum(CAST(`o`.`total` AS SIGNED)) FROM `shop`.`orders` AS `o` \
                 GROUP BY `c`.`name`",
                &schema(&["name", "sum(o.total)"])
            )
            .as_deref(),
            Some(
                "SELECT `c`.`name` AS `name`, sum(CAST(`o`.`total` AS SIGNED)) AS \
                 `sum(o.total)` FROM `shop`.`orders` AS `o` GROUP BY `c`.`name`"
            )
        );
    }

    /// An alias the statement already carries is **replaced**, not appended to: the plan's field
    /// name is the one the result has to come back under, and the unparser's own alias is a
    /// rendering of an expression that has since been renamed.
    #[test]
    fn an_alias_the_statement_already_has_is_replaced() {
        assert_eq!(
            named_projection(
                "SELECT `total` AS `t` FROM `shop`.`orders`",
                &schema(&["total"])
            )
            .as_deref(),
            Some("SELECT `total` AS `total` FROM `shop`.`orders`")
        );
    }

    /// A name with a backtick in it is escaped by doubling, because the alias is written as an
    /// identifier rather than pasted into the text.
    #[test]
    fn a_name_with_a_backtick_is_escaped() {
        assert_eq!(
            named_projection("SELECT 1", &schema(&["we`ird"])).as_deref(),
            Some("SELECT 1 AS `we``ird`")
        );
    }

    /// **Every shape it cannot name is left alone**, so the rewrite can only ever improve a
    /// statement that was going to fail.
    #[test]
    fn a_statement_it_cannot_name_is_sent_as_it_arrived() {
        for (sql, fields) in [
            ("SELECT * FROM `shop`.`orders`", &["id"][..]),
            ("SELECT `id`, `total` FROM `shop`.`orders`", &["id"][..]),
            (
                "SELECT `id` FROM `shop`.`orders` UNION SELECT `id` FROM `shop`.`customers`",
                &["id"][..],
            ),
            ("INSERT INTO `t` VALUES (1)", &["id"][..]),
            ("this is not sql", &["id"][..]),
        ] {
            assert_eq!(named_projection(sql, &schema(fields)), None, "{sql}");
        }
    }
}
