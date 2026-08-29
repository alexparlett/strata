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
//! [`override_call`] reads it to translate, and [`support`] hands the same table to the engine as
//! the source's [`FunctionMap`](crate::sources::source::FunctionMap). A member is mapped only
//! where the operator means the *same thing* as the local function, judged against what the local
//! function returns — never a lossy approximation, because a query that answers differently
//! depending on where it ran is worse than one that refuses.
//!
//! *Where* the translation happens is [`dialect`](super::dialect)'s: the connection's own unparser
//! dialect, so it is the same answer wherever the connection's SQL is written down.

use datafusion::common::{DataFusionError, Result as DfResult};
use datafusion::logical_expr::Expr;
use datafusion::sql::sqlparser::ast::{BinaryOperator, Expr as SqlExpr};
use datafusion::sql::unparser::Unparser;

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
/// function the user cannot see or remove — which is what a first version did. Left out, it is
/// handed to `PostgreSqlDialect` like any other unknown name, whose default writes a call and
/// unparses the arguments under it; the `json_get` that put the union there is what refuses then,
/// in the user's own terms. And a union that reached a remote statement without one would fail on
/// the server as an undefined function, which [`remote_refusal`] answers.
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

/// How `connection` spells a call to `function`, or `None` where [`FAMILY`] has no opinion about
/// the name — which is the caller's cue to fall through to `PostgreSqlDialect`'s own overrides.
///
/// The two answers it does give come from one table because they are one question: a family name
/// is either translated or is the refusal.
pub(super) fn override_call(
    unparser: &Unparser,
    function: &str,
    args: &[Expr],
    connection: &str,
) -> Option<DfResult<SqlExpr>> {
    let &(name, spelling, why) = FAMILY.iter().find(|(known, ..)| *known == function)?;
    let Some(spelling) = spelling else {
        return Some(refused(unsupported_function(name, connection, why)));
    };
    Some(
        spelled(unparser, spelling, args)
            .unwrap_or_else(|| refused(pathless_refusal(name, connection))),
    )
}

/// A refusal, in the shape every hop between here and the results pane passes through unwrapped.
fn refused(why: String) -> DfResult<SqlExpr> {
    Err(DataFusionError::Execution(why))
}

/// `args` as the operator expression `spelling` describes, or `None` where the call is not a value
/// and a path of at least one key — which is the only shape either operator form can carry, and so
/// is a refusal rather than a translation.
fn spelled(unparser: &Unparser, spelling: Spelling, args: &[Expr]) -> Option<DfResult<SqlExpr>> {
    let (value, path) = args.split_first()?;
    let (last, lead) = path.split_last()?;
    Some(chained(unparser, spelling, value, lead, last))
}

/// The chain itself, once the call is known to carry a value and a path.
///
/// Each argument goes back through `unparser`, which is what makes a nested accessor an operator
/// expression too: the inner call reaches this same override on the way past, and an inner member
/// with no spelling refuses from in there.
fn chained(
    unparser: &Unparser,
    spelling: Spelling,
    value: &Expr,
    lead: &[Expr],
    last: &Expr,
) -> DfResult<SqlExpr> {
    let walked = lead.iter().try_fold(
        unparser.expr_to_sql(value)?,
        |walked, key| -> DfResult<SqlExpr> {
            Ok(operator(
                walked,
                BinaryOperator::Arrow,
                unparser.expr_to_sql(key)?,
            ))
        },
    )?;
    let last = unparser.expr_to_sql(last)?;
    Ok(match spelling {
        Spelling::Text => operator(walked, BinaryOperator::LongArrow, last),
        Spelling::Present => SqlExpr::Nested(Box::new(SqlExpr::IsNotNull(Box::new(operator(
            walked,
            BinaryOperator::Arrow,
            last,
        ))))),
    })
}

/// One operator application, parenthesised.
///
/// **Every step is nested**, because what is being replaced is an atom: Postgres gives `->`,
/// `->>` and every other user-level operator one precedence class, so an unparenthesised
/// `'a' || x ->> 'k'` would bind as `('a' || x) ->> 'k'`. The unparser parenthesises the binary
/// operators *it* writes and knows nothing about the ones written here.
fn operator(value: SqlExpr, op: BinaryOperator, key: SqlExpr) -> SqlExpr {
    SqlExpr::Nested(Box::new(SqlExpr::BinaryOp {
        left: Box::new(value),
        op,
        right: Box::new(key),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
