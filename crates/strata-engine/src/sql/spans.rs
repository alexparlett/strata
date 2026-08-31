//! Turning a fault into a **byte-spanned diagnostic**.
//!
//! The service's answer is a list of [`Diagnostic`]s the editor squiggles, so every tier has to
//! say *where*. This module is the whole of that arithmetic: splitting a buffer into statements,
//! recognizing the parse failure that only means "still typing", and folding DataFusion's own
//! error — spanned or not — onto a range of the buffer the user wrote.
//!
//! Kept apart from [`service`](super::service), which decides *what* is wrong, and from
//! [`lint`](super::lint), which decides it at token level: a span is a fact about text, and
//! nothing here reads the catalog.

use std::ops::Range;

use datafusion::common::diagnostic::DiagnosticKind;
use datafusion::common::{DataFusionError, SchemaError};
use datafusion::sql::sqlparser::parser::ParserError;

use crate::sql::lex::{rel_offset, split_statements, Tok, TokKind};
use strata_model::{Diagnostic, Severity};

/// Whether two byte ranges intersect.
pub(crate) fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// The planner failed to resolve a column reference (`Schema error: No field
/// named …`) — matched by variant, not message text.
pub(crate) fn is_unresolved_column(err: &DataFusionError) -> bool {
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
pub(crate) fn is_incomplete(
    err: &DataFusionError,
    slice: &str,
    stmt: &Range<usize>,
    toks: &[Tok],
) -> bool {
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
pub(crate) fn statement_ranges(sql: &str, toks: &[Tok]) -> Vec<Range<usize>> {
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
pub(crate) fn trim_range(sql: &str, range: Range<usize>) -> Option<Range<usize>> {
    let slice = &sql[range.clone()];
    let trimmed = slice.trim_start();
    let start = range.start + (slice.len() - trimmed.len());
    let end = start + trimmed.trim_end().len();
    (start < end).then_some(start..end)
}

/// The span of a statement's leading keyword run (`CREATE EXTERNAL TABLE`,
/// `INSERT INTO`, …) — what a policy diagnostic underlines instead of the whole
/// statement.
pub(crate) fn leading_keywords_span(toks: &[Tok], stmt: &Range<usize>) -> Range<usize> {
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

/// Fold a parse/plan error for the statement at `stmt` (whose text is `slice`) into a
/// byte-spanned [`Diagnostic`]. Best span first: the planner's own spanned
/// `Diagnostic` (DF 54, `collect_spans` on) → the `Line: N, Column: M` embedded in a
/// parser message → the statement's leading keywords.
pub(crate) fn df_error_diag(
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
pub(crate) fn widen_to_token(span: Range<usize>, toks: &[Tok]) -> Range<usize> {
    if span.end > span.start {
        return span;
    }
    toks.iter()
        .find(|t| t.span.start <= span.start && span.start < t.span.end)
        .map(|t| t.span.clone())
        .unwrap_or(span.start..span.start + 1)
}

/// `Line: N, Column: M` from a sqlparser message, if present.
pub(crate) fn extract_line_col(message: &str) -> Option<(usize, usize)> {
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
