//! The SQL **validator** (S25 / P2-18) — everything the editor squiggles.
//!
//! One entry point, [`validate`], accumulating four tiers of diagnostics:
//!
//! 1. **Lexical** — the tokenizer's own faults (unterminated string / quoted ident),
//!    unbalanced parentheses, and the keyword-typo lint (`FORM` → `FROM`).
//! 2. **Policy** — each statement goes through the statement layer's own stages
//!    ([`statements::pipeline`](crate::statements::pipeline)): its bare reads resolve against the
//!    connected databases ([`qualify`](crate::statements::pipeline::qualify), DB-09 — before the
//!    classification, and its refusals squiggle the name), then it classifies for a caller
//!    holding [`Capability::full`]. **These are the Run's own stages, not a second reading of the
//!    same rules**: a statement the editor did not underline is a statement Run is prepared to
//!    perform. Queries, introspection and the statements the engine implements itself draw no
//!    squiggle and go on to the tiers below; what is refused gets a policy diagnostic pointing at
//!    the right surface.
//! 3. **Names** — the native [`resolve`](crate::sql::resolve)r walks the parsed AST and
//!    reports **every** unknown table/column with a span (the planner below is fail-fast: one name
//!    per statement), staying quiet where a mid-edit scope is unknowable. Name faults skip the
//!    dry-plan.
//!    A statement bound for a **server** (`ddl::dispatched`) stops there: its types, functions and
//!    clauses are that server's vocabulary, so judging it here would squiggle a statement Run
//!    performs.
//! 4. **Semantic** — the allowed statements are **dry-planned** against the live `SessionContext`,
//!    then optimized for the analyzer's type coercion, so unknown functions, bad casts and the
//!    name semantics the resolver skips surface as the *same* errors a Run would hit. Nothing
//!    executes and no snapshot materializes.
//!
//! Statements are split on top-level `;` and validated independently, so one broken
//! statement never hides the others' diagnostics.

use std::cmp::Ordering;
use std::ops::Range;

use datafusion::common::diagnostic::DiagnosticKind;
use datafusion::common::{DataFusionError, SchemaError, TableReference};
use datafusion::prelude::SessionContext;
use datafusion::sql::sqlparser::parser::ParserError;

use crate::policy::{Capability, PolicyProvider, Principal};
use crate::sql::lex::{
    byte_span, is_reserved_in_name_position, lex, rel_offset, split_statements, Tok, TokKind,
};
use crate::sql::qualify::Names;
use crate::sql::resolve::resolve;
use crate::sql::FunctionCatalog;
use crate::statements::pipeline::{classify, parse_range, qualify, Admitted, Pipeline};
use strata_model::{Diagnostic, Severity};

/// Clause keywords we typo-check bare identifiers against (edit distance ≤ 1).
const CLAUSE_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "GROUP",
    "ORDER",
    "HAVING",
    "QUALIFY",
    "LIMIT",
    "OFFSET",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "NATURAL",
    "ON",
    "USING",
    "AS",
    "BY",
    "DISTINCT",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "WITH",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "ASC",
    "DESC",
];

/// Validate `sql` against the live session and return **all** diagnostics, byte-spanned
/// where the fault is localizable. Read-only over the context: statements are parsed and
/// planned, never executed (DDL only takes effect when its plan is driven).
pub async fn validate(
    p: &Pipeline<'_>,
    policy: &dyn PolicyProvider,
    functions: &FunctionCatalog,
    sql: &str,
) -> Vec<Diagnostic> {
    let ctx = p.context();
    let mut out = Vec::new();
    if sql.trim().is_empty() {
        return out;
    }

    let dialect = ctx.state_ref().read().config_options().sql_parser.dialect;

    let (toks, lex_err) = lex(sql, dialect.as_ref());
    if let Some(e) = lex_err {
        out.push(diag(Severity::Error, e.message, e.span, sql));
        return out;
    }

    check_parens(&toks, sql, &mut out);
    let hints = keyword_typo_hints(&toks, ctx, functions);

    let who = Principal::new(Capability::full());
    let state = ctx.state();
    let ranges = statement_ranges(sql, &toks);
    let last = ranges.len().saturating_sub(1);
    for (idx, stmt_range) in ranges.into_iter().enumerate() {
        let slice = &sql[stmt_range.clone()];
        let parsed = match parse_range(&state, &dialect, slice) {
            Ok(parsed) => parsed,
            Err(err) => {
                if idx == last && is_incomplete(&err, slice, &stmt_range, &toks) {
                    check_from_targets(ctx, &toks, &stmt_range, sql, &mut out);
                    continue;
                }
                let mut d = df_error_diag(&err, sql, slice, &stmt_range, &toks);
                if let Some((_, hint)) = hints
                    .iter()
                    .find(|(span, _)| d.span.as_ref().is_some_and(|s| overlaps(s, span)))
                {
                    d.message = hint.clone();
                }
                out.push(d);
                check_from_targets(ctx, &toks, &stmt_range, sql, &mut out);
                continue;
            }
        };
        let qualified = match qualify(p, parsed) {
            Ok(qualified) => qualified,
            Err(refusals) => {
                for refusal in refusals {
                    let span = refusal
                        .span
                        .and_then(|span| byte_span(slice, stmt_range.start, span))
                        .unwrap_or_else(|| leading_keywords_span(&toks, &stmt_range));
                    out.push(diag(Severity::Error, refusal.message(), span, sql));
                }
                continue;
            }
        };
        let admitted = match classify(policy, &who, qualified).await {
            Ok(admitted) => admitted,
            Err(refused) => {
                out.push(diag(
                    Severity::Error,
                    refused.message(),
                    leading_keywords_span(&toks, &stmt_range),
                    sql,
                ));
                continue;
            }
        };
        if let Admitted::Statement { kind, ref stmt } = admitted {
            if crate::ddl::dispatched(ctx, kind, stmt) {
                continue;
            }
        }
        let stmt = admitted.into_statement();
        let resolution = resolve(ctx, &stmt, slice, stmt_range.start, sql).await;
        if !resolution.diags.is_empty() {
            out.extend(resolution.diags);
            continue;
        }
        let planned = match state.statement_to_plan(stmt).await {
            Ok(plan) => state.optimize(&plan).map(|_| ()),
            Err(err) => Err(err),
        };
        if let Err(err) = planned {
            let premature = is_unresolved_column(&err) && !resolution.complete;
            if !premature {
                out.push(df_error_diag(&err, sql, slice, &stmt_range, &toks));
            }
        }
    }

    for (span, hint) in hints {
        let covered = out
            .iter()
            .any(|d| d.span.as_ref().is_some_and(|s| overlaps(s, &span)));
        if !covered {
            out.push(diag(Severity::Warning, hint, span, sql));
        }
    }
    out
}

/// Whether two byte ranges intersect.
fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// The planner failed to resolve a column reference (`Schema error: No field
/// named …`) — matched by variant, not message text.
fn is_unresolved_column(err: &DataFusionError) -> bool {
    matches!(
        err.find_root(),
        DataFusionError::SchemaError(e, _)
            if matches!(e.as_ref(), SchemaError::FieldNotFound { .. })
    )
}

/// A parse failure at end-of-input — the statement is a valid *prefix* of something,
/// i.e. incomplete rather than wrong. Structural test first: the parser's reported
/// position (the `Line: N, Column: M` suffix sqlparser appends to every parse error —
/// the same contract [`df_error_diag`] spans rely on) sits at or past the statement's
/// last token, meaning the parser consumed everything written and wanted more. A
/// message with no position falls back to sqlparser's `found: EOF` wording.
fn is_incomplete(err: &DataFusionError, slice: &str, stmt: &Range<usize>, toks: &[Tok]) -> bool {
    let msg = match err.find_root() {
        DataFusionError::SQL(pe, _) => match pe.as_ref() {
            ParserError::ParserError(s) | ParserError::TokenizerError(s) => s,
            ParserError::RecursionLimitExceeded => return false,
        },
        _ => return false,
    };
    if let Some((line, col)) = extract_line_col(msg) {
        let at = rel_offset(slice, line as u64, col as u64);
        let last_tok_end = toks
            .iter()
            .filter(|t| t.span.start >= stmt.start && t.span.end <= stmt.end)
            .map(|t| t.span.end - stmt.start)
            .max();
        return last_tok_end.is_none_or(|end| at >= end);
    }
    msg.contains("found: EOF")
}

/// Byte ranges of the token-bearing statements in `sql`, split on top-level `;`.
/// Token-level, so `;` inside strings/comments never splits, and whitespace- or
/// comment-only segments (no tokens) are dropped rather than "validated".
fn statement_ranges(sql: &str, toks: &[Tok]) -> Vec<Range<usize>> {
    split_statements(toks, sql.len())
        .into_iter()
        .filter(|r| {
            toks.iter()
                .any(|t| t.span.start >= r.start && t.span.end <= r.end)
        })
        .filter_map(|r| trim_range(sql, r))
        .collect()
}

/// Shrink `range` to its non-whitespace core; `None` if nothing is left.
fn trim_range(sql: &str, range: Range<usize>) -> Option<Range<usize>> {
    let slice = &sql[range.clone()];
    let trimmed = slice.trim_start();
    let start = range.start + (slice.len() - trimmed.len());
    let end = start + trimmed.trim_end().len();
    (start < end).then_some(start..end)
}

/// The span of a statement's leading keyword run (`CREATE EXTERNAL TABLE`,
/// `INSERT INTO`, …) — what a policy diagnostic underlines instead of the whole
/// statement.
fn leading_keywords_span(toks: &[Tok], stmt: &Range<usize>) -> Range<usize> {
    let mut in_stmt = toks
        .iter()
        .filter(|t| t.span.start >= stmt.start && t.span.end <= stmt.end);
    let Some(first) = in_stmt.next() else {
        return stmt.clone();
    };
    let mut end = first.span.end;
    for t in in_stmt.take(2) {
        if t.kind == TokKind::Keyword {
            end = t.span.end;
        } else {
            break;
        }
    }
    first.span.start..end
}

/// Token-level unknown-table check for a statement the **parser rejected**: the name
/// chains in table position (right after `FROM`/`JOIN`) are resolved against the live
/// catalog. Conservative by design — names the statement introduces itself (any ident
/// directly followed by `AS`: CTEs, aliases) and table functions (chain followed by
/// `(`) are skipped, and mixed/quoted multi-part names are left to the planner.
fn check_from_targets(
    ctx: &SessionContext,
    toks: &[Tok],
    stmt: &Range<usize>,
    sql: &str,
    out: &mut Vec<Diagnostic>,
) {
    fn is_name(t: &Tok) -> bool {
        match t.kind {
            TokKind::Ident | TokKind::QuotedIdent => true,
            TokKind::Keyword => !is_reserved_in_name_position(&t.text),
            _ => false,
        }
    }
    let stmt_toks: Vec<&Tok> = toks
        .iter()
        .filter(|t| t.span.start >= stmt.start && t.span.end <= stmt.end)
        .collect();
    let local: Vec<&str> = stmt_toks
        .windows(2)
        .filter(|w| is_name(w[0]) && w[1].kind == TokKind::Keyword && w[1].eq_ci("AS"))
        .map(|w| w[0].text.as_str())
        .collect();

    let mut i = 0;
    while i < stmt_toks.len() {
        let t = stmt_toks[i];
        i += 1;
        if t.kind != TokKind::Keyword || !(t.eq_ci("FROM") || t.eq_ci("JOIN")) {
            continue;
        }
        let mut parts: Vec<&Tok> = Vec::new();
        let mut j = i;
        while j < stmt_toks.len() && is_name(stmt_toks[j]) {
            parts.push(stmt_toks[j]);
            j += 1;
            if j < stmt_toks.len()
                && stmt_toks[j].kind == TokKind::Punct
                && stmt_toks[j].text == "."
            {
                j += 1;
            } else {
                break;
            }
        }
        if parts.is_empty() {
            continue;
        }
        if stmt_toks
            .get(j)
            .is_some_and(|t| t.kind == TokKind::Punct && t.text == "(")
        {
            continue;
        }
        if local.iter().any(|l| l.eq_ignore_ascii_case(&parts[0].text)) {
            continue;
        }
        let exists = match parts.as_slice() {
            [one] if one.kind == TokKind::QuotedIdent => {
                ctx.table_exist(TableReference::bare(one.text.clone()))
            }
            [one] => ctx.table_exist(one.text.as_str()),
            many if many.iter().all(|p| p.kind != TokKind::QuotedIdent) => {
                let name = many
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>()
                    .join(".");
                ctx.table_exist(name.as_str())
            }
            _ => continue,
        };
        if !exists.unwrap_or(true) {
            let span = parts.first().unwrap().span.start..parts.last().unwrap().span.end;
            out.push(diag(
                Severity::Error,
                format!("Table or view '{}' not found", &sql[span.clone()]),
                span,
                sql,
            ));
        }
    }
}

/// Fold a parse/plan error for the statement at `stmt` (whose text is `slice`) into a
/// byte-spanned [`Diagnostic`]. Best span first: the planner's own spanned
/// `Diagnostic` (DF 54, `collect_spans` on) → the `Line: N, Column: M` embedded in a
/// parser message → the statement's leading keywords.
fn df_error_diag(
    err: &DataFusionError,
    sql: &str,
    slice: &str,
    stmt: &Range<usize>,
    toks: &[Tok],
) -> Diagnostic {
    if let Some(d) = err.diagnostic() {
        let severity = match d.kind {
            DiagnosticKind::Error => Severity::Error,
            DiagnosticKind::Warning => Severity::Warning,
        };
        let span = d
            .span
            .map(|s| {
                let start = stmt.start + rel_offset(slice, s.start.line, s.start.column);
                let end = stmt.start + rel_offset(slice, s.end.line, s.end.column);
                widen_to_token(start..end, toks)
            })
            .unwrap_or_else(|| leading_keywords_span(toks, stmt));
        return diag(severity, d.message.clone(), span, sql);
    }

    let (mut message, parse_loc) = match err.find_root() {
        DataFusionError::SQL(pe, _) => match pe.as_ref() {
            ParserError::ParserError(s) | ParserError::TokenizerError(s) => {
                (s.clone(), extract_line_col(s))
            }
            ParserError::RecursionLimitExceeded => {
                ("Statement is too deeply nested to parse".to_string(), None)
            }
        },
        root => (root.message().into_owned(), None),
    };
    let span = match parse_loc {
        Some((line, col)) => {
            if let Some(at) = message.rfind(" at Line: ") {
                message.truncate(at);
            }
            widen_to_token(
                {
                    let at = stmt.start + rel_offset(slice, line as u64, col as u64);
                    at..at
                },
                toks,
            )
        }
        None => leading_keywords_span(toks, stmt),
    };
    diag(Severity::Error, message, span, sql)
}

/// Grow a (possibly empty) span to the full token under its start, so squiggles cover
/// the offending word rather than a single character. Leaves real ranges alone.
fn widen_to_token(span: Range<usize>, toks: &[Tok]) -> Range<usize> {
    if span.end > span.start {
        return span;
    }
    toks.iter()
        .find(|t| t.span.start <= span.start && span.start < t.span.end)
        .map(|t| t.span.clone())
        .unwrap_or(span.start..span.start + 1)
}

/// `Line: N, Column: M` from a sqlparser message, if present.
fn extract_line_col(message: &str) -> Option<(usize, usize)> {
    let at = message.rfind("Line: ")?;
    let rest = &message[at + "Line: ".len()..];
    let line: usize = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    let at = rest.find("Column: ")?;
    let rest = &rest[at + "Column: ".len()..];
    let column: usize = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some((line, column))
}

/// Unbalanced parentheses → point at the offending `(` or `)`.
fn check_parens(toks: &[Tok], sql: &str, out: &mut Vec<Diagnostic>) {
    let mut stack: Vec<Range<usize>> = Vec::new();
    for t in toks {
        if t.kind == TokKind::Punct && t.text == "(" {
            stack.push(t.span.clone());
        } else if t.kind == TokKind::Punct && t.text == ")" && stack.pop().is_none() {
            out.push(diag(
                Severity::Error,
                "Unmatched closing parenthesis".into(),
                t.span.clone(),
                sql,
            ));
        }
    }
    for open in stack {
        out.push(diag(
            Severity::Error,
            "Unclosed parenthesis".into(),
            open,
            sql,
        ));
    }
}

/// Spot bare identifiers one edit away from a clause keyword — e.g. `FORM` → `FROM` —
/// and return them as `(span, message)` hints. High-confidence only: an identifier
/// that resolves as a table or registered function is never second-guessed. The
/// caller decides how each hint surfaces: merged into an overlapping parse error's
/// message, dropped under a better engine error, or a standalone warning.
///
/// **"Resolves" is `qualify::resolves`, not the workspace's own catalog** (DB-09): asking the
/// narrower question squiggled `SELECT * FROM orders` — a query that runs — as an unknown word one
/// edit from `ORDER`, which only the *table not found* error over the same span had been hiding.
fn keyword_typo_hints(
    toks: &[Tok],
    ctx: &SessionContext,
    functions: &FunctionCatalog,
) -> Vec<(Range<usize>, String)> {
    /// A token usable as a name — what an alias or a typo'd keyword's *operand*
    /// looks like.
    fn name_like(t: Option<&Tok>) -> bool {
        t.is_some_and(|t| match t.kind {
            TokKind::Ident | TokKind::QuotedIdent => true,
            TokKind::Keyword => !is_reserved_in_name_position(&t.text),
            _ => false,
        })
    }
    let names = Names::of(ctx);
    let mut hints = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.kind != TokKind::Ident || t.text.len() < 2 {
            continue;
        }
        if names.resolves(&t.text) || functions.contains(&t.text) {
            continue;
        }
        if name_like(i.checked_sub(1).and_then(|p| toks.get(p))) && !name_like(toks.get(i + 1)) {
            continue;
        }
        let dot = |t: Option<&Tok>| t.is_some_and(|t| t.kind == TokKind::Punct && t.text == ".");
        if dot(toks.get(i + 1)) || dot(i.checked_sub(1).and_then(|p| toks.get(p))) {
            continue;
        }
        let up = t.text.to_ascii_uppercase();
        if CLAUSE_KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(&up)) {
            continue;
        }
        if let Some(kw) = CLAUSE_KEYWORDS.iter().find(|k| near_keyword(&up, k)) {
            hints.push((
                t.span.clone(),
                format!("Unknown keyword '{}'. Did you mean '{}'?", t.text, kw),
            ));
        }
    }
    hints
}

pub(crate) fn diag(
    severity: Severity,
    message: String,
    span: Range<usize>,
    sql: &str,
) -> Diagnostic {
    Diagnostic {
        severity,
        message,
        loc: Some(line_col(sql, span.start)),
        span: Some(span),
    }
}

/// 1-based `line L:C` label for a byte offset (Problems row display).
fn line_col(sql: &str, offset: usize) -> String {
    let off = offset.min(sql.len());
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, ch) in sql.char_indices() {
        if i >= off {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    format!("line {line}:{col}")
}

/// A likely typo of a keyword: differs by ≤1 insert/delete/substitute **or** a single
/// adjacent transposition (Damerau) — so `FORM`→`FROM` (a swap = 2 substitutions) is
/// caught. Case-insensitive; inputs already upper-cased.
fn near_keyword(a: &str, b: &str) -> bool {
    let (av, bv): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if av == bv {
        return false;
    }
    edit_distance_at_most_1(&av, &bv) || adjacent_transposition(&av, &bv)
}

/// ≤1 insertion/deletion/substitution via a two-pointer walk.
fn edit_distance_at_most_1(a: &[char], b: &[char]) -> bool {
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0usize, 0usize, 0u8);
    while i < la && j < lb {
        if a[i].eq_ignore_ascii_case(&b[j]) {
            i += 1;
            j += 1;
        } else {
            if edits == 1 {
                return false;
            }
            edits += 1;
            match la.cmp(&lb) {
                Ordering::Greater => i += 1,
                Ordering::Less => j += 1,
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
            }
        }
    }
    true
}

/// Exactly one adjacent swap (same length, two neighbouring positions swapped).
fn adjacent_transposition(a: &[char], b: &[char]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff: Vec<usize> = (0..a.len())
        .filter(|&i| !a[i].eq_ignore_ascii_case(&b[i]))
        .collect();
    diff.len() == 2
        && diff[1] == diff[0] + 1
        && a[diff[0]].eq_ignore_ascii_case(&b[diff[1]])
        && a[diff[1]].eq_ignore_ascii_case(&b[diff[0]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::CapabilityPolicyProvider;
    use crate::statements::pipeline::policy_verdicts;
    use crate::statements::Fault;
    use datafusion::arrow::array::{Int64Array, StringArray};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::prelude::SessionConfig;
    use futures::executor::block_on;
    use std::sync::Arc;

    /// A context shaped like the engine's: `collect_spans` on, one table `t(id, name)`.
    fn ctx() -> SessionContext {
        let mut config = SessionConfig::new();
        config.options_mut().sql_parser.collect_spans = true;
        let ctx = SessionContext::new_with_config(config);
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1_i64, 2])),
                Arc::new(StringArray::from(vec!["a", "b"])),
            ],
        )
        .unwrap();
        ctx.register_batch("t", batch).unwrap();
        ctx
    }

    /// An engine nobody restricted — the policy behind the editor's own capability.
    fn policy() -> CapabilityPolicyProvider {
        CapabilityPolicyProvider::new(Capability::full())
    }

    /// Every diagnostic `sql` draws against `ctx`.
    fn check(ctx: &SessionContext, sql: &str) -> Vec<Diagnostic> {
        let policy = policy();
        block_on(validate(
            &Pipeline::new(ctx),
            &policy,
            &FunctionCatalog::default(),
            sql,
        ))
    }

    fn run(sql: &str) -> Vec<Diagnostic> {
        check(&ctx(), sql)
    }

    fn spanned<'a>(sql: &'a str, d: &Diagnostic) -> &'a str {
        &sql[d.span.clone().expect("diagnostic span")]
    }

    #[test]
    fn valid_statements_produce_no_diagnostics() {
        assert!(run("SELECT id, name FROM t WHERE id > 1 ORDER BY name").is_empty());
        assert!(run("SELECT * FROM t; EXPLAIN SELECT id FROM t;").is_empty());
        assert!(run("").is_empty());
        assert!(run("-- just a comment").is_empty());
    }

    #[test]
    fn unknown_table_is_spanned() {
        let sql = "SELECT * FROM nope";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_error());
        assert!(out[0].message.contains("nope"), "{}", out[0].message);
        assert_eq!(spanned(sql, &out[0]), "nope");
    }

    #[test]
    fn unknown_column_is_spanned() {
        let sql = "SELECT missing FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert_eq!(spanned(sql, &out[0]), "missing");
    }

    #[test]
    fn cte_drafts_keep_the_no_from_grace() {
        assert!(run("WITH x AS (SELECT id FROM t) SELECT draft_col").is_empty());
    }

    #[test]
    fn columns_before_from_stay_quiet() {
        assert!(run("SELECT name, tags").is_empty());
        assert!(run("SELECT missing").is_empty());
        assert!(!run("SELECT nosuchfn(1)").is_empty());
        assert!(run("SELECT 1 + 2").is_empty());
    }

    /// The base context plus a registered view `v` over `t` — the Save flow's result.
    fn ctx_with_view() -> SessionContext {
        let ctx = ctx();
        let df = block_on(ctx.sql("CREATE VIEW v AS SELECT id, name FROM t")).expect("create view");
        block_on(df.collect()).expect("apply view");
        ctx
    }

    #[test]
    fn views_resolve_like_tables() {
        let ctx = ctx_with_view();
        assert!(check(&ctx, "SELECT id FROM v").is_empty());
        let sql = "SELECT missing FROM v";
        let out = check(&ctx, sql);
        assert_eq!(out.len(), 1);
        assert_eq!(spanned(sql, &out[0]), "missing");
        let out = check(&ctx, "selct id from v");
        assert!(
            !out.iter().any(|d| d.message.contains("not found")),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unknown_function_errors() {
        let out = run("SELECT not_a_function(id) FROM t");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("not_a_function"),
            "{}",
            out[0].message
        );
    }

    #[test]
    fn function_arity_is_checked() {
        let out = run("SELECT upper() FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());

        let out = run("SELECT upper(name, id) FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
    }

    #[test]
    fn function_argument_types_are_checked() {
        let out = run("SELECT array_length(id) FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());

        assert!(run("SELECT character_length(name) FROM t").is_empty());
    }

    #[test]
    fn expression_type_faults_are_checked() {
        let out = run("SELECT name + INTERVAL '1 day' FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
    }

    #[test]
    fn bad_cast_errors() {
        let out = run("SELECT CAST(id AS notatype) FROM t");
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
    }

    #[test]
    fn statements_accumulate_independently() {
        let sql = "SELECT * FROM nope; SELECT missing FROM t; SELECT id FROM t";
        let out = run(sql);
        assert_eq!(
            out.len(),
            2,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert_eq!(spanned(sql, &out[0]), "nope");
        assert_eq!(spanned(sql, &out[1]), "missing");
    }

    #[test]
    fn syntax_error_is_located() {
        let sql = "SELECT id FROM t WHERE AND id = 1";
        let out = run(sql);
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].span.is_some());
        assert!(out[0].is_error());
    }

    #[test]
    fn trailing_incomplete_statement_stays_quiet() {
        assert!(run("select").is_empty());
        assert!(run("SELECT id FROM t WHERE").is_empty());
        assert!(run("SELECT id FROM t ORDER BY").is_empty());

        let sql = "SELECT id FROM t WHERE; SELECT id FROM t";
        let out = run(sql);
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
    }

    #[test]
    fn unterminated_string_reports_lex_error() {
        let out = run("SELECT 'abc FROM t");
        assert_eq!(out.len(), 1);
        assert!(out[0].is_error());
    }

    #[test]
    fn broken_statement_still_flags_unknown_from_target() {
        let sql = "selct * from nope";
        let out = run(sql);
        assert_eq!(
            out.len(),
            2,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(
            out.iter().any(|d| d.is_error()
                && d.message.contains("Did you mean 'SELECT'")
                && spanned(sql, d) == "selct"),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(
            out.iter().any(|d| d.is_error()
                && d.message.contains("not found")
                && spanned(sql, d) == "nope"),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn from_target_fallback_stays_conservative() {
        let sql = "selct * from t";
        assert!(!run(sql).iter().any(|d| d.message.contains("not found")));
        let sql = "selct * from public.t";
        assert!(!run(sql).iter().any(|d| d.message.contains("not found")));
        let sql = "WITH x AS (SELCT 1) SELECT * FROM x";
        assert!(
            !run(sql).iter().any(|d| d.message.contains("not found")),
            "CTE name must not be flagged"
        );
        let sql = "selct * from read_parquet('f.parquet')";
        assert!(!run(sql).iter().any(|d| d.message.contains("not found")));
    }

    #[test]
    fn keyword_like_table_names_are_still_checked() {
        let sql = "selc * from event";
        let out = run(sql);
        assert!(
            out.iter()
                .any(|d| d.message.contains("not found") && spanned(sql, d) == "event"),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );

        let ctx = ctx();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1_i64]))],
        )
        .unwrap();
        ctx.register_batch("event", batch).unwrap();
        let out = check(&ctx, sql);
        assert!(
            !out.iter().any(|d| d.message.contains("not found")),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn keyword_typo_merges_into_the_parse_error() {
        let sql = "SELECT * FORM t";
        let out = run(sql);
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
        assert!(
            out[0].message.contains("Did you mean 'FROM'"),
            "{}",
            out[0].message
        );
        assert_eq!(spanned(sql, &out[0]), "FORM");
    }

    #[test]
    fn typo_hint_defers_to_a_better_engine_error() {
        let sql = "SELECT fom FROM t";
        let out = run(sql);
        assert_eq!(
            out.len(),
            1,
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert!(out[0].is_error());
        assert_eq!(spanned(sql, &out[0]), "fom");
    }

    #[test]
    fn the_editor_squiggles_what_the_router_still_refuses() {
        let out = run("CREATE DATABASE other");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].message, Fault::CreateDatabase.message());

        let sql = "TRUNCATE TABLE t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].message, Fault::Unsupported.message());
        assert_eq!(spanned(sql, &out[0]), "TRUNCATE TABLE");

        let out = run("INSERT OVERWRITE INTO t VALUES (3, 'c')");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].message, Fault::InsertOverwrite.message());

        let out = run("PREPARE p AS INSERT INTO t VALUES (3, 'c')");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].message, Fault::PrepareNonQuery.message());
    }

    /// The statements the editor now runs itself draw no policy squiggle — the whole
    /// point of the `Intercept` verdict, and the thing a `Refuse` used to hide.
    #[test]
    fn an_intercepted_statement_gets_no_squiggle() {
        for sql in [
            "CREATE EXTERNAL TABLE x STORED AS PARQUET LOCATION 'f.parquet'",
            "CREATE TABLE copy_t AS SELECT * FROM t",
            "CREATE TABLE cols (id BIGINT)",
            "INSERT INTO t VALUES (3, 'c')",
            "DROP TABLE t",
            "CREATE VIEW v AS SELECT id FROM t",
            "DROP VIEW IF EXISTS v",
            "COPY t TO 'out.parquet'",
            "SET datafusion.execution.batch_size = 1024",
            "RESET datafusion.execution.batch_size",
            "PREPARE p AS SELECT id FROM t",
            "DEALLOCATE p",
        ] {
            let out = run(sql);
            assert!(out.is_empty(), "{sql}: {out:?}");
        }
    }

    /// Interception is not a bypass: an intercepted statement falls through to the
    /// name and semantic tiers exactly as a query does, so typed DDL gets the same
    /// unknown-table diagnostic a `SELECT` would.
    #[test]
    fn an_intercepted_statement_still_gets_name_diagnostics() {
        let out = run("CREATE TABLE copy_t AS SELECT * FROM nope");
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].is_error());
        assert!(
            out[0].message.to_lowercase().contains("nope"),
            "{}",
            out[0].message
        );
    }

    /// Reserved names, both halves: a `__snap_` identifier in a statement the editor
    /// would run itself is refused before it can collide with a live snapshot
    /// registration — which fails as "already exists", on a name the same prefix keeps
    /// invisible.
    #[test]
    fn a_snapshot_name_is_refused_in_any_statement() {
        for sql in [
            "CREATE EXTERNAL TABLE __snap_2 STORED AS PARQUET LOCATION 'f.parquet'",
            "CREATE TABLE __snap_2 AS SELECT * FROM t",
            "CREATE TABLE __SNAP_2 (id BIGINT)",
            "CREATE VIEW __snap_2 AS SELECT id FROM t",
            "INSERT INTO __snap_2 VALUES (3, 'c')",
            "DROP TABLE __snap_2",
            "DROP VIEW __snap_2",
            "CREATE TABLE mine AS SELECT * FROM __snap_3",
            "COPY (SELECT * FROM __snap_3) TO 'out.parquet'",
            "COPY __snap_3 TO 'out.parquet'",
            "SELECT 1 FROM __snap_3",
            "SELECT * FROM __snap_3",
            "EXPLAIN SELECT * FROM __snap_3",
        ] {
            let out = run(sql);
            assert_eq!(out.len(), 1, "{sql}: {out:?}");
            assert_eq!(out[0].message, Fault::ReservedName.message(), "{sql}");
        }
    }

    /// **The prefix is a namespace, and the namespace is the workspace catalog's.** A `__snap_`
    /// name qualified into a database connection's catalog is a relation somebody else named, so
    /// reading it is ordinary and writing to it is refused for being remote rather than reserved.
    ///
    /// Deliberately **syntactic**: nothing asks whether `pg` is registered, because [`classify`] is
    /// a pure function of the parsed statement. A qualifier naming no catalog resolves nowhere, and
    /// the two arms that could care already say so.
    ///
    /// The second half holds the other direction, and **the quoted spellings are the ones that
    /// bite**: the catalog list resolves by `fold_ident`, so a raw compare reads `"STRATA"` as
    /// somewhere else and let `SELECT * FROM "STRATA".public.__snap_3` hand back another tab's
    /// snapshot. The unquoted spellings could not have caught it, since the parser folds those
    /// first.
    #[test]
    fn the_reserved_namespace_is_the_workspace_catalog() {
        for sql in [
            "SELECT * FROM pg.public.__snap_3",
            "SELECT * FROM pg.analytics.__snap_3",
            "EXPLAIN SELECT * FROM pg.public.__snap_3",
        ] {
            let out = run(sql);
            assert!(
                out.iter()
                    .all(|d| d.message != Fault::ReservedName.message()),
                "{sql}: {out:?}"
            );
        }
        for sql in [
            "SELECT * FROM public.__snap_3",
            "SELECT * FROM strata.public.__snap_3",
            "DROP TABLE strata.public.__snap_3",
            "SELECT * FROM STRATA.PUBLIC.__SNAP_3",
            "SELECT * FROM \"STRATA\".public.__snap_3",
            "SELECT * FROM \"strata\".\"public\".\"__snap_3\"",
        ] {
            let out = run(sql);
            assert_eq!(out[0].message, Fault::ReservedName.message(), "{sql}");
        }
    }

    /// **The zero-copies claim, made executable.** For every form both surfaces refuse, the
    /// agent gate renders byte-for-byte the message the editor's diagnostic shows for the same
    /// SQL — one pipeline, one message table, two consumers. The divergences between the two
    /// capabilities are pinned where they are decided (`statements::pipeline`'s parity matrix);
    /// this is the half that says the *diagnostics* pass reads the same table.
    #[test]
    fn the_gate_and_the_editor_refuse_with_the_same_words() {
        let ctx = ctx();
        let policy = policy();
        let pipeline = Pipeline::new(&ctx);
        let agent = Principal::new(Capability::read_only());
        for sql in [
            "CREATE DATABASE other",
            "CREATE SCHEMA other",
            "DROP SCHEMA s",
            "TRUNCATE TABLE t",
            "MERGE INTO t USING u ON t.id = u.id WHEN MATCHED THEN DELETE",
        ] {
            let verdicts =
                block_on(policy_verdicts(&pipeline, &policy, &agent, sql)).expect("parses");
            assert_eq!(verdicts.len(), 1, "{sql}");
            assert_eq!(verdicts[0].message(), check(&ctx, sql)[0].message, "{sql}");
        }
    }

    /// The dry-plan reaches typed DDL now that the editor intercepts it rather than
    /// refusing it — and planning a DDL statement must still build its node without
    /// executing it (execution lives only in `execute_logical_plan`). This is the
    /// pin on that: the tier-4 pass runs, reports nothing, and creates nothing.
    #[test]
    fn validation_never_mutates_the_session() {
        let ctx = ctx();
        for sql in [
            "CREATE VIEW v AS SELECT id FROM t",
            "CREATE TABLE made AS SELECT id FROM t",
            "DROP TABLE t",
        ] {
            let out = check(&ctx, sql);
            assert!(out.is_empty(), "{sql}: {out:?}");
        }
        assert!(!ctx.table_exist("v").unwrap());
        assert!(!ctx.table_exist("made").unwrap());
        assert!(ctx.table_exist("t").unwrap());
    }

    #[test]
    fn unbalanced_parens_are_flagged() {
        let sql = "SELECT sum(id FROM t";
        let out = run(sql);
        assert!(out.iter().any(|d| d.message.contains("Unclosed")));
    }

    fn messages(out: &[Diagnostic]) -> Vec<&str> {
        out.iter().map(|d| d.message.as_str()).collect()
    }

    #[test]
    fn multiple_unknown_columns_all_squiggle() {
        let sql = "SELECT nme, product_idd FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 2, "{:?}", messages(&out));
        assert!(out.iter().all(Diagnostic::is_error));
        assert_eq!(spanned(sql, &out[0]), "nme");
        assert_eq!(spanned(sql, &out[1]), "product_idd");
    }

    #[test]
    fn unknown_table_mutes_its_columns() {
        let sql = "SELECT missing FROM nope";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "nope");
    }

    #[test]
    fn qualified_unknown_columns_are_spanned() {
        let sql = "SELECT t.missing FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "t.missing");

        let sql = "SELECT e.missing FROM t e";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert_eq!(spanned(sql, &out[0]), "e.missing");

        assert!(run("SELECT e.id FROM t e").is_empty());
    }

    #[test]
    fn unknown_qualifier_is_flagged() {
        let sql = "SELECT x.id FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "x.id");
    }

    #[test]
    fn cte_columns_resolve() {
        let sql = "WITH c AS (SELECT id FROM t) SELECT missing FROM c";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "missing");

        assert!(run("WITH c AS (SELECT id FROM t) SELECT id FROM c").is_empty());
    }

    #[test]
    fn cte_body_columns_are_checked() {
        let sql = "WITH c AS (SELECT missing FROM t) SELECT draft";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "missing");
    }

    #[test]
    fn derived_table_columns_resolve() {
        let sql = "SELECT d.missing FROM (SELECT id FROM t) d";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "d.missing");

        let sql = "SELECT d.missing FROM (SELECT * FROM t) d";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert_eq!(spanned(sql, &out[0]), "d.missing");

        assert!(run("SELECT d.x FROM (SELECT id + 1 FROM t) d").is_empty());
    }

    #[test]
    fn set_op_branches_each_checked() {
        let sql = "SELECT nme FROM t UNION ALL SELECT idd FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 2, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "nme");
        assert_eq!(spanned(sql, &out[1]), "idd");
    }

    #[test]
    fn correlated_subqueries_see_the_outer_scope() {
        assert!(
            run("SELECT id FROM t WHERE EXISTS (SELECT 1 FROM t u WHERE u.id = t.id)").is_empty()
        );

        let sql = "SELECT id FROM t WHERE EXISTS (SELECT 1 FROM t u WHERE u.missing = t.id)";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "u.missing");
    }

    #[test]
    fn select_aliases_are_legal_in_order_and_group() {
        assert!(run("SELECT id AS a FROM t ORDER BY a").is_empty());
        assert!(run("SELECT id FROM t GROUP BY 1").is_empty());

        let sql = "SELECT id AS a FROM t ORDER BY missing";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "missing");
    }

    #[test]
    fn name_faults_defer_type_faults() {
        let sql = "SELECT nme, name + INTERVAL '1 day' FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "nme");
        assert!(!run("SELECT name + INTERVAL '1 day' FROM t").is_empty());
    }

    #[test]
    fn dangling_join_stays_quiet() {
        assert!(run("SELECT id FROM t JOIN").is_empty());
        assert!(run("SELECT id FROM t LEFT JOIN").is_empty());
    }

    #[test]
    fn half_written_cte_stays_quiet() {
        assert!(run("WITH x AS (SELECT id FROM t)").is_empty());
        assert!(run("WITH x AS (SELECT id FROM t) SELECT").is_empty());
    }

    #[test]
    fn ambiguity_is_still_engine_authoritative() {
        let ctx = ctx();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, true),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1_i64])),
                Arc::new(StringArray::from(vec!["x"])),
            ],
        )
        .unwrap();
        ctx.register_batch("t2", batch).unwrap();
        let out = check(&ctx, "SELECT name FROM t JOIN t2 ON t.id = t2.id");
        assert_eq!(out.len(), 1, "{:?}", messages(&out));
        assert!(
            out[0].message.to_lowercase().contains("ambiguous"),
            "{}",
            out[0].message
        );
    }

    #[test]
    fn views_get_multi_error_parity() {
        let ctx = ctx_with_view();
        let sql = "SELECT nme, idd FROM v";
        let out = check(&ctx, sql);
        assert_eq!(out.len(), 2, "{:?}", messages(&out));
        assert_eq!(spanned(sql, &out[0]), "nme");
        assert_eq!(spanned(sql, &out[1]), "idd");
    }

    #[test]
    fn aliases_near_keywords_are_not_second_guessed() {
        assert!(run("SELECT od.id FROM t od WHERE od.id > 0").is_empty());
        assert!(run("SELECT id AS od FROM t").is_empty());
    }

    #[test]
    fn incompleteness_is_positional_not_textual() {
        assert!(run("SELECT id FROM t WHERE").is_empty());
        assert!(!run("SELECT id FROM t WHERE ORDER").is_empty());
    }
}
