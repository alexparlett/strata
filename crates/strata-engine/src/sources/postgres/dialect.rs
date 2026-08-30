//! **The data source's own unparser dialect**: `PostgreSqlDialect`, plus the JSON accessor family
//! spelled the way the server spells it ([`json`]).
//!
//! The dialect is where the rewrite belongs because it is where the *rendering* decision is made:
//! `Dialect::scalar_function_to_sql_overrides` is consulted for every `Expr::ScalarFunction` the
//! unparser writes, so a mapped accessor becomes an operator expression and an unmapped one
//! refuses wherever this data source's SQL is written down — the federated statement, the fallback
//! provider's own scan, and its pushdown check. The rewrite it replaces ran *after* unparsing, on
//! the federation executor's `ast_analyzer`, and so reached only the first of those three.
//!
//! It carries the data source's name because a refusal names it, which is also why there is one
//! dialect per registered relation rather than one shared value.
//!
//! **A newtype, not a fork.** `PostgreSqlDialect` overrides nine of the trait's methods and every
//! one but the scalar-function hook is forwarded verbatim below; everything it does not override
//! is the trait's own default here too. `a_delegating_dialect_answers_as_postgres_does` is what
//! fails if a dependency bump gives it a tenth.

use datafusion::common::Result as DfResult;
use datafusion::logical_expr::Expr;
use datafusion::sql::sqlparser::ast::{DataType, Expr as SqlExpr};
use datafusion::sql::unparser::dialect::{Dialect, IntervalStyle, PostgreSqlDialect};
use datafusion::sql::unparser::Unparser;

use super::json;

/// What every method below delegates to.
const POSTGRES: PostgreSqlDialect = PostgreSqlDialect {};

/// `PostgreSqlDialect` as one data source speaks it.
#[derive(Debug)]
pub(super) struct PgDialect {
    /// The catalog the data source registered under — what a refusal names.
    source: String,
}

impl PgDialect {
    pub(super) fn new(source: String) -> Self {
        Self { source }
    }
}

impl Dialect for PgDialect {
    /// The family first, then Postgres's own overrides — `array_has` and `round`, which this must
    /// not swallow.
    fn scalar_function_to_sql_overrides(
        &self,
        unparser: &Unparser,
        func_name: &str,
        args: &[Expr],
    ) -> DfResult<Option<SqlExpr>> {
        match json::override_call(unparser, func_name, args, &self.source) {
            Some(spelled) => spelled.map(Some),
            None => POSTGRES.scalar_function_to_sql_overrides(unparser, func_name, args),
        }
    }

    fn identifier_quote_style(&self, identifier: &str) -> Option<char> {
        POSTGRES.identifier_quote_style(identifier)
    }

    fn use_array_keyword_for_array_literals(&self) -> bool {
        POSTGRES.use_array_keyword_for_array_literals()
    }

    fn supports_qualify(&self) -> bool {
        POSTGRES.supports_qualify()
    }

    fn requires_derived_table_alias(&self) -> bool {
        POSTGRES.requires_derived_table_alias()
    }

    fn supports_empty_select_list(&self) -> bool {
        POSTGRES.supports_empty_select_list()
    }

    fn interval_style(&self) -> IntervalStyle {
        POSTGRES.interval_style()
    }

    fn float64_ast_dtype(&self) -> DataType {
        POSTGRES.float64_ast_dtype()
    }

    fn int8_cast_dtype(&self) -> DataType {
        POSTGRES.int8_cast_dtype()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::array::RecordBatch;
    use datafusion::arrow::datatypes::{DataType as ArrowType, Field, Schema, TimeUnit};
    use datafusion::logical_expr::{Expr as PlanExpr, LogicalPlan, TableProviderFilterPushDown};
    use datafusion::prelude::SessionContext;
    use datafusion_table_providers_common::sql::sql_provider_datafusion::default_filter_pushdown;

    use super::*;

    /// One data source's dialect, under the name every refusal below names.
    fn pg() -> PgDialect {
        PgDialect::new("pg".to_string())
    }

    /// The statement this data source's provider writes for `sql`, or the refusal that stopped it.
    ///
    /// A planned query rather than a hand-built expression, because what the override is handed is
    /// what the *planner* made of what the user typed: `->>` is a `json_as_text` call by the time
    /// anything here sees it. Unoptimized, so a nested accessor is still nested — `functions-json`
    /// collapses those in a rule of its own.
    ///
    /// **No federation anywhere in it.** The rewrite used to ride the federation executor and so
    /// reached only a subplan the federation rule had taken; on the dialect it is simply how this
    /// data source's SQL is written, which is what an ordinary `Unparser` here demonstrates.
    async fn unparsed(sql: &str) -> Result<String, String> {
        Unparser::new(&pg())
            .plan_to_sql(&planned(sql).await)
            .map(|statement| statement.to_string())
            .map_err(|e| e.to_string())
    }

    /// `sql` planned against a relation shaped like the remote one, with the JSON family
    /// registered as the engine registers it.
    async fn planned(sql: &str) -> LogicalPlan {
        let mut ctx = SessionContext::new();
        datafusion_functions_json::register_all(&mut ctx).expect("the JSON functions");
        let schema = Schema::new(vec![
            Field::new("id", ArrowType::Int64, true),
            Field::new("name", ArrowType::Utf8, true),
            Field::new("tags", ArrowType::Utf8, true),
        ]);
        ctx.register_batch("orders", RecordBatch::new_empty(Arc::new(schema)))
            .expect("a relation to plan against");
        ctx.state()
            .create_logical_plan(sql)
            .await
            .expect("a planned statement")
    }

    /// The scan the plan built for `sql` refers to the table as `orders`, quoted.
    const SCAN: &str = r#"FROM "orders""#;

    #[tokio::test]
    async fn an_accessor_becomes_the_operator_the_user_typed() {
        assert_eq!(
            unparsed("SELECT id FROM orders WHERE json_as_text(tags, 'channel') = 'web'").await,
            Ok(format!(
                r#"SELECT "orders"."id" {SCAN} WHERE (("orders"."tags" ->> 'channel') = 'web')"#
            ))
        );
    }

    #[tokio::test]
    async fn a_path_chains_arrows_and_ends_in_the_text_one() {
        assert_eq!(
            unparsed("SELECT json_as_text(tags, 'a', 'b') FROM orders").await,
            Ok(format!(
                r#"SELECT (("orders"."tags" -> 'a') ->> 'b') {SCAN}"#
            ))
        );
    }

    /// `?` asks whether the path resolves, which `IS NOT NULL` over `->` answers and Postgres's
    /// own `?` does not: `?` is true for a *string array element* too, where the local function is
    /// false, and it does not accept an integer index at all.
    #[tokio::test]
    async fn a_containment_test_asks_whether_the_path_resolves() {
        assert_eq!(
            unparsed("SELECT id FROM orders WHERE json_contains(tags, 'channel')").await,
            Ok(format!(
                r#"SELECT "orders"."id" {SCAN} WHERE (("orders"."tags" -> 'channel') IS NOT NULL)"#
            ))
        );
    }

    #[tokio::test]
    async fn an_operator_expression_is_parenthesised_where_it_stands() {
        assert_eq!(
            unparsed("SELECT 'x' || json_as_text(tags, 'channel') FROM orders").await,
            Ok(format!(
                r#"SELECT ('x' || ("orders"."tags" ->> 'channel')) {SCAN}"#
            ))
        );
    }

    /// The optimizer collapses this shape into one call with a two-key path (`functions-json`'s
    /// own `unnest_json_calls`), so what this pins is the **order** — an accessor inside another is
    /// never left behind as a function call, because each argument goes back through the unparser
    /// and reaches the same override.
    #[tokio::test]
    async fn a_nested_accessor_is_rewritten_from_the_inside_out() {
        assert_eq!(
            unparsed("SELECT json_as_text(json_as_text(tags, 'a'), 'b') FROM orders").await,
            Ok(format!(
                r#"SELECT (("orders"."tags" ->> 'a') ->> 'b') {SCAN}"#
            ))
        );
    }

    /// `json_unions_as_text` wraps a union column in `json_union_to_text` after planning, so the
    /// expression that reaches here has Strata's own call above the user's `json_get` — and the
    /// refusal has to name the one the user typed.
    ///
    /// The outer call is offered to the override first and is not in the family, so it goes to
    /// `PostgreSqlDialect`, whose default writes a call and unparses the arguments under it. That
    /// is where the `json_get` refuses.
    #[tokio::test]
    async fn the_union_returning_accessor_refuses_and_names_the_one_that_works() {
        let why = unparsed("SELECT json_union_to_text(json_get(tags, 'type')) FROM orders")
            .await
            .expect_err("'->' has no server-side spelling");
        assert!(
            !why.contains("json_union_to_text"),
            "the user never typed that one: {why}"
        );
        assert!(
            why.contains("'json_get'") && why.contains("'->>'") && why.contains("'pg'"),
            "the refusal names the function, the alternative and the data source: {why}"
        );
        assert!(why.contains("CREATE TABLE"), "and the way out: {why}");
    }

    #[tokio::test]
    async fn an_unmapped_member_refuses_without_claiming_an_alternative() {
        let why = unparsed("SELECT json_length(tags, 'items') FROM orders")
            .await
            .expect_err("counting has no faithful operator form");
        assert!(
            why.contains("'json_length'") && why.contains("'pg'") && why.contains("CREATE TABLE"),
            "{why}"
        );
        assert!(
            !why.contains("'->>'"),
            "only '->' has an alternative to name: {why}"
        );
    }

    /// A path is what both forms are built from, so a call without one is a refusal rather than a
    /// half-translated expression — and it says *that*, rather than that the accessor is
    /// unsupported, which the same accessor with a key disproves on the next line.
    ///
    /// `json_as_text` is the arm that reaches here at all: `json_contains` declares a minimum of
    /// two arguments, so a pathless one is refused by the planner long before any of this.
    #[tokio::test]
    async fn an_accessor_with_no_key_refuses_for_that_reason() {
        let why = unparsed("SELECT json_as_text(tags) FROM orders")
            .await
            .expect_err("there is nothing to look up");
        assert!(
            why.contains("'json_as_text'") && why.contains("without a key"),
            "{why}"
        );
        assert!(unparsed("SELECT json_as_text(tags, 'a') FROM orders")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn a_function_that_is_not_in_the_family_is_left_alone() {
        assert_eq!(
            unparsed("SELECT upper(name) FROM orders").await,
            Ok(format!(r#"SELECT upper("orders"."name") {SCAN}"#))
        );
    }

    /// **The family is offered the name first, and everything else falls through** — including
    /// `PostgreSqlDialect`'s own two overrides, which a newtype that answered `None` for
    /// non-family names would have swallowed.
    #[tokio::test]
    async fn postgres_own_scalar_overrides_survive_the_newtype() {
        assert_eq!(
            unparsed("SELECT round(CAST(id AS DOUBLE), 2) FROM orders").await,
            Ok(format!(
                r#"SELECT round(CAST("orders"."id" AS NUMERIC), 2) {SCAN}"#
            )),
            "'round' takes Postgres's numeric cast"
        );
        assert_eq!(
            unparsed("SELECT array_has(make_array(1, 2), id) FROM orders").await,
            Ok(format!(r#"SELECT "orders"."id" = ANY(ARRAY[1, 2]) {SCAN}"#)),
            "'array_has' takes Postgres's ANY, over its ARRAY literal"
        );
    }

    /// **A refused call stops claiming pushdown**, which is the plan-shape half of moving the
    /// rewrite onto the dialect: the fallback provider asks the dialect whether a filter can be
    /// written down, and a filter it cannot write is one DataFusion keeps and evaluates here.
    ///
    /// Before, that filter was `Exact` — written into the scan's SQL as a `json_length(…)` call and
    /// refused by the *server*, in the server's words. The refusal a user sees is unchanged,
    /// because the federated path unparses the filter it kept and lands on the same override.
    #[tokio::test]
    async fn a_filter_the_dialect_cannot_write_is_not_pushed_down() {
        for (sql, expected) in [
            (
                "SELECT id FROM orders WHERE json_as_text(tags, 'channel') = 'web'",
                TableProviderFilterPushDown::Exact,
            ),
            (
                "SELECT id FROM orders WHERE json_get_str(tags, 'channel') = 'web'",
                TableProviderFilterPushDown::Unsupported,
            ),
        ] {
            let predicate = predicate_of(&planned(sql).await);
            assert_eq!(
                default_filter_pushdown(&[&predicate], &pg()),
                vec![expected],
                "{sql}"
            );
        }
    }

    /// The predicate of the one `Filter` in a planned `SELECT … WHERE …`.
    fn predicate_of(plan: &LogicalPlan) -> PlanExpr {
        match plan {
            LogicalPlan::Filter(filter) => filter.predicate.clone(),
            other => predicate_of(
                other
                    .inputs()
                    .first()
                    .expect("a planned WHERE has a Filter under it"),
            ),
        }
    }

    /// **Everything but the scalar-function hook is Postgres's own answer.**
    ///
    /// The newtype forwards the eight other methods `PostgreSqlDialect` overrides and takes the
    /// trait's default for the rest — which is only correct while those two sets are what they are.
    /// Comparing every cheap answer, forwarded or not, is what fails if a dependency bump gives
    /// `PostgreSqlDialect` a ninth override. (`interval_style` is forwarded but not compared:
    /// `IntervalStyle` is neither `PartialEq` nor `Debug`.)
    #[test]
    fn a_delegating_dialect_answers_as_postgres_does() {
        macro_rules! same {
            ($ours:expr, $($method:ident($($arg:expr),*)),+ $(,)?) => {
                vec![$((
                    stringify!($method),
                    $ours.$method($($arg),*) == POSTGRES.$method($($arg),*),
                )),+]
            };
        }
        let ours = pg();
        let mut answers = same!(
            ours,
            identifier_quote_style("plain"),
            identifier_quote_style("select"),
            use_array_keyword_for_array_literals(),
            supports_nulls_first_in_sort(),
            use_timestamp_for_date64(),
            float64_ast_dtype(),
            utf8_cast_dtype(),
            large_utf8_cast_dtype(),
            int64_cast_dtype(),
            int32_cast_dtype(),
            int8_cast_dtype(),
            date32_cast_dtype(),
            timestamp_cast_dtype(&TimeUnit::Nanosecond, &None),
            date_field_extract_style(),
            character_length_style(),
            supports_column_alias_in_table_alias(),
            requires_derived_table_alias(),
            division_operator(),
            full_qualified_col(),
            unnest_as_table_factor(),
            unnest_as_lateral_flatten(),
            supports_qualify(),
            supports_empty_select_list(),
            string_literal_to_sql("literal"),
        );
        answers.push((
            "col_alias_overrides",
            ours.col_alias_overrides("alias").expect("no failure")
                == POSTGRES.col_alias_overrides("alias").expect("no failure"),
        ));
        let differing: Vec<&str> = answers
            .iter()
            .filter(|(_, same)| !same)
            .map(|(method, _)| *method)
            .collect();
        assert!(
            differing.is_empty(),
            "these no longer answer as PostgreSqlDialect's own do: {differing:?}"
        );
    }
}
