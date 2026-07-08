//! Statement splitting + **caret clause-context** over the token stream. Heuristic,
//! not a full parse (mid-edit SQL rarely parses) — enough to drive completion:
//! what does the caret sit after, and which tables are in scope?

use std::ops::Range;

use crate::sql::lex::{Tok, TokKind};

/// What the caret position expects — the completion provider keys off this.
#[derive(Clone, Debug, PartialEq)]
pub enum Context {
    /// Start of a statement → statement keywords (SELECT, WITH, CREATE, …).
    StatementStart,
    /// After `SELECT` (projection) → columns + functions + keywords.
    SelectList,
    /// After `FROM` → table / view names.
    AfterFrom,
    /// After a `JOIN` keyword → table / view names.
    AfterJoin,
    /// After `alias.` → columns of the resolved table (name held here).
    AfterDot(String),
    /// A general expression position (WHERE / HAVING / QUALIFY / ON / ORDER BY /
    /// GROUP BY) → columns + functions + keywords.
    Expr,
    Unknown,
}

/// The caret's clause context plus the partial word being typed and the tables in
/// scope for the current statement.
pub struct CaretAnalysis {
    pub context: Context,
    /// The word currently under/just-before the caret (what completion filters on).
    pub partial: String,
    /// Byte span to replace when a completion is accepted (the partial word).
    pub replace: Range<usize>,
    /// `alias → table` bindings from the current statement's FROM/JOIN.
    pub aliases: Vec<(String, String)>,
    /// Table names in scope (FROM/JOIN targets of the current statement).
    pub in_scope: Vec<String>,
}

const JOIN_LEADINS: &[&str] = &[
    "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "NATURAL", "OUTER", "LATERAL",
    "SEMI", "ANTI",
];
const EXPR_CLAUSES: &[&str] = &["WHERE", "HAVING", "QUALIFY", "ON", "ORDER", "GROUP", "BY"];

/// Byte range of the statement containing `caret` (split on top-level `;`).
fn statement_bounds(toks: &[Tok], sql_len: usize, caret: usize) -> (usize, usize) {
    let mut start = 0usize;
    let mut end = sql_len;
    for t in toks {
        if t.kind == TokKind::Punct && t.text == ";" {
            if t.span.end <= caret {
                start = t.span.end;
            } else {
                end = t.span.start;
                break;
            }
        }
    }
    (start, end)
}

/// Extract `alias → table` from the FROM/JOIN items of the token slice. Best-effort:
/// after a `FROM`/`JOIN` keyword, read `ident [AS] [alias]`.
fn aliases_of(toks: &[Tok]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let is_from = toks[i].kind == TokKind::Keyword && toks[i].eq_ci("FROM");
        let is_join = toks[i].kind == TokKind::Keyword && toks[i].eq_ci("JOIN");
        if is_from || is_join {
            // The table name is the next identifier-ish token.
            if let Some(tbl) = toks.get(i + 1).filter(|t| is_name(t)) {
                let table = tbl.text.clone();
                // Optional `AS`, then an optional alias identifier.
                let mut j = i + 2;
                if toks.get(j).map(|t| t.eq_ci("AS")).unwrap_or(false) {
                    j += 1;
                }
                let alias = toks
                    .get(j)
                    .filter(|t| is_name(t))
                    .map(|t| t.text.clone())
                    .unwrap_or_else(|| table.clone());
                out.push((alias, table));
            }
        }
        i += 1;
    }
    out
}

fn is_name(t: &Tok) -> bool {
    matches!(t.kind, TokKind::Ident | TokKind::QuotedIdent)
}

/// Analyse the caret: partial word, clause context, and in-scope tables.
pub fn analyze_caret(sql: &str, caret: usize, toks: &[Tok]) -> CaretAnalysis {
    let caret = caret.min(sql.len());
    let (lo, hi) = statement_bounds(toks, sql.len(), caret);
    let stmt: Vec<Tok> = toks
        .iter()
        .filter(|t| t.span.start >= lo && t.span.end <= hi)
        .cloned()
        .collect();

    let aliases = aliases_of(&stmt);
    let in_scope: Vec<String> = aliases.iter().map(|(_, t)| t.clone()).collect();

    // The partial word = a name/keyword token whose span ends exactly at the caret
    // (i.e. we're typing its tail). Otherwise the caret sits after some other token.
    let partial_tok = stmt
        .iter()
        .find(|t| t.span.end == caret && matches!(t.kind, TokKind::Ident | TokKind::Keyword | TokKind::QuotedIdent));
    let (partial, replace) = match partial_tok {
        Some(t) => (t.text.clone(), t.span.clone()),
        None => (String::new(), caret..caret),
    };

    // Preceding meaningful token (the one before the partial, else before the caret).
    let before: Vec<&Tok> = stmt
        .iter()
        .filter(|t| t.span.end <= replace.start)
        .collect();
    let prev = before.last().copied();
    let prev2 = if before.len() >= 2 {
        Some(before[before.len() - 2])
    } else {
        None
    };

    let context = if prev.is_none() {
        Context::StatementStart
    } else if prev.map(|t| t.kind == TokKind::Punct && t.text == ".").unwrap_or(false) {
        // `x.` → columns of x (resolve alias → table).
        let owner = prev2.map(|t| t.text.clone()).unwrap_or_default();
        let table = aliases
            .iter()
            .find(|(a, _)| a.eq_ignore_ascii_case(&owner))
            .map(|(_, t)| t.clone())
            .unwrap_or(owner);
        Context::AfterDot(table)
    } else if prev.map(|t| t.eq_ci("FROM")).unwrap_or(false) {
        Context::AfterFrom
    } else if prev.map(|t| JOIN_LEADINS.iter().any(|k| t.eq_ci(k))).unwrap_or(false) {
        Context::AfterJoin
    } else {
        // Fall back to the most recent clause keyword in this statement.
        match last_clause(&before) {
            Some(k) if k.eq_ignore_ascii_case("SELECT") => Context::SelectList,
            Some(k) if k.eq_ignore_ascii_case("FROM") => Context::AfterFrom,
            Some(k) if JOIN_LEADINS.iter().any(|j| k.eq_ignore_ascii_case(j)) => Context::AfterJoin,
            Some(k) if EXPR_CLAUSES.iter().any(|c| k.eq_ignore_ascii_case(c)) => Context::Expr,
            _ => Context::Unknown,
        }
    };

    CaretAnalysis {
        context,
        partial,
        replace,
        aliases,
        in_scope,
    }
}

/// The most recent clause keyword among the tokens before the caret.
fn last_clause(before: &[&Tok]) -> Option<String> {
    const CLAUSES: &[&str] = &[
        "SELECT", "FROM", "WHERE", "GROUP", "HAVING", "QUALIFY", "ORDER", "ON",
        "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "NATURAL",
    ];
    before
        .iter()
        .rev()
        .find(|t| t.kind == TokKind::Keyword && CLAUSES.iter().any(|c| t.eq_ci(c)))
        .map(|t| t.text.clone())
}
