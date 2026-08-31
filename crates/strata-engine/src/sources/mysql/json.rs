//! **The JSON accessor family, spelled the way `MySQL` spells it.**
//!
//! `payload ->> 'type'` is planned by `datafusion-functions-json` into a UDF call — `->` becomes
//! `json_get`, `->>` becomes `json_as_text`, `?` becomes `json_contains` — and a UDF call unparses
//! **by name**, so a federated subplan carries `json_as_text(payload, 'type')` to a server that has
//! no such function. Federation has no per-expression fallback: the subplan is not re-planned
//! locally, so this is an execute-time remote error rather than a slow answer.
//!
//! **[`FAMILY`] is the whole of what this module knows**, and it is the only source of "mapped":
//! [`override_call`] reads it to translate, and [`support`] hands the same table to the engine as
//! the source's [`FunctionMap`](crate::sources::source::FunctionMap). A member is mapped only
//! where the server expression means the *same thing* as the local function, judged against what
//! the local function returns — never a lossy approximation, because a query that answers
//! differently depending on where it ran is worse than one that refuses.
//!
//! **Two of the differences from the `PostgreSQL` spelling are corrections, not decoration**, and
//! both were measured against a running server:
//!
//! - `JSON_EXTRACT` answers **JSON null**, not SQL `NULL`, for a key whose value is `null`. So a
//!   bare `->>` hands back the *string* `'null'` where `json_as_text` hands back `NULL`, and
//!   `JSON_CONTAINS_PATH` is what says "the path resolves" rather than Postgres's
//!   `IS NOT NULL` over `->`. [`Spelling::Text`] guards the extract with
//!   `NULLIF(…, CAST('null' AS JSON))` — against the *cast*, because `MySQL` compares a bare
//!   `'null'` string equal to the JSON string `"null"` and unequal to JSON null, which is the
//!   error in both directions at once.
//! - `JSON_CONTAINS_PATH` answers `NULL` for a `NULL` document where `json_contains` answers
//!   `false`, so [`Spelling::Present`] coalesces.
//!
//! **A path member is written unquoted, and a key that would need quotes is refused by name.**
//! `MySQL`'s path syntax quotes an exotic member in double quotes — and the provider crate strips
//! every `"` from the statement before sending it (`MySQLConnection::query_arrow`), so a quoted
//! member would arrive as an unquoted one and silently look up a different key. Unquoted plus a
//! refusal is the only spelling that cannot be wrong; the strip itself is
//! `UPSTREAM_REPORTS.md`'s.
//!
//! *Where* the translation happens is [`dialect`](super::dialect)'s: the data source's own unparser
//! dialect, so it is the same answer wherever the data source's SQL is written down.

use datafusion::common::{DataFusionError, Result as DfResult, ScalarValue};
use datafusion::logical_expr::Expr;
use datafusion::sql::sqlparser::ast::{
    CastKind, DataType, Expr as SqlExpr, Function, FunctionArg, FunctionArgExpr,
    FunctionArgumentList, FunctionArguments, ObjectName, Value, ValueWithSpan,
};
use datafusion::sql::sqlparser::tokenizer::Span;
use datafusion::sql::unparser::Unparser;

use crate::sources::source::{unsupported_function, FunctionMap, Support, MATERIALIZE};

/// What an accessor becomes on the server, given a value and a path of one or more members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spelling {
    /// `JSON_UNQUOTE(NULLIF(JSON_EXTRACT(x, '$.a.b'), CAST('null' AS JSON)))` — the value at the
    /// path, as text, with a JSON null answering SQL `NULL` the way `json_as_text` does.
    Text,
    /// `COALESCE(JSON_CONTAINS_PATH(x, 'one', '$.a.b'), FALSE)` — whether the path resolves,
    /// `false` for a document that is `NULL` the way `json_contains` is.
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
/// omission, and two of them are names `MySQL` **also has** for something else, which is the
/// sharper reason the table is a table:
///
/// - `json_get` returns `JsonUnion`'s Arrow *union*, which no `MySQL` expression produces. The app
///   renders that type specially (`query::json_unions_as_text`), so a `->` that pushed down would
///   come back as text and the column would read differently depending on where it ran. Its
///   refusal names `->>`, which does push down.
/// - `json_get_str` is NULL for anything that is not a JSON string, where the text spelling
///   stringifies objects, arrays, numbers and booleans.
/// - `json_get_json` and `json_get_array` hand back the source slice verbatim; the server hands
///   back its own normalised rendering (`{"k": "v"}`, with the space it inserts).
/// - `json_get_int` / `json_get_float` / `json_get_bool` are NULL for a value of the wrong JSON
///   type, where a cast off the text spelling would raise a server-side error instead.
/// - `json_length` counts array elements *or* object keys and is NULL for anything else.
///   **`MySQL` has a `JSON_LENGTH` of its own**, which raises on a scalar and counts an object's
///   members at the top level only — so leaving this name to unparse as a call would run a
///   different function under the user's own spelling rather than failing.
/// - `json_object_keys` returns a list per row; `JSON_KEYS` returns a JSON array, and the name
///   does not match either way.
/// - `json_contains` is mapped, but **not** to `MySQL`'s `JSON_CONTAINS`, which asks whether a
///   candidate *document* is contained rather than whether a path resolves. That is the second
///   name the server also has and means something else by.
/// - `json_from_scalar` is about the union type, which never leaves here.
///
/// **`json_union_to_text` is deliberately absent**, for the reason the `PostgreSQL` table leaves it
/// out: it is never something a user typed, so refusing it by name would name a function the user
/// cannot see or remove. Left out, it falls through to `MySqlDialect`, whose default writes a call
/// and unparses the arguments under it; the `json_get` that put the union there is what refuses
/// then, in the user's own terms.
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

/// `MySQL`'s `ER_SP_DOES_NOT_EXIST` — "FUNCTION `db`.`name` does not exist", the errno a
/// federated statement gets back for carrying a name only DataFusion knows.
///
/// The **code**, not the prose, and the rendering is read off the crate that writes it:
/// `mysql_async`'s `ServerError` renders `"ERROR {state} ({code}): {message}"`, so the code arrives
/// parenthesised between the SQLSTATE and the message. Matching the wording instead would fire on
/// any message where the words merely co-occur. If the driver ever stops rendering the code, this
/// stops wrapping, which is the safe direction: the server's own answer, exactly as before this
/// existed.
const UNDEFINED_FUNCTION: &str = " (1305): ";

/// Why a **mapped** accessor cannot be sent *in this shape* — the call has no lookup path, and a
/// path is the whole of what either spelling is built from.
///
/// Its own sentence rather than [`unsupported_function`]'s, because that one says the function
/// cannot run on this data source at all, which the same accessor with a key would immediately
/// disprove.
fn pathless_refusal(function: &str, source: &str) -> String {
    format!(
        "'{function}' cannot run on the source '{source}' without a key to look up. {MATERIALIZE}"
    )
}

/// Why a **mapped** accessor cannot be sent *for this path* — a member the server's path syntax
/// cannot carry here.
///
/// `what` is the member as the user wrote it, so the sentence names the key rather than the
/// function's shape.
fn path_refusal(function: &str, source: &str, what: &str) -> String {
    format!(
        "'{function}' cannot look up {what} on the source '{source}': a MySQL path can only name \
         a plain key here. {MATERIALIZE}"
    )
}

/// Whether a remote statement failed because the server has no such function, rather than for any
/// of the reasons a server's own answer is the right one to show.
pub(super) fn lacks_the_name(raw: &str) -> bool {
    raw.contains(UNDEFINED_FUNCTION)
}

/// The same way out, for the refusals only the server can raise — a created SQL macro that
/// survived `simplify`, or a name typed into a view body.
///
/// [`FAMILY`] deliberately does not claim to enumerate those, so this says what is true of all of
/// them — the server lacks the name — and keeps the server's own sentence, which is the only thing
/// that can say *which* name. On a line of its own, because that sentence is already several and
/// ends in the provider crate's documentation URL.
pub(super) fn remote_refusal(raw: &str, source: &str) -> String {
    format!(
        "{raw}\nThe data source '{source}' does not have it, so this query cannot run on the \
         server. {MATERIALIZE}"
    )
}

/// How `source` spells a call to `function`, or `None` where [`FAMILY`] has no opinion about the
/// name — which is the caller's cue to fall through to `MySqlDialect`'s own overrides.
///
/// The two answers it does give come from one table because they are one question: a family name
/// is either translated or is the refusal.
pub(super) fn override_call(
    unparser: &Unparser,
    function: &str,
    args: &[Expr],
    source: &str,
) -> Option<DfResult<SqlExpr>> {
    let &(name, spelling, why) = FAMILY.iter().find(|(known, ..)| *known == function)?;
    let Some(spelling) = spelling else {
        return Some(refused(unsupported_function(name, source, why)));
    };
    let Some((value, path)) = args.split_first() else {
        return Some(refused(pathless_refusal(name, source)));
    };
    if path.is_empty() {
        return Some(refused(pathless_refusal(name, source)));
    }
    let path = match path_expression(path) {
        Ok(path) => path,
        Err(what) => return Some(refused(path_refusal(name, source, &what))),
    };
    Some(
        unparser
            .expr_to_sql(value)
            .map(|value| spelled(spelling, value, &path)),
    )
}

/// A refusal, in the shape every hop between here and the results pane passes through unwrapped.
fn refused(why: String) -> DfResult<SqlExpr> {
    Err(DataFusionError::Execution(why))
}

/// `args` as one `MySQL` path expression — `$.a.b`, `$.items[0]` — or the member that cannot be
/// written into one, quoted for the refusal.
///
/// Every member has to be a **literal**, because the path is one string the server parses rather
/// than a chain of operators taking expressions: a column where a key belongs has no spelling
/// here at all.
fn path_expression(path: &[Expr]) -> Result<String, String> {
    let mut written = String::from("$");
    for member in path {
        match member {
            Expr::Literal(
                ScalarValue::Utf8(Some(key))
                | ScalarValue::Utf8View(Some(key))
                | ScalarValue::LargeUtf8(Some(key)),
                _,
            ) => {
                if !plain_key(key) {
                    return Err(format!("the key '{key}'"));
                }
                written.push('.');
                written.push_str(key);
            }
            Expr::Literal(ScalarValue::Int64(Some(index)), _) if *index >= 0 => {
                written.push_str(&format!("[{index}]"));
            }
            Expr::Literal(ScalarValue::UInt64(Some(index)), _) => {
                written.push_str(&format!("[{index}]"));
            }
            other => return Err(format!("'{other}'")),
        }
    }
    Ok(written)
}

/// Whether `key` is a member name a `MySQL` path can carry unquoted: an `ECMAScript` identifier,
/// narrowed to ASCII.
///
/// Narrowed because the quoted form is unavailable here (see the module docs), so this is not "may
/// it be written plainly" but "is it addressable at all" — and a rule that is too generous fails on
/// the server, while one that is too strict refuses in words naming the key.
fn plain_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The server expression `spelling` describes, over an already-unparsed `value` and a written
/// `path`.
fn spelled(spelling: Spelling, value: SqlExpr, path: &str) -> SqlExpr {
    let extract = call("JSON_EXTRACT", vec![value.clone(), literal(path)]);
    match spelling {
        Spelling::Text => call(
            "JSON_UNQUOTE",
            vec![call(
                "NULLIF",
                vec![
                    extract,
                    SqlExpr::Cast {
                        kind: CastKind::Cast,
                        expr: Box::new(literal("null")),
                        data_type: DataType::JSON,
                        array: false,
                        format: None,
                    },
                ],
            )],
        ),
        Spelling::Present => call(
            "COALESCE",
            vec![
                call(
                    "JSON_CONTAINS_PATH",
                    vec![value, literal("one"), literal(path)],
                ),
                SqlExpr::Value(ValueWithSpan {
                    value: Value::Boolean(false),
                    span: Span::empty(),
                }),
            ],
        ),
    }
}

/// One call, in the shape the unparser writes its own: no window, no filter, no argument clauses.
fn call(name: &str, args: Vec<SqlExpr>) -> SqlExpr {
    SqlExpr::Function(Function {
        name: ObjectName::from(vec![name.into()]),
        uses_odbc_syntax: false,
        parameters: FunctionArguments::None,
        args: FunctionArguments::List(FunctionArgumentList {
            duplicate_treatment: None,
            args: args
                .into_iter()
                .map(|arg| FunctionArg::Unnamed(FunctionArgExpr::Expr(arg)))
                .collect(),
            clauses: Vec::new(),
        }),
        filter: None,
        null_treatment: None,
        over: None,
        within_group: Vec::new(),
    })
}

/// One single-quoted string, which is how every path, mode word and sentinel below reaches the
/// server.
fn literal(text: &str) -> SqlExpr {
    SqlExpr::Value(ValueWithSpan {
        value: Value::SingleQuotedString(text.to_string()),
        span: Span::empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wrapper's predicate reads the **code** the driver renders, so every wording
    /// `ER_SP_DOES_NOT_EXIST` takes is covered and a relation that vanished — the catalog's own
    /// reconciliation, not this — is not.
    #[test]
    fn only_a_missing_name_is_wrapped() {
        let rendered = |state: &str, code: u16, message: &str| {
            format!(
                "Query execution failed.\nERROR {state} ({code}): {message}\nFor details, refer \
                 to the MySQL manual: https://dev.mysql.com/doc/mysql-errors/9.1/en/\
                 error-reference-introduction.html"
            )
        };
        assert!(lacks_the_name(&rendered(
            "42000",
            1305,
            "FUNCTION shop.json_get_str does not exist"
        )));
        assert!(!lacks_the_name(&rendered(
            "42S02",
            1146,
            "Table 'shop.transient' doesn't exist"
        )));
        assert!(!lacks_the_name(&rendered(
            "42000",
            1142,
            "SELECT command denied"
        )));
        assert!(
            !lacks_the_name(&rendered(
                "22032",
                3141,
                "Invalid JSON text (1305) in argument 1"
            )),
            "the code is read where the driver puts it, not anywhere it appears"
        );
    }

    /// The wrapped message keeps the server's own words and adds ours on a line of their own —
    /// the crate's last line is a bare URL, and a sentence appended to it reads as part of it.
    #[test]
    fn the_wrapper_starts_its_own_line() {
        let wrapped = remote_refusal("ERROR …\nhttps://example.invalid/docs", "my");
        assert!(wrapped.contains("docs\nThe data source 'my'"), "{wrapped}");
        assert!(wrapped.contains("CREATE TABLE"), "{wrapped}");
    }

    /// **A path is written unquoted, so only a plain key is addressable** — and the two integer
    /// forms a literal index arrives as are both indexes.
    #[test]
    fn a_path_is_members_and_indexes() {
        let key = |text: &str| Expr::Literal(ScalarValue::Utf8(Some(text.to_string())), None);
        let index = |n: i64| Expr::Literal(ScalarValue::Int64(Some(n)), None);
        assert_eq!(path_expression(&[key("a")]), Ok("$.a".to_string()));
        assert_eq!(
            path_expression(&[key("source"), key("campaign")]),
            Ok("$.source.campaign".to_string())
        );
        assert_eq!(
            path_expression(&[key("items"), index(0)]),
            Ok("$.items[0]".to_string())
        );
        assert_eq!(
            path_expression(&[Expr::Literal(ScalarValue::UInt64(Some(2)), None)]),
            Ok("$[2]".to_string())
        );
        assert_eq!(
            path_expression(&[key("first-name")]),
            Err("the key 'first-name'".to_string()),
            "a key the path can only carry in quotes is named, not approximated"
        );
        assert_eq!(path_expression(&[key("")]), Err("the key ''".to_string()));
        assert!(
            path_expression(&[index(-1)]).is_err(),
            "a negative index matches nothing locally, so it has no spelling here either"
        );
        assert!(
            path_expression(&[Expr::Column("k".into())]).is_err(),
            "a path is one string the server parses, so a column has no place in it"
        );
    }

    /// The two guards, as text — the whole of what separates this from a bare `->>` chain, and
    /// both measured against a running server (`tests/mysql_federation.rs` re-measures them).
    #[test]
    fn the_two_spellings_guard_what_the_server_answers_differently() {
        let column = SqlExpr::Identifier("p".into());
        assert_eq!(
            spelled(Spelling::Text, column.clone(), "$.a.b").to_string(),
            "JSON_UNQUOTE(NULLIF(JSON_EXTRACT(p, '$.a.b'), CAST('null' AS JSON)))"
        );
        assert_eq!(
            spelled(Spelling::Present, column, "$.a").to_string(),
            "COALESCE(JSON_CONTAINS_PATH(p, 'one', '$.a'), false)"
        );
    }

    /// A key that cannot be written into a path is not the same refusal as a call with no path at
    /// all, because the fixes differ: one is a different key, the other is a different call.
    #[test]
    fn the_two_path_refusals_say_different_things() {
        assert!(pathless_refusal("json_as_text", "my").contains("without a key to look up"));
        let why = path_refusal("json_as_text", "my", "the key 'first-name'");
        assert!(
            why.contains("'first-name'") && why.contains("plain key"),
            "{why}"
        );
    }
}
