//! `analyze` — the static validator (S25). Runs cheap, error-tolerant checks over the
//! token stream and returns [`crate::diagnostics::Diagnostic`]s (with byte spans for
//! squiggles). Deliberately **conservative**: structural faults + high-confidence
//! keyword typos only. Deep semantic truth (unknown table/column/function) is left to
//! the engine dry-plan (see `docs/SQL_LANGUAGE_SPEC.md` §3), since bare identifiers are
//! often legitimately quoted / aliased / CTE / information_schema names.

use std::ops::Range;

use crate::diagnostics::{DiagSource, Diagnostic, Severity};
use crate::sql::lex::{lex, Tok, TokKind};
use crate::sql::symbols::Catalog;

/// Clause keywords we typo-check bare identifiers against (edit distance ≤ 1).
const CLAUSE_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "GROUP", "ORDER", "HAVING", "QUALIFY", "LIMIT", "OFFSET",
    "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "OUTER", "CROSS", "NATURAL", "ON", "USING",
    "AS", "BY", "DISTINCT", "UNION", "INTERSECT", "EXCEPT", "WITH", "AND", "OR", "NOT",
    "NULL", "CASE", "WHEN", "THEN", "ELSE", "END", "ASC", "DESC",
];

/// Analyse `sql` against `catalog` and return diagnostics. `catalog` is currently used
/// only to avoid typo-flagging a real identifier; semantic checks come later.
pub fn analyze(sql: &str, catalog: &Catalog) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if sql.trim().is_empty() {
        return out;
    }

    let (toks, lex_err) = lex(sql);
    if let Some(e) = lex_err {
        out.push(diag(
            Severity::Error,
            e.message,
            Some("Syntax".into()),
            e.span,
            sql,
        ));
        // A tokenizer failure means the rest of the checks would be unreliable.
        return out;
    }

    check_parens(&toks, sql, &mut out);
    check_keyword_typos(&toks, catalog, sql, &mut out);
    out
}

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
                    Some("Syntax".into()),
                    t.span.clone(),
                    sql,
                ));
            }
        }
    }
    for open in stack {
        out.push(diag(
            Severity::Error,
            "Unclosed parenthesis".into(),
            Some("Syntax".into()),
            open,
            sql,
        ));
    }
}

/// Flag a bare identifier that is one edit away from a clause keyword and is not a
/// real symbol — e.g. `FORM` → `FROM`. High-confidence only.
fn check_keyword_typos(toks: &[Tok], catalog: &Catalog, sql: &str, out: &mut Vec<Diagnostic>) {
    for t in toks {
        if t.kind != TokKind::Ident || t.text.len() < 2 {
            continue;
        }
        // Don't second-guess something that actually resolves.
        if catalog.has_table(&t.text) || catalog.functions.contains(&t.text) {
            continue;
        }
        let up = t.text.to_ascii_uppercase();
        if CLAUSE_KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(&up)) {
            continue; // it *is* a keyword (lexer may have classed a contextual word as ident)
        }
        if let Some(kw) = CLAUSE_KEYWORDS
            .iter()
            .find(|k| edit_distance_at_most_1(&up, k))
        {
            out.push(diag(
                Severity::Warning,
                format!("Unknown keyword `{}` — did you mean `{}`?", t.text, kw),
                Some("Typo".into()),
                t.span.clone(),
                sql,
            ));
        }
    }
}

fn diag(
    severity: Severity,
    message: String,
    code: Option<String>,
    span: Range<usize>,
    sql: &str,
) -> Diagnostic {
    Diagnostic {
        severity,
        source: DiagSource::Validation,
        message,
        loc: Some(line_col(sql, span.start)),
        code,
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

/// Whether `a` and `b` are equal or differ by a single insertion/deletion/substitution
/// (case-insensitive; inputs already upper-cased). Cheap early-out on length.
fn edit_distance_at_most_1(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > 1 {
        return false;
    }
    if a == b {
        return false; // exact match isn't a typo (handled by callers anyway)
    }
    // Two-pointer walk allowing one edit.
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
                std::cmp::Ordering::Greater => i += 1, // deletion from a
                std::cmp::Ordering::Less => j += 1,    // insertion into a
                std::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                } // substitution
            }
        }
    }
    true
}
