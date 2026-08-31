//! The **token-level tier** — what the service can say before, and without, a parse.
//!
//! Three lints over the lexer's own output: unbalanced parentheses, identifiers one edit away
//! from a clause keyword, and the relation names in a `FROM`/`JOIN` position of a statement the
//! parser *rejected*. That last one is the ladder's bottom rung — the only reading available
//! while a statement is half-written — and it asks the [`NameOracle`] the statement pass asks,
//! so a name only a connected database holds is a known name here too.

use std::cmp::Ordering;
use std::ops::Range;

use datafusion::common::TableReference;

use crate::sql::lex::{is_reserved_in_name_position, Tok, TokKind};
use crate::sql::oracle::NameOracle;
use crate::sql::spans::diag;
use crate::sql::FunctionCatalog;
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

/// Unbalanced parentheses → point at the offending `(` or `)`.
pub(crate) fn check_parens(toks: &[Tok], sql: &str, out: &mut Vec<Diagnostic>) {
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

/// Token-level unknown-table check for a statement the **parser rejected**: the name
/// chains in table position (right after `FROM`/`JOIN`) are resolved through the
/// [`NameOracle`]. Conservative by design — names the statement introduces itself (any ident
/// directly followed by `AS`: CTEs, aliases) and table functions (chain followed by
/// `(`) are skipped, and mixed/quoted multi-part names are left to the planner.
///
/// **The oracle, not the workspace catalog**: this rung runs on the half-written statement the
/// statement pass never reaches, and asking the narrower question squiggled a bare name only a
/// connected database holds — a name the same buffer resolves and runs the moment it parses.
pub(crate) fn check_from_targets(
    names: &NameOracle<'_>,
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
                names.resolves(TableReference::bare(one.text.clone()))
            }
            [one] => names.resolves(TableReference::from(one.text.as_str())),
            many if many.iter().all(|p| p.kind != TokKind::QuotedIdent) => {
                let name = many
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>()
                    .join(".");
                names.resolves(TableReference::from(name.as_str()))
            }
            _ => continue,
        };
        if !exists {
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

/// Spot bare identifiers one edit away from a clause keyword — e.g. `FORM` → `FROM` —
/// and return them as `(span, message)` hints. High-confidence only: an identifier
/// that resolves as a table or registered function is never second-guessed. The
/// caller decides how each hint surfaces: merged into an overlapping parse error's
/// message, dropped under a better engine error, or a standalone warning.
///
/// **"Resolves" is the [`NameOracle`], not the workspace's own catalog**: asking the
/// narrower question squiggled `SELECT * FROM orders` — a query that runs — as an unknown word one
/// edit from `ORDER`, which only the *table not found* error over the same span had been hiding.
pub(crate) fn keyword_typo_hints(
    toks: &[Tok],
    names: &NameOracle<'_>,
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
    let mut hints = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.kind != TokKind::Ident || t.text.len() < 2 {
            continue;
        }
        if names.resolves(TableReference::bare(t.text.clone())) || functions.contains(&t.text) {
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
