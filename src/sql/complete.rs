//! `complete` — ranked completions for a caret position (S7). Reuses the tokeniser +
//! clause-context, then offers tables/columns/functions/keywords per context, filtered
//! by the partial word under the caret.

use std::ops::Range;

use crate::sql::context::{analyze_caret, Context};
use crate::sql::lex::lex;
use crate::sql::symbols::{Catalog, TableSym};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Table,
    View,
    Column,
    Function,
    Keyword,
}

/// One completion candidate. `replace` is the byte span of the partial word to swap
/// out when accepted (so we replace the half-typed token, not just insert).
#[derive(Clone, PartialEq)]
pub struct Completion {
    pub label: String,
    pub insert: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    pub replace: Range<usize>,
}

const STATEMENT_KEYWORDS: &[&str] = &[
    "SELECT", "WITH", "CREATE", "INSERT", "COPY", "EXPLAIN", "DROP", "DESCRIBE", "SHOW",
    "SET",
];
const EXPR_KEYWORDS: &[&str] = &[
    "AND", "OR", "NOT", "IN", "IS", "NULL", "LIKE", "ILIKE", "BETWEEN", "CASE", "WHEN",
    "THEN", "ELSE", "END", "DISTINCT", "AS",
];

/// Completions for the caret at byte `caret` in `sql`.
pub fn complete(sql: &str, caret: usize, catalog: &Catalog) -> Vec<Completion> {
    let (toks, _lex_err) = lex(sql);
    let ca = analyze_caret(sql, caret, &toks);
    let replace = ca.replace.clone();
    let partial = ca.partial.to_ascii_lowercase();

    let mut items: Vec<Completion> = Vec::new();

    match &ca.context {
        Context::StatementStart => {
            for k in STATEMENT_KEYWORDS {
                items.push(keyword(k, &replace));
            }
        }
        Context::AfterFrom | Context::AfterJoin => {
            for t in &catalog.tables {
                items.push(table_item(t, &replace));
            }
        }
        Context::AfterDot(table) => {
            if let Some(t) = catalog.table(table) {
                for c in &t.columns {
                    items.push(column_item(&c.name, Some(&c.dtype), &replace));
                }
            }
        }
        Context::SelectList | Context::Expr => {
            // In-scope columns first (qualified detail), then functions + keywords.
            for tname in &ca.in_scope {
                if let Some(t) = catalog.table(tname) {
                    for c in &t.columns {
                        items.push(column_item(
                            &c.name,
                            Some(&format!("{} · {}", t.name, c.dtype)),
                            &replace,
                        ));
                    }
                }
            }
            for f in catalog.functions.all() {
                items.push(function_item(f, &replace));
            }
            for k in EXPR_KEYWORDS {
                items.push(keyword(k, &replace));
            }
        }
        Context::Unknown => {
            for t in &catalog.tables {
                items.push(table_item(t, &replace));
            }
            for f in catalog.functions.all() {
                items.push(function_item(f, &replace));
            }
        }
    }

    // Filter to the partial word, prefix matches first.
    if !partial.is_empty() {
        items.retain(|c| c.label.to_ascii_lowercase().contains(&partial));
        items.sort_by_key(|c| {
            usize::from(!c.label.to_ascii_lowercase().starts_with(&partial))
        });
    }
    items.truncate(50);
    items
}

fn table_item(t: &TableSym, replace: &Range<usize>) -> Completion {
    Completion {
        label: t.name.clone(),
        insert: t.name.clone(),
        kind: if t.is_view {
            CompletionKind::View
        } else {
            CompletionKind::Table
        },
        detail: Some(if t.is_view { "view" } else { "table" }.into()),
        replace: replace.clone(),
    }
}

fn column_item(name: &str, detail: Option<&str>, replace: &Range<usize>) -> Completion {
    Completion {
        label: name.to_string(),
        insert: name.to_string(),
        kind: CompletionKind::Column,
        detail: detail.map(|d| d.to_string()),
        replace: replace.clone(),
    }
}

fn function_item(name: &str, replace: &Range<usize>) -> Completion {
    Completion {
        label: name.to_string(),
        insert: format!("{name}("),
        kind: CompletionKind::Function,
        detail: Some("function".into()),
        replace: replace.clone(),
    }
}

fn keyword(k: &str, replace: &Range<usize>) -> Completion {
    Completion {
        label: k.to_string(),
        insert: k.to_string(),
        kind: CompletionKind::Keyword,
        detail: None,
        replace: replace.clone(),
    }
}
