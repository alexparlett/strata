//! **The data source's own unparser dialect**: `MySqlDialect`, plus the JSON accessor family
//! spelled the way the server spells it ([`json`]).
//!
//! The dialect is where the rewrite belongs because it is where the *rendering* decision is made:
//! `Dialect::scalar_function_to_sql_overrides` is consulted for every `Expr::ScalarFunction` the
//! unparser writes, so a mapped accessor becomes a server expression and an unmapped one refuses
//! wherever this data source's SQL is written down — the federated statement, the fallback
//! provider's own scan, and its pushdown check.
//!
//! It carries the data source's name because a refusal names it, which is also why there is one
//! dialect per registered relation rather than one shared value.
//!
//! **A newtype, not a fork.** `MySqlDialect` overrides twelve of the trait's methods and every one
//! but the scalar-function hook is forwarded verbatim below; everything it does not override is
//! the trait's own default here too. `a_delegating_dialect_answers_as_mysql_does` is what fails if
//! a dependency bump gives it a thirteenth.

use std::sync::Arc;

use datafusion::arrow::datatypes::TimeUnit;
use datafusion::common::Result as DfResult;
use datafusion::logical_expr::Expr;
use datafusion::sql::sqlparser::ast::{DataType, Expr as SqlExpr};
use datafusion::sql::unparser::dialect::{
    DateFieldExtractStyle, Dialect, IntervalStyle, MySqlDialect,
};
use datafusion::sql::unparser::Unparser;

use super::json;

/// What every method below delegates to.
const MYSQL: MySqlDialect = MySqlDialect {};

/// `MySqlDialect` as one data source speaks it.
#[derive(Debug)]
pub(super) struct MyDialect {
    /// The catalog the data source registered under — what a refusal names.
    source: String,
}

impl MyDialect {
    pub(super) fn new(source: String) -> Self {
        Self { source }
    }
}

impl Dialect for MyDialect {
    /// The family first, then `MySQL`'s own override — `date_part`, which this must not swallow.
    fn scalar_function_to_sql_overrides(
        &self,
        unparser: &Unparser,
        func_name: &str,
        args: &[Expr],
    ) -> DfResult<Option<SqlExpr>> {
        match json::override_call(unparser, func_name, args, &self.source) {
            Some(spelled) => spelled.map(Some),
            None => MYSQL.scalar_function_to_sql_overrides(unparser, func_name, args),
        }
    }

    fn supports_qualify(&self) -> bool {
        MYSQL.supports_qualify()
    }

    fn identifier_quote_style(&self, identifier: &str) -> Option<char> {
        MYSQL.identifier_quote_style(identifier)
    }

    fn supports_nulls_first_in_sort(&self) -> bool {
        MYSQL.supports_nulls_first_in_sort()
    }

    fn interval_style(&self) -> IntervalStyle {
        MYSQL.interval_style()
    }

    fn utf8_cast_dtype(&self) -> DataType {
        MYSQL.utf8_cast_dtype()
    }

    fn large_utf8_cast_dtype(&self) -> DataType {
        MYSQL.large_utf8_cast_dtype()
    }

    fn date_field_extract_style(&self) -> DateFieldExtractStyle {
        MYSQL.date_field_extract_style()
    }

    fn int64_cast_dtype(&self) -> DataType {
        MYSQL.int64_cast_dtype()
    }

    fn int32_cast_dtype(&self) -> DataType {
        MYSQL.int32_cast_dtype()
    }

    fn timestamp_cast_dtype(&self, time_unit: &TimeUnit, tz: &Option<Arc<str>>) -> DataType {
        MYSQL.timestamp_cast_dtype(time_unit, tz)
    }

    fn requires_derived_table_alias(&self) -> bool {
        MYSQL.requires_derived_table_alias()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::arrow::array::RecordBatch;
    use datafusion::arrow::datatypes::{DataType as ArrowType, Field, Schema};
    use datafusion::logical_expr::{Expr as PlanExpr, LogicalPlan, TableProviderFilterPushDown};
    use datafusion::prelude::SessionContext;
    use datafusion_table_providers_common::sql::sql_provider_datafusion::default_filter_pushdown;

    use super::*;

    /// One data source's dialect, under the name every refusal below names.
    fn my() -> MyDialect {
        MyDialect::new("my".to_string())
    }

    /// The statement this data source's provider writes for `sql`, or the refusal that stopped it.
    ///
    /// A planned query rather than a hand-built expression, because what the override is handed is
    /// what the *planner* made of what the user typed: `->>` is a `json_as_text` call by the time
    /// anything here sees it.
    async fn unparsed(sql: &str) -> Result<String, String> {
        Unparser::new(&my())
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

    /// The scan the plan built for `sql` refers to the table as `orders`, in backticks.
    const SCAN: &str = "FROM `orders`";

    #[tokio::test]
    async fn an_accessor_becomes_the_guarded_extract() {
        assert_eq!(
            unparsed("SELECT id FROM orders WHERE json_as_text(tags, 'channel') = 'web'").await,
            Ok(format!(
                "SELECT `orders`.`id` {SCAN} WHERE (JSON_UNQUOTE(NULLIF(JSON_EXTRACT\
                 (`orders`.`tags`, '$.channel'), CAST('null' AS JSON))) = 'web')"
            ))
        );
    }

    /// A chain is **one** path, not a chain of operators: the whole lookup is a single string the
    /// server parses.
    #[tokio::test]
    async fn a_chained_accessor_is_one_path() {
        assert_eq!(
            unparsed("SELECT json_as_text(tags, 'a', 'b') FROM orders").await,
            Ok(format!(
                "SELECT JSON_UNQUOTE(NULLIF(JSON_EXTRACT(`orders`.`tags`, '$.a.b'), \
                 CAST('null' AS JSON))) {SCAN}"
            ))
        );
    }

    #[tokio::test]
    async fn a_containment_test_asks_whether_the_path_resolves() {
        assert_eq!(
            unparsed("SELECT id FROM orders WHERE json_contains(tags, 'channel')").await,
            Ok(format!(
                "SELECT `orders`.`id` {SCAN} WHERE COALESCE(JSON_CONTAINS_PATH(`orders`.`tags`, \
                 'one', '$.channel'), false)"
            ))
        );
    }

    /// **An accessor inside another is rewritten too**, from the inside out: each argument goes
    /// back through the unparser and reaches this same override, so no inner call is left behind
    /// as a function the server has never heard of.
    ///
    /// The nested spelling is what the nested call *means* — read the value at `a` as text, parse
    /// that text, read `b` from it — which is what the server's own `JSON_EXTRACT` over a string
    /// does. (`functions-json`'s `unnest_json_calls` collapses this shape into one two-key call
    /// when the optimizer runs; this plan is unoptimized, which is what leaves it nested here.)
    #[tokio::test]
    async fn a_nested_accessor_is_rewritten_from_the_inside_out() {
        assert_eq!(
            unparsed("SELECT json_as_text(json_as_text(tags, 'a'), 'b') FROM orders").await,
            Ok(format!(
                "SELECT JSON_UNQUOTE(NULLIF(JSON_EXTRACT(JSON_UNQUOTE(NULLIF(JSON_EXTRACT\
                 (`orders`.`tags`, '$.a'), CAST('null' AS JSON))), '$.b'), \
                 CAST('null' AS JSON))) {SCAN}"
            ))
        );
    }

    /// `json_unions_as_text` wraps a union column in `json_union_to_text` after planning, so the
    /// expression that reaches here has Strata's own call above the user's `json_get` — and the
    /// refusal has to name the one the user typed.
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
            why.contains("'json_get'") && why.contains("'->>'") && why.contains("'my'"),
            "the refusal names the function, the alternative and the data source: {why}"
        );
        assert!(why.contains("CREATE TABLE"), "and the way out: {why}");
    }

    /// **The server has a `JSON_LENGTH` of its own**, and it is not this one — so the refusal is
    /// what stops a differently-behaving function running under the user's own spelling.
    #[tokio::test]
    async fn a_name_the_server_also_has_still_refuses() {
        let why = unparsed("SELECT json_length(tags, 'items') FROM orders")
            .await
            .expect_err("MySQL's JSON_LENGTH is a different function");
        assert!(
            why.contains("'json_length'") && why.contains("'my'") && why.contains("CREATE TABLE"),
            "{why}"
        );
        assert!(
            !why.contains("'->>'"),
            "only '->' has an alternative to name: {why}"
        );
    }

    /// A path is what both spellings are built from, so a call without one is a refusal rather
    /// than a half-translated expression — and it says *that*, rather than that the accessor is
    /// unsupported, which the same accessor with a key disproves on the next line.
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

    /// A key the path syntax could only carry in quotes is refused **by name**, because the
    /// driver strips the quotes it would need and an unquoted `$.first-name` reads as arithmetic
    /// over `$.first`.
    #[tokio::test]
    async fn a_key_the_path_cannot_carry_is_refused_by_name() {
        let why = unparsed("SELECT json_as_text(tags, 'first-name') FROM orders")
            .await
            .expect_err("a hyphen is not a plain member name");
        assert!(
            why.contains("'first-name'") && why.contains("plain key") && why.contains("'my'"),
            "{why}"
        );
    }

    #[tokio::test]
    async fn a_function_that_is_not_in_the_family_is_left_alone() {
        assert_eq!(
            unparsed("SELECT upper(name) FROM orders").await,
            Ok(format!("SELECT upper(`orders`.`name`) {SCAN}"))
        );
    }

    /// **The family is offered the name first, and everything else falls through** — including
    /// `MySqlDialect`'s own override, which a newtype that answered `None` for non-family names
    /// would have swallowed.
    #[tokio::test]
    async fn mysqls_own_scalar_override_survives_the_newtype() {
        let written = unparsed("SELECT date_part('year', CAST(name AS TIMESTAMP)) FROM orders")
            .await
            .expect("a rendered extract");
        assert!(
            written.contains("EXTRACT(YEAR FROM"),
            "'date_part' takes MySQL's EXTRACT: {written}"
        );
    }

    /// **A refused call stops claiming pushdown**, which is the plan-shape half of putting the
    /// rewrite on the dialect: the fallback provider asks the dialect whether a filter can be
    /// written down, and a filter it cannot write is one DataFusion keeps and evaluates here.
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
            (
                "SELECT id FROM orders WHERE json_as_text(tags, 'first-name') = 'web'",
                TableProviderFilterPushDown::Unsupported,
            ),
        ] {
            let predicate = predicate_of(&planned(sql).await);
            assert_eq!(
                default_filter_pushdown(&[&predicate], &my()),
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

    /// **Everything but the scalar-function hook is `MySQL`'s own answer.**
    ///
    /// The newtype forwards the eleven other methods `MySqlDialect` overrides and takes the
    /// trait's default for the rest — which is only correct while those two sets are what they
    /// are. Comparing every cheap answer, forwarded or not, is what fails if a dependency bump
    /// gives `MySqlDialect` a twelfth. (`interval_style` is forwarded but not compared:
    /// `IntervalStyle` is neither `PartialEq` nor `Debug`.)
    #[test]
    fn a_delegating_dialect_answers_as_mysql_does() {
        macro_rules! same {
            ($ours:expr, $($method:ident($($arg:expr),*)),+ $(,)?) => {
                vec![$((
                    stringify!($method),
                    $ours.$method($($arg),*) == MYSQL.$method($($arg),*),
                )),+]
            };
        }
        let ours = my();
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
                == MYSQL.col_alias_overrides("alias").expect("no failure"),
        ));
        let differing: Vec<&str> = answers
            .iter()
            .filter(|(_, same)| !same)
            .map(|(method, _)| *method)
            .collect();
        assert!(
            differing.is_empty(),
            "these no longer answer as MySqlDialect's own do: {differing:?}"
        );
    }
}
