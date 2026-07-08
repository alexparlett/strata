//! `complete` — ranked completions for a caret position (S7). Reuses the tokeniser +
//! clause-context, then offers tables/columns/functions/keywords per context, filtered
//! by the partial word under the caret.

use std::ops::Range;

// The full keyword set DataFusion's parser recognises (sqlparser's own table, via the
// datafusion re-export) — the authoritative list, not a hand-picked subset.
use datafusion::sql::sqlparser::keywords::ALL_KEYWORDS;

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

/// Curated multi-word phrases — `sqlparser` keywords are single tokens, so these read
/// nicer as one completion (`GROUP BY` not `GROUP` then `BY`). Offered alongside the
/// full single-word `ALL_KEYWORDS` set. Query-only; every word here must be a keyword
/// we *don't* block below, so the phrase and its parts stay consistent.
const MULTI_WORD: &[&str] = &[
    // clauses
    "GROUP BY", "ORDER BY", "PARTITION BY", "UNION ALL",
    // joins (incl. DataFusion's semi/anti/natural — see the SELECT reference)
    "INNER JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "CROSS JOIN", "NATURAL JOIN",
    "LEFT OUTER JOIN", "RIGHT OUTER JOIN", "FULL OUTER JOIN",
    "LEFT SEMI JOIN", "RIGHT SEMI JOIN", "LEFT ANTI JOIN", "RIGHT ANTI JOIN",
    // predicates
    "IS NULL", "IS NOT NULL", "NOT IN", "IS DISTINCT FROM", "IS NOT DISTINCT FROM",
];

/// DDL/DML keywords excluded from completion — those statements are blocked in the
/// editor (`crate::ddl` allows only queries + CREATE/DROP VIEW via the UI, and SET /
/// SHOW / RESET stay), so offering them would mislead. Filtered (case-insensitively)
/// out of `ALL_KEYWORDS`. (Scalar fns like `replace` still come from the engine
/// registry, so blocking the *keyword* doesn't hide the function.)
const BLOCKED_KEYWORDS: &[&str] = &[
    // create / drop / alter surface
    "CREATE", "TABLE", "VIEW", "EXTERNAL", "DATABASE", "SCHEMA", "DROP", "ALTER",
    "TRUNCATE", "RENAME", "CASCADE", "RESTRICT", "TEMPORARY", "TEMP", "UNLOGGED",
    // data mutation
    "INSERT", "INTO", "UPDATE", "DELETE", "COPY", "MERGE", "UPSERT", "REPLACE",
    "OVERWRITE", "VACUUM",
    // transactions / permissions
    "GRANT", "REVOKE", "COMMIT", "ROLLBACK", "SAVEPOINT", "BEGIN", "START",
    "TRANSACTION", "LOCK", "UNLOCK",
    // schema objects
    "CONSTRAINT", "REFERENCES", "INDEX", "SEQUENCE", "TRIGGER", "PROCEDURE", "STORED",
];

/// Completions for the caret at byte `caret` in `sql`.
pub fn complete(sql: &str, caret: usize, catalog: &Catalog) -> Vec<Completion> {
    let (toks, _lex_err) = lex(sql);
    let ca = analyze_caret(sql, caret, &toks);
    let replace = ca.replace.clone();
    let partial = ca.partial.to_ascii_lowercase();

    let mut items: Vec<Completion> = Vec::new();

    match &ca.context {
        Context::AfterDot(table) => {
            // Only columns of the qualified table.
            if let Some(t) = catalog.table(table) {
                for c in &t.columns {
                    items.push(column_item(&c.name, Some(&c.dtype), &replace));
                }
            }
        }
        Context::AfterFrom | Context::AfterJoin => {
            // The table name being typed, plus keywords that can follow (WHERE / JOIN
            // / GROUP BY / …) — the filter narrows to whichever the partial matches.
            for t in &catalog.tables {
                items.push(table_item(t, &replace));
            }
            push_keywords(&mut items, &replace);
        }
        Context::SelectList | Context::Expr => {
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
            push_keywords(&mut items, &replace);
        }
        // Statement start / unknown: keywords + any symbols (the partial narrows it).
        Context::StatementStart | Context::Unknown => {
            for t in &catalog.tables {
                items.push(table_item(t, &replace));
            }
            for f in catalog.functions.all() {
                items.push(function_item(f, &replace));
            }
            push_keywords(&mut items, &replace);
        }
    }

    // Filter to the partial word; rank prefix matches first, then shorter, then
    // alphabetical (so `fr` surfaces `FROM` above a substring match).
    if !partial.is_empty() {
        items.retain(|c| c.label.to_ascii_lowercase().contains(&partial));
        items.sort_by(|a, b| {
            let (al, bl) = (a.label.to_ascii_lowercase(), b.label.to_ascii_lowercase());
            let (ap, bp) = (al.starts_with(&partial), bl.starts_with(&partial));
            bp.cmp(&ap)
                .then(al.len().cmp(&bl.len()))
                .then(al.cmp(&bl))
        });
    }
    items.truncate(50);
    items
}

/// Push the query keyword set: curated multi-word phrases + the full single-word
/// `ALL_KEYWORDS`, minus the blocked DDL/DML keywords.
fn push_keywords(items: &mut Vec<Completion>, replace: &Range<usize>) {
    for &k in MULTI_WORD {
        items.push(keyword(k, replace));
    }
    for &k in ALL_KEYWORDS {
        if BLOCKED_KEYWORDS.iter().any(|b| b.eq_ignore_ascii_case(k)) {
            continue;
        }
        items.push(keyword(k, replace));
    }
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
