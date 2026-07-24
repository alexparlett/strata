//! The SQL **validator** (S25 / P2-18) — everything the editor squiggles.
//!
//! One entry point, [`validate`], accumulating three tiers of diagnostics:
//!
//! 1. **Lexical** — the tokenizer's own faults (unterminated string / quoted ident),
//!    unbalanced parentheses, and the keyword-typo lint (`FORM` → `FROM`).
//! 2. **Policy** — each statement is parsed with DataFusion's own `DFParser` (via
//!    [`SessionState::sql_to_statement`]) and classified against the managed-DDL
//!    policy: only queries / `EXPLAIN` / `SHOW` / `DESCRIBE` pass. Everything else —
//!    `CREATE`/`DROP VIEW` (views are Save's artifact: ⌘S wraps the plain query in
//!    `CREATE OR REPLACE VIEW` itself), `CREATE EXTERNAL TABLE`, CTAS, `INSERT`,
//!    `COPY`, `SET`, `CREATE DATABASE`/`SCHEMA` and other DDL/DML — gets a policy
//!    diagnostic pointing at the right surface instead of a confusing engine error.
//! 3. **Semantic** — the allowed statements are **dry-planned** against the live
//!    `SessionContext` ([`SessionState::statement_to_plan`], then
//!    [`SessionState::optimize`] for the analyzer's type coercion): unknown
//!    tables/views/columns/functions, bad casts and un-coercible expressions surface
//!    as the *same* errors a Run would hit — zero drift, nothing executes and no
//!    snapshot materializes. DF 54 attaches spanned [`Diagnostic`]s to resolution
//!    errors (the engine enables `collect_spans`), which map straight onto squiggles.
//!
//! Statements are split on top-level `;` and validated independently, so one broken
//! statement never hides the others' diagnostics.

use std::ops::Range;

use datafusion::common::{DataFusionError, TableReference};
use datafusion::prelude::SessionContext;
use datafusion::sql::parser::Statement as DFStatement;
use datafusion::sql::sqlparser::ast::{ObjectType, Statement as SqlStatement};
use datafusion::sql::sqlparser::parser::ParserError;

use crate::engine::sql::lex::{lex, Tok, TokKind};
use crate::engine::sql::FunctionCatalog;
use strata_model::{DiagSource, Diagnostic, Severity};

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
    ctx: &SessionContext,
    functions: &FunctionCatalog,
    sql: &str,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if sql.trim().is_empty() {
        return out;
    }

    let (toks, lex_err) = lex(sql);
    if let Some(e) = lex_err {
        out.push(diag(Severity::Error, e.message, e.span, sql));
        // A tokenizer failure means splitting/planning would misread the text.
        return out;
    }

    check_parens(&toks, sql, &mut out);
    let hints = keyword_typo_hints(&toks, ctx, functions);

    let state = ctx.state();
    let dialect = state.config_options().sql_parser.dialect;
    let ranges = statement_ranges(sql, &toks);
    let last = ranges.len().saturating_sub(1);
    for (idx, stmt_range) in ranges.into_iter().enumerate() {
        let slice = &sql[stmt_range.clone()];
        let stmt = match state.sql_to_statement(slice, &dialect) {
            Ok(stmt) => stmt,
            Err(err) => {
                // A trailing statement that fails at end-of-input is a valid *prefix*
                // — the user is mid-thought, not mistaken. Stay quiet (Run still
                // rejects it); an incomplete statement *followed by* another one is a
                // real fault and keeps its error. Name checks below run either way.
                if idx == last && is_incomplete(&err) {
                    check_from_targets(ctx, &toks, &stmt_range, sql, &mut out);
                    continue;
                }
                let mut d = df_error_diag(&err, sql, slice, &stmt_range, &toks);
                // When the parser choked on a token that reads as a keyword typo, the
                // hint is the better wording of the same fault — one diagnostic, not
                // an error and a warning stacked on the same span.
                if let Some((_, hint)) = hints.iter().find(|(span, _)| {
                    d.span.as_ref().is_some_and(|s| overlaps(s, span))
                }) {
                    d.message = hint.clone();
                }
                out.push(d);
                // The statement didn't parse, so the planner never resolved names —
                // best-effort check the FROM/JOIN targets against the catalog so a
                // broken keyword doesn't hide an unknown table. (When the parse
                // succeeds, the planner is the authority and this never runs.)
                check_from_targets(ctx, &toks, &stmt_range, sql, &mut out);
                continue;
            }
        };
        if let Some(policy) = policy_block(&stmt) {
            out.push(diag(
                Severity::Error,
                policy,
                leading_keywords_span(&toks, &stmt_range),
                sql,
            ));
            continue;
        }
        let planned = match state.statement_to_plan(stmt).await {
            // The analyzer pass (type coercion, subquery checks) only runs in
            // `optimize` — it's what catches statically-bad casts and expressions.
            Ok(plan) => state.optimize(&plan).map(|_| ()),
            Err(err) => Err(err),
        };
        if let Err(err) = planned {
            // `SELECT name, tags` with no FROM written yet resolves every column
            // against an empty schema — "column not found" there is premature, not
            // wrong (the same valid-prefix stance as the incomplete trailing
            // statement above). Unresolved-column errors stay quiet until the
            // statement has a FROM to resolve against; everything else (unknown
            // functions, bad casts, policy) still surfaces, and a Run of a
            // FROM-less projection reports the real engine error in the results.
            let premature = is_unresolved_column(&err) && !has_from(&toks, &stmt_range);
            if !premature {
                out.push(df_error_diag(&err, sql, slice, &stmt_range, &toks));
            }
        }
    }

    // A hint standing on its own becomes a warning; one overlapping any error is
    // redundant (the parse arm already took its wording, or the engine's message —
    // e.g. an unknown-column error on the same token — says it better).
    for (span, hint) in hints {
        let covered = out.iter().any(|d| d.span.as_ref().is_some_and(|s| overlaps(s, &span)));
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
            if matches!(e.as_ref(), datafusion::common::SchemaError::FieldNotFound { .. })
    )
}

/// Whether the statement's **main query** has a `FROM` — resolution context for its
/// column references. Paren depth 0 only: a FROM inside a CTE body or subquery
/// resolves *that* scope, not the outer projection (`WITH x AS (… FROM t) SELECT
/// draft|` is still a FROM-less draft and keeps the mid-edit grace).
fn has_from(toks: &[Tok], stmt: &Range<usize>) -> bool {
    let mut depth = 0i32;
    for t in toks {
        if t.span.start < stmt.start || t.span.end > stmt.end {
            continue;
        }
        match t.kind {
            TokKind::Punct if t.text == "(" => depth += 1,
            TokKind::Punct if t.text == ")" => depth -= 1,
            TokKind::Keyword if depth == 0 && t.eq_ci("FROM") => return true,
            _ => {}
        }
    }
    false
}

/// A parse failure at end-of-input — the statement is a valid *prefix* of something,
/// i.e. incomplete rather than wrong (sqlparser reports the offending token as `EOF`).
fn is_incomplete(err: &DataFusionError) -> bool {
    match err.find_root() {
        DataFusionError::SQL(pe, _) => match pe.as_ref() {
            ParserError::ParserError(s) | ParserError::TokenizerError(s) => {
                s.contains("found: EOF")
            }
            ParserError::RecursionLimitExceeded => false,
        },
        _ => false,
    }
}

// ---- statement split -------------------------------------------------------

/// Byte ranges of the token-bearing statements in `sql`, split on top-level `;`.
/// Token-level, so `;` inside strings/comments never splits, and whitespace- or
/// comment-only segments (no tokens) are dropped rather than "validated".
fn statement_ranges(sql: &str, toks: &[Tok]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for t in toks {
        if t.kind == TokKind::Punct && t.text == ";" {
            ranges.push(start..t.span.start);
            start = t.span.end;
        }
    }
    ranges.push(start..sql.len());
    ranges
        .into_iter()
        .filter(|r| toks.iter().any(|t| t.span.start >= r.start && t.span.end <= r.end))
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

// ---- managed-DDL policy ----------------------------------------------------

/// The managed-DDL policy verdict for one parsed statement: `None` = the editor may
/// run (or Save may capture) it; `Some(message)` = blocked, with the message naming
/// the surface that owns the capability. Matching the *parsed* statement keeps this a
/// general classification, not a leading-keyword sniff.
fn policy_block(stmt: &DFStatement) -> Option<String> {
    let s = match stmt {
        DFStatement::CreateExternalTable(_) => {
            return Some(
                "CREATE EXTERNAL TABLE is not supported in the editor. Register tables in \
                 Table Config"
                    .into(),
            );
        }
        DFStatement::CopyTo(_) => {
            return Some("COPY TO is not supported in the editor. Use Export".into());
        }
        DFStatement::Reset(_) => {
            return Some(
                "RESET is not supported in the editor. Engine options are set in Settings".into(),
            );
        }
        DFStatement::Explain(_) => return None,
        DFStatement::Statement(s) => s.as_ref(),
    };
    match s {
        // Runnable: queries + introspection.
        SqlStatement::Query(_)
        | SqlStatement::Explain { .. }
        | SqlStatement::ExplainTable { .. }
        | SqlStatement::ShowTables { .. }
        | SqlStatement::ShowColumns { .. }
        | SqlStatement::ShowFunctions { .. }
        | SqlStatement::ShowVariable { .. }
        | SqlStatement::ShowVariables { .. }
        | SqlStatement::ShowDatabases { .. }
        | SqlStatement::ShowSchemas { .. } => None,
        // Views are Save's artifact, not the editor's: ⌘S / Save-as-view wraps the
        // *plain query* in `CREATE OR REPLACE VIEW` itself, so typed view DDL would
        // nest DDL inside the wrapper — block it toward the real flow.
        SqlStatement::CreateView(_) => Some(
            "CREATE VIEW is not supported in the editor. Write the query and use Save as view"
                .into(),
        ),
        SqlStatement::Drop { object_type, .. } => match object_type {
            ObjectType::View => Some(
                "DROP VIEW is not supported in the editor. Drop views from the catalog".into(),
            ),
            _ => Some(
                "DROP is not supported in the editor. Deregister tables from the catalog".into(),
            ),
        },
        SqlStatement::CreateTable(_) => Some(
            "CREATE TABLE is not supported in the editor. Register tables in Table Config".into(),
        ),
        SqlStatement::Insert(_) => {
            Some("INSERT is not supported in the editor. Load data through Table Config".into())
        }
        SqlStatement::CreateDatabase { .. } | SqlStatement::CreateSchema { .. } => {
            Some("CREATE DATABASE and CREATE SCHEMA are not supported".into())
        }
        SqlStatement::Set(_) => Some(
            "SET is not supported in the editor. Engine options are set in Settings".into(),
        ),
        _ => Some(
            "This statement is not supported in the editor. Only SELECT, EXPLAIN, SHOW and \
             DESCRIBE can run here"
                .into(),
        ),
    }
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
    // A token usable as a table name. sqlparser classes every word in its keyword
    // dictionary as a keyword — including non-reserved ones that are perfectly
    // legal table names (`event`, `user`, `day`, …) — so keyword tokens count as
    // names here too, except the clause keywords that actually end a FROM item.
    fn is_name(t: &Tok) -> bool {
        match t.kind {
            TokKind::Ident | TokKind::QuotedIdent => true,
            TokKind::Keyword => !CLAUSE_KEYWORDS.iter().any(|k| t.eq_ci(k)),
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
        // The dotted name chain right after the clause keyword.
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
        // A table function call, not a table name.
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
            // A quoted name resolves exactly; `bare` skips the parse-and-normalize.
            [one] if one.kind == TokKind::QuotedIdent => {
                ctx.table_exist(TableReference::bare(one.text.clone()))
            }
            [one] => ctx.table_exist(one.text.as_str()),
            many if many.iter().all(|p| p.kind != TokKind::QuotedIdent) => {
                let name = many.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(".");
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

// ---- engine error → diagnostic ---------------------------------------------

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
            datafusion::common::diagnostic::DiagnosticKind::Error => Severity::Error,
            datafusion::common::diagnostic::DiagnosticKind::Warning => Severity::Warning,
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
            // The location is part of the span now — drop the noisy suffix.
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

/// Byte offset of 1-based (`line`, `column`) within `slice` (clamped to its end).
/// Columns count characters; for ASCII SQL that's exact, non-ASCII is approximate —
/// same convention as the tokenizer mapping in [`lex`].
fn rel_offset(slice: &str, line: u64, column: u64) -> usize {
    let line = (line.max(1) - 1) as usize;
    let column = (column.max(1) - 1) as usize;
    let base = slice
        .split_inclusive('\n')
        .take(line)
        .map(|l| l.len())
        .sum::<usize>();
    (base + column).min(slice.len())
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
    let line: usize = rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()?;
    let at = rest.find("Column: ")?;
    let rest = &rest[at + "Column: ".len()..];
    let column: usize = rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()?;
    Some((line, column))
}

// ---- lexical / structural tier ----------------------------------------------

/// Unbalanced parentheses → point at the offending `(` or `)`.
fn check_parens(toks: &[Tok], sql: &str, out: &mut Vec<Diagnostic>) {
    let mut stack: Vec<Range<usize>> = Vec::new();
    for t in toks {
        if t.kind == TokKind::Punct && t.text == "(" {
            stack.push(t.span.clone());
        } else if t.kind == TokKind::Punct && t.text == ")" {
            if stack.pop().is_none() {
                out.push(diag(
                    Severity::Error,
                    "Unmatched closing parenthesis".into(),
                    t.span.clone(),
                    sql,
                ));
            }
        }
    }
    for open in stack {
        out.push(diag(Severity::Error, "Unclosed parenthesis".into(), open, sql));
    }
}

/// Spot bare identifiers one edit away from a clause keyword — e.g. `FORM` → `FROM` —
/// and return them as `(span, message)` hints. High-confidence only: an identifier
/// that resolves as a table or registered function is never second-guessed. The
/// caller decides how each hint surfaces: merged into an overlapping parse error's
/// message, dropped under a better engine error, or a standalone warning.
fn keyword_typo_hints(
    toks: &[Tok],
    ctx: &SessionContext,
    functions: &FunctionCatalog,
) -> Vec<(Range<usize>, String)> {
    let mut hints = Vec::new();
    for t in toks {
        if t.kind != TokKind::Ident || t.text.len() < 2 {
            continue;
        }
        // Don't second-guess something that actually resolves.
        if ctx.table_exist(t.text.as_str()).unwrap_or(false) || functions.contains(&t.text) {
            continue;
        }
        let up = t.text.to_ascii_uppercase();
        if CLAUSE_KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(&up)) {
            continue; // it *is* a keyword (lexer may have classed a contextual word as ident)
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

fn diag(severity: Severity, message: String, span: Range<usize>, sql: &str) -> Diagnostic {
    Diagnostic {
        severity,
        source: DiagSource::Validation,
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
                std::cmp::Ordering::Greater => i += 1,
                std::cmp::Ordering::Less => j += 1,
                std::cmp::Ordering::Equal => {
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
    use datafusion::arrow::array::{Int64Array, StringArray};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use futures::executor::block_on;
    use std::sync::Arc;

    /// A context shaped like the engine's: `collect_spans` on, one table `t(id, name)`.
    fn ctx() -> SessionContext {
        let mut config = datafusion::prelude::SessionConfig::new();
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

    fn run(sql: &str) -> Vec<Diagnostic> {
        block_on(validate(&ctx(), &FunctionCatalog::default(), sql))
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
        // The FROM inside the CTE body resolves *that* scope — the main query is
        // still a FROM-less draft and keeps its mid-edit grace.
        assert!(run("WITH x AS (SELECT id FROM t) SELECT draft_col").is_empty());
    }

    #[test]
    fn columns_before_from_stay_quiet() {
        // Mid-composition: no FROM yet, so column references have nothing to
        // resolve against — flagging them is premature, not helpful.
        assert!(run("SELECT name, tags").is_empty());
        assert!(run("SELECT missing").is_empty());
        // Non-column faults still surface without a FROM…
        assert!(!run("SELECT nosuchfn(1)").is_empty());
        // …and once a FROM exists, unknown columns are real again (see
        // `unknown_column_is_spanned`); a FROM-less literal projection stays
        // valid as ever.
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
        let f = FunctionCatalog::default();
        // A view is a first-class query target…
        assert!(block_on(validate(&ctx, &f, "SELECT id FROM v")).is_empty());
        // …its columns are checked through it…
        let sql = "SELECT missing FROM v";
        let out = block_on(validate(&ctx, &f, sql));
        assert_eq!(out.len(), 1);
        assert_eq!(spanned(sql, &out[0]), "missing");
        // …and the broken-parse fallback resolves views too (no false "not found").
        let out = block_on(validate(&ctx, &f, "selct id from v"));
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
        assert!(out[0].message.contains("not_a_function"), "{}", out[0].message);
    }

    #[test]
    fn function_arity_is_checked() {
        // Too few and too many arguments both fail the signature at plan time.
        let out = run("SELECT upper() FROM t");
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(out[0].is_error());

        let out = run("SELECT upper(name, id) FROM t");
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(out[0].is_error());
    }

    #[test]
    fn function_argument_types_are_checked() {
        // An argument the signature can't accept (a scalar into an array function).
        // Note the bound is the engine's own coercion rules: e.g. Int64 into
        // `character_length` coerces and is deliberately NOT flagged.
        let out = run("SELECT array_length(id) FROM t");
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(out[0].is_error());

        // A correctly-typed call stays clean.
        assert!(run("SELECT character_length(name) FROM t").is_empty());
    }

    #[test]
    fn expression_type_faults_are_checked() {
        // Un-coercible arithmetic (the analyzer's type-coercion pass).
        let out = run("SELECT name + INTERVAL '1 day' FROM t");
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(out[0].is_error());
    }

    #[test]
    fn bad_cast_errors() {
        let out = run("SELECT CAST(id AS notatype) FROM t");
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(out[0].is_error());
    }

    #[test]
    fn statements_accumulate_independently() {
        let sql = "SELECT * FROM nope; SELECT missing FROM t; SELECT id FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 2, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert_eq!(spanned(sql, &out[0]), "nope");
        assert_eq!(spanned(sql, &out[1]), "missing");
    }

    #[test]
    fn syntax_error_is_located() {
        // A mid-statement fault (an expression can't start with AND) — not an
        // incompleteness case, so it reports with a span.
        let sql = "SELECT id FROM t WHERE AND id = 1";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(out[0].span.is_some());
        assert!(out[0].is_error());
    }

    #[test]
    fn trailing_incomplete_statement_stays_quiet() {
        // A valid prefix at the end of the buffer is typing-in-progress, not a fault.
        assert!(run("select").is_empty());
        assert!(run("SELECT id FROM t WHERE").is_empty());
        assert!(run("SELECT id FROM t ORDER BY").is_empty());

        // …but an incomplete statement followed by another one is a real fault.
        let sql = "SELECT id FROM t WHERE; SELECT id FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
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
        // A typo'd keyword kills the parse, but the FROM target must still be checked
        // (the token-level fallback). Exactly two diagnostics: the merged parse error
        // on `selct` (hint wording, no separate warning) and the `nope` lookup.
        let sql = "selct * from nope";
        let out = run(sql);
        assert_eq!(out.len(), 2, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
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
        // A real table never gets flagged…
        let sql = "selct * from t";
        assert!(!run(sql).iter().any(|d| d.message.contains("not found")));
        // …nor a qualified name that resolves…
        let sql = "selct * from public.t";
        assert!(!run(sql).iter().any(|d| d.message.contains("not found")));
        // …nor a name the statement introduces itself (CTE).
        let sql = "WITH x AS (SELCT 1) SELECT * FROM x";
        assert!(
            !run(sql).iter().any(|d| d.message.contains("not found")),
            "CTE name must not be flagged"
        );
        // …nor a table-function call in FROM position.
        let sql = "selct * from read_parquet('f.parquet')";
        assert!(!run(sql).iter().any(|d| d.message.contains("not found")));
    }

    #[test]
    fn keyword_like_table_names_are_still_checked() {
        // `event` sits in sqlparser's keyword dictionary (non-reserved), so it lexes
        // as a keyword — the fallback must still treat it as a table name.
        let sql = "selc * from event";
        let out = run(sql);
        assert!(
            out.iter().any(|d| d.message.contains("not found") && spanned(sql, d) == "event"),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );

        // And a real table that happens to carry a keyword name is not flagged.
        let ctx = ctx();
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)])),
            vec![Arc::new(Int64Array::from(vec![1_i64]))],
        )
        .unwrap();
        ctx.register_batch("event", batch).unwrap();
        let out = block_on(validate(&ctx, &FunctionCatalog::default(), sql));
        assert!(
            !out.iter().any(|d| d.message.contains("not found")),
            "{:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn keyword_typo_merges_into_the_parse_error() {
        // One diagnostic, not an error and a warning stacked on the same token: the
        // parse error takes the hint's wording.
        let sql = "SELECT * FORM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(out[0].is_error());
        assert!(out[0].message.contains("Did you mean 'FROM'"), "{}", out[0].message);
        assert_eq!(spanned(sql, &out[0]), "FORM");
    }

    #[test]
    fn typo_hint_defers_to_a_better_engine_error() {
        // `fom` parses fine as a column, so the planner's unknown-column error is the
        // authority — the speculative keyword hint must not add a second row.
        let sql = "SELECT fom FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(out[0].is_error());
        assert_eq!(spanned(sql, &out[0]), "fom");
    }

    #[test]
    fn policy_blocks_managed_ddl_with_guidance() {
        let sql = "CREATE EXTERNAL TABLE x STORED AS PARQUET LOCATION 'f.parquet'";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("Table Config"), "{}", out[0].message);
        assert_eq!(spanned(sql, &out[0]), "CREATE EXTERNAL TABLE");

        let out = run("INSERT INTO t VALUES (3, 'c')");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("INSERT"), "{}", out[0].message);

        let out = run("CREATE TABLE copy_t AS SELECT * FROM t");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("Table Config"), "{}", out[0].message);

        let out = run("CREATE DATABASE other");
        assert_eq!(out.len(), 1);

        let out = run("SET datafusion.execution.batch_size = 1024");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("Settings"), "{}", out[0].message);
    }

    #[test]
    fn view_ddl_is_blocked_toward_the_save_flow() {
        // Views are Save's artifact: ⌘S wraps the *plain query* in CREATE OR REPLACE
        // VIEW itself, so typed view DDL is blocked like the rest of the managed DDL.
        let sql = "CREATE VIEW v AS SELECT id FROM t";
        let out = run(sql);
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("Save"), "{}", out[0].message);
        assert_eq!(spanned(sql, &out[0]), "CREATE VIEW");

        let out = run("DROP VIEW IF EXISTS v");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("catalog"), "{}", out[0].message);
    }

    #[test]
    fn validation_never_mutates_the_session() {
        let ctx = ctx();
        let out = block_on(validate(
            &ctx,
            &FunctionCatalog::default(),
            "CREATE VIEW v AS SELECT id FROM t",
        ));
        assert_eq!(out.len(), 1);
        // Validating the CREATE VIEW must not have created it.
        assert!(!ctx.table_exist("v").unwrap());
    }

    #[test]
    fn unbalanced_parens_are_flagged() {
        let sql = "SELECT sum(id FROM t";
        let out = run(sql);
        assert!(out.iter().any(|d| d.message.contains("Unclosed")));
    }
}
