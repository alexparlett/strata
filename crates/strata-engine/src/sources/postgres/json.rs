//! **The JSON accessor family, spelled the way Postgres spells it.**
//!
//! `payload ->> 'type'` is planned by `datafusion-functions-json` into a UDF call — `->` becomes
//! `json_get`, `->>` becomes `json_as_text`, `?` becomes `json_contains` — and a UDF call unparses
//! **by name**, so a federated subplan carries `json_as_text(payload, 'type')` to a server that has
//! no such function. Federation has no per-expression fallback: the subplan is not re-planned
//! locally, so this is an execute-time remote error rather than a slow answer.
//!
//! The symmetry that makes the rewrite right, rather than an emulation: server-side the column
//! really is `jsonb`, and Postgres natively speaks the operators the user typed. `payload ->>
//! 'type'` is better SQL there than anything we could send instead.
//!
//! **[`FAMILY`] is the whole of what this module knows**, and it is the only source of "mapped":
//! the rewrite reads it to translate, and [`support`] hands the same table to the engine as the
//! backend's [`FunctionMap`](crate::sources::backend::FunctionMap). A member is mapped only where
//! the operator means the *same thing* as the local function, judged against what the local
//! function returns — never a lossy approximation, because a query that answers differently
//! depending on where it ran is worse than one that refuses.

use std::ops::ControlFlow;

use datafusion::common::{DataFusionError, Result as DfResult};
use datafusion::sql::sqlparser::ast::{
    BinaryOperator, Expr, Function, FunctionArg, FunctionArgExpr, FunctionArguments, Statement,
    VisitMut, VisitorMut,
};

use crate::sources::source::{unsupported_function, FunctionMap, Support, MATERIALIZE};

/// What an accessor becomes on the server, given a value and a path of one or more keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spelling {
    /// `(x -> k1 -> … ->> kn)` — the value at the path, as text.
    Text,
    /// `((x -> k1 -> … -> kn) IS NOT NULL)` — whether the path resolves to anything.
    Present,
}

/// The `datafusion-functions-json` family, keyed by the names the **planner** produces, and what
/// each one is spelled as on the server.
///
/// The names are the UDFs' own (`ScalarUDF::name`), which is what the unparser writes: an alias
/// (`json_len`) never reaches a statement, and `json_get_str` reaches one only when a user types it,
/// since the operator rewrite produces `json_get` / `json_as_text` / `json_contains` and nothing
/// else.
///
/// Why each unmapped entry is unmapped — every one of them is a semantic difference, not an
/// omission:
///
/// - `json_get` returns `JsonUnion`'s Arrow *union*, which no Postgres expression produces. The
///   app renders that type specially (`query::json_unions_as_text`), so a `->` that pushed down as
///   `->` would come back as text and the column would read differently depending on where it ran.
///   Its refusal names `->>`, which does push down.
/// - `json_get_str` is NULL for anything that is not a JSON string, where `->>` stringifies
///   objects, arrays, numbers and booleans.
/// - `json_get_json` and `json_get_array` hand back the source slice verbatim; `->` hands back
///   `jsonb`, which is normalised (whitespace dropped, object keys reordered, numbers
///   canonicalised).
/// - `json_get_int` / `json_get_float` / `json_get_bool` are NULL for a value of the wrong JSON
///   type, where a cast off `->>` would raise a server-side error instead.
/// - `json_length` counts array elements *or* object keys and is NULL for anything else;
///   `jsonb_array_length` raises on a non-array, and the object half is a set-returning function.
/// - `json_object_keys` returns a list per row, where `jsonb_object_keys` returns a row per key.
/// - `json_from_scalar` is about the union type, which never leaves here.
///
/// **`json_union_to_text` is deliberately absent**, and it is the one omission that is not about
/// semantics. It is never something a user typed: it is `query::json_unions_as_text`'s own
/// projection over a `json_get` result, added after planning. Refusing it *by name* would name a
/// function the user cannot see or remove — which is what a first version did, since it sits above
/// the `json_get` in the statement and the traversal reaches an outer projection before the
/// subquery under it. Left out, the `json_get` that put the union there is what refuses, in the
/// user's own terms; and a union that reached a remote statement without one would fail on the
/// server as an undefined function, which [`remote_refusal`] answers.
const FAMILY: &[(&str, Option<Spelling>, &str)] = &[
    ("json_as_text", Some(Spelling::Text), ""),
    ("json_contains", Some(Spelling::Present), ""),
    ("json_from_scalar", None, ""),
    ("json_get", None, ARROW_INSTEAD),
    ("json_get_array", None, ""),
    ("json_get_bool", None, ""),
    ("json_get_float", None, ""),
    ("json_get_int", None, ""),
    ("json_get_json", None, ""),
    ("json_get_str", None, ""),
    ("json_length", None, ""),
    ("json_object_keys", None, ""),
];

/// What `->` has instead of itself: the one member of the family whose refusal can name a working
/// alternative.
pub(super) const ARROW_INSTEAD: &str = "Use '->>' instead, which does run on the server.";

/// [`FAMILY`] as the engine reads it — which members this server can compute, and what a refusal
/// about one of the others has to add.
pub(super) fn support() -> FunctionMap {
    FunctionMap::of(FAMILY.iter().map(|&(name, spelling, why)| {
        let support = match spelling {
            Some(_) => Support::Mapped,
            None => Support::Unmapped {
                why: why.to_string(),
            },
        };
        (name, support)
    }))
}

/// Postgres's `undefined_function`, which covers a missing function *and* a missing operator —
/// what a federated statement gets back for carrying a name only DataFusion knows.
///
/// The **code**, not the prose, and the prefix is read off the crate that writes it:
/// `datafusion-table-providers-postgres`'s `format_postgres_query_error` renders
/// `"{db_error}\nSQLSTATE: {code}"` for every server error it hands back. Matching the wording
/// instead would miss every 42883 spelled some third way — `could not identify an equality
/// operator for type json` is the one a federated `SELECT DISTINCT` over a `json` column
/// raises — and would fire on any message where the words merely co-occur. If the crate ever
/// stops rendering the code, this stops wrapping, which is the safe direction: the server's own
/// answer, exactly as before this existed.
const UNDEFINED_FUNCTION: &str = "SQLSTATE: 42883";

/// Why a **mapped** accessor cannot be sent *in this shape* — the call has no lookup path, and a
/// path is the whole of what either operator form is built from.
///
/// Its own sentence rather than [`unsupported_function`]'s, because that one says the function
/// cannot run on this connection at all, which the same accessor with a key would immediately
/// disprove.
fn pathless_refusal(function: &str, connection: &str) -> String {
    format!(
        "'{function}' cannot run on the database connection '{connection}' without a key to look \
         up. {MATERIALIZE}"
    )
}

/// Whether a remote statement failed because the server has no such function or operator, rather
/// than for any of the reasons a server's own answer is the right one to show.
pub(super) fn lacks_the_name(raw: &str) -> bool {
    raw.contains(UNDEFINED_FUNCTION)
}

/// The same way out, for the refusals only the server can raise — a created SQL macro that
/// survived `simplify`, an arrow builtin, an accessor over a column that is `text` rather than
/// `json`.
///
/// [`FAMILY`] deliberately does not claim to enumerate those, so this says what is true of all of
/// them — the server lacks the name — and keeps the server's own sentence, which is the only thing
/// that can say *which* name. On a line of its own, because that sentence is already several and
/// ends in the provider crate's documentation URL.
pub(super) fn remote_refusal(raw: &str, connection: &str) -> String {
    format!(
        "{raw}\nThe database connection '{connection}' does not have it, so this query cannot run \
         on the server. {MATERIALIZE}"
    )
}

/// Rewrite every mapped accessor in `statement` into its operator form, and refuse the statement
/// if it carries an unmapped one.
///
/// One pass over the whole statement, because one table answers both questions: a family name is
/// either translated or is the refusal. Bottom-up, so a nested accessor is already an operator
/// expression by the time the call around it is read.
pub(super) fn push_down(mut statement: Statement, connection: &str) -> DfResult<Statement> {
    match VisitMut::visit(&mut statement, &mut PushDown { connection }) {
        ControlFlow::Continue(()) => Ok(statement),
        ControlFlow::Break(why) => Err(DataFusionError::Execution(why)),
    }
}

struct PushDown<'a> {
    connection: &'a str,
}

impl VisitorMut for PushDown<'_> {
    type Break = String;

    fn post_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        let Expr::Function(function) = expr else {
            return ControlFlow::Continue(());
        };
        let Some(called) = plain_call(function) else {
            return ControlFlow::Continue(());
        };
        let Some(&(name, spelling, why)) = FAMILY.iter().find(|(known, ..)| *known == called)
        else {
            return ControlFlow::Continue(());
        };
        let Some(spelling) = spelling else {
            return ControlFlow::Break(unsupported_function(name, self.connection, why));
        };
        match spelled(spelling, function) {
            Some(operators) => {
                *expr = operators;
                ControlFlow::Continue(())
            }
            None => ControlFlow::Break(pathless_refusal(name, self.connection)),
        }
    }
}

/// The name of a plain scalar call — one identifier, a positional argument list, and none of the
/// clauses a scalar function never carries.
///
/// Anything else wearing a family name is left alone: this rewrites what the unparser writes for a
/// `ScalarFunction`, and a shape it does not write is not one to guess at.
fn plain_call(function: &Function) -> Option<&str> {
    let Function {
        name,
        uses_odbc_syntax: false,
        parameters: FunctionArguments::None,
        args: FunctionArguments::List(_),
        filter: None,
        null_treatment: None,
        over: None,
        within_group,
    } = function
    else {
        return None;
    };
    match (within_group.is_empty(), name.0.as_slice()) {
        (true, [part]) => part.as_ident().map(|ident| ident.value.as_str()),
        _ => None,
    }
}

/// `function`'s arguments as the operator expression `spelling` describes, or `None` where the
/// call is not a value and a path of at least one key — which is the only shape either operator
/// form can carry, and so is a refusal rather than a translation.
fn spelled(spelling: Spelling, function: &Function) -> Option<Expr> {
    let FunctionArguments::List(list) = &function.args else {
        return None;
    };
    let args = list
        .args
        .iter()
        .map(|arg| match arg {
            FunctionArg::Unnamed(FunctionArgExpr::Expr(expr)) => Some(expr.clone()),
            _ => None,
        })
        .collect::<Option<Vec<Expr>>>()?;
    let (value, path) = args.split_first()?;
    let (last, lead) = path.split_last()?;
    let walked = lead.iter().fold(value.clone(), |value, key| {
        operator(value, BinaryOperator::Arrow, key.clone())
    });
    match spelling {
        Spelling::Text => Some(operator(walked, BinaryOperator::LongArrow, last.clone())),
        Spelling::Present => Some(Expr::Nested(Box::new(Expr::IsNotNull(Box::new(operator(
            walked,
            BinaryOperator::Arrow,
            last.clone(),
        )))))),
    }
}

/// One operator application, parenthesised.
///
/// **Every step is nested**, because what is being replaced is an atom: Postgres gives `->`,
/// `->>` and every other user-level operator one precedence class, so an unparenthesised
/// `'a' || x ->> 'k'` would bind as `('a' || x) ->> 'k'`.
fn operator(value: Expr, op: BinaryOperator, key: Expr) -> Expr {
    Expr::Nested(Box::new(Expr::BinaryOp {
        left: Box::new(value),
        op,
        right: Box::new(key),
    }))
}

#[cfg(test)]
mod tests {
    use datafusion::sql::sqlparser::dialect::PostgreSqlDialect;
    use datafusion::sql::sqlparser::parser::Parser;

    use super::*;

    /// The statements under test are what the unparser writes, so they are round-tripped through
    /// the parser rather than hand-built.
    fn rewritten(sql: &str) -> Result<String, String> {
        let mut parsed = Parser::parse_sql(&PostgreSqlDialect {}, sql).expect("a statement");
        let statement = parsed.pop().expect("one statement");
        push_down(statement, "pg")
            .map(|statement| statement.to_string())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn an_accessor_becomes_the_operator_the_user_typed() {
        assert_eq!(
            rewritten("SELECT id FROM orders WHERE json_as_text(tags, 'channel') = 'web'"),
            Ok("SELECT id FROM orders WHERE (tags ->> 'channel') = 'web'".to_string())
        );
    }

    #[test]
    fn a_path_chains_arrows_and_ends_in_the_text_one() {
        assert_eq!(
            rewritten("SELECT json_as_text(tags, 'a', 'b', 0) FROM orders"),
            Ok("SELECT (((tags -> 'a') -> 'b') ->> 0) FROM orders".to_string())
        );
    }

    /// `?` asks whether the path resolves, which `IS NOT NULL` over `->` answers and Postgres's
    /// own `?` does not: `?` is true for a *string array element* too, where the local function is
    /// false, and it does not accept an integer index at all.
    #[test]
    fn a_containment_test_asks_whether_the_path_resolves() {
        assert_eq!(
            rewritten("SELECT id FROM orders WHERE json_contains(tags, 'channel')"),
            Ok("SELECT id FROM orders WHERE ((tags -> 'channel') IS NOT NULL)".to_string())
        );
        assert_eq!(
            rewritten("SELECT json_contains(tags, 'a', 'b') FROM orders"),
            Ok("SELECT (((tags -> 'a') -> 'b') IS NOT NULL) FROM orders".to_string())
        );
    }

    #[test]
    fn an_operator_expression_is_parenthesised_where_it_stands() {
        assert_eq!(
            rewritten("SELECT 'x' || json_as_text(tags, 'channel') FROM orders"),
            Ok("SELECT 'x' || (tags ->> 'channel') FROM orders".to_string())
        );
    }

    /// The planner collapses this shape into one call with a two-key path long before it reaches
    /// here (`functions-json`'s own `unnest_json_calls`), so what this pins is the **order** — an
    /// accessor inside another is never left behind as a function call.
    #[test]
    fn a_nested_accessor_is_rewritten_from_the_inside_out() {
        assert_eq!(
            rewritten("SELECT json_as_text(json_as_text(tags, 'a'), 'b') FROM orders"),
            Ok("SELECT ((tags ->> 'a') ->> 'b') FROM orders".to_string())
        );
    }

    /// `json_unions_as_text` wraps a union column in `json_union_to_text` after planning, so the
    /// statement that reaches here has Strata's own projection above the user's `json_get` — and
    /// the refusal has to name the one the user typed.
    #[test]
    fn the_union_returning_accessor_refuses_and_names_the_one_that_works() {
        let why = rewritten("SELECT json_union_to_text(json_get(tags, 'channel')) FROM orders")
            .expect_err("'->' has no server-side spelling");
        assert!(
            !why.contains("json_union_to_text"),
            "the user never typed that one: {why}"
        );
        assert!(
            why.contains("'json_get'") && why.contains("'->>'") && why.contains("'pg'"),
            "the refusal names the function, the alternative and the connection: {why}"
        );
        assert!(why.contains("CREATE TABLE"), "and the way out: {why}");
    }

    #[test]
    fn an_unmapped_member_refuses_without_claiming_an_alternative() {
        let why = rewritten("SELECT json_length(tags, 'items') FROM orders")
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
    /// unsupported, which the same accessor with a key would disprove on the next line.
    #[test]
    fn an_accessor_with_no_key_refuses_for_that_reason() {
        let why = rewritten("SELECT json_as_text(tags) FROM orders")
            .expect_err("there is nothing to look up");
        assert!(
            why.contains("'json_as_text'") && why.contains("without a key"),
            "{why}"
        );
        assert!(rewritten("SELECT json_contains(tags) FROM orders").is_err());
        assert!(rewritten("SELECT json_as_text(tags, 'a') FROM orders").is_ok());
    }

    #[test]
    fn a_function_that_is_not_in_the_family_is_left_alone() {
        assert_eq!(
            rewritten("SELECT upper(name), json_valid(tags) FROM orders"),
            Ok("SELECT upper(name), json_valid(tags) FROM orders".to_string())
        );
    }

    /// The wrapper's predicate reads the **code** the provider crate renders, so every wording
    /// `undefined_function` takes is covered and a relation that vanished — the catalog's own
    /// reconciliation, not this — is not.
    #[test]
    fn only_a_missing_name_is_wrapped() {
        let rendered = |message: &str, code: &str| {
            format!(
                "Query execution failed.\nERROR: {message}\nSQLSTATE: {code}\nFor details, refer \
                 to the PostgreSQL manual: https://www.postgresql.org/docs/17/index.html"
            )
        };
        assert!(lacks_the_name(&rendered(
            "function json_length(text, unknown) does not exist",
            "42883"
        )));
        assert!(lacks_the_name(&rendered(
            "operator does not exist: text ->> unknown",
            "42883"
        )));
        assert!(
            lacks_the_name(&rendered(
                "could not identify an equality operator for type json",
                "42883"
            )),
            "a third wording of the same code is still a name the server lacks"
        );
        assert!(!lacks_the_name(&rendered(
            "relation \"public.transient\" does not exist",
            "42P01"
        )));
        assert!(!lacks_the_name(&rendered("permission denied", "42501")));
    }

    /// The wrapped message keeps the server's own words and adds ours on a line of their own —
    /// the crate's last line is a bare URL, and a sentence appended to it reads as part of it.
    #[test]
    fn the_wrapper_starts_its_own_line() {
        let wrapped = remote_refusal("ERROR: …\nhttps://example.invalid/docs", "pg");
        assert!(
            wrapped.contains("docs\nThe database connection 'pg'"),
            "{wrapped}"
        );
        assert!(wrapped.contains("CREATE TABLE"), "{wrapped}");
    }
}
