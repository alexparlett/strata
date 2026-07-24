//! `complete` — ranked completions for a caret position (S7). Reuses the tokeniser +
//! clause-context, then offers relations/columns/functions/keywords per context,
//! filtered and ranked against the partial word under the caret.
//!
//! Ranking is a composite sort: **match tier** (exact > prefix > word-boundary >
//! substring > subsequence, [`fuzzy::match_tier`]) then **context tier** (what this
//! clause position is *for* — columns before functions before keywords in
//! expressions, relations only after FROM/JOIN, statement keywords first in a blank
//! statement), then label length, then alphabetical. The deep tail of sqlparser's
//! `ALL_KEYWORDS` stays reachable but demoted: it only surfaces on a ≥2-char prefix
//! match, so `SERDE`-class noise never buries a catalog symbol.

use std::ops::Range;

// The full keyword set DataFusion's parser recognises (sqlparser's own table, via the
// datafusion re-export) — the authoritative list, not a hand-picked subset.
use datafusion::sql::sqlparser::keywords::ALL_KEYWORDS;

use crate::engine::sql::context::{
    analyze_caret, CaretAnalysis, Clause, Context, Role, LITERAL_WORDS, OPERAND_EXPECTING,
};
use crate::engine::sql::fuzzy::match_tier;
use crate::engine::sql::lex::{caret_extends_numeric_literal, caret_in_string_or_comment, lex};
use crate::engine::sql::symbols::{Catalog, TableSym};
use strata_model::Kind;

mod ranking;
#[cfg(test)]
mod tests;
mod vocabulary;

use ranking::*;
use vocabulary::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// Completions for the caret at byte `caret` in `sql`. `manual` marks an explicit
/// trigger (⌃/⌘Space) — it widens the offer by lifting the obscure-keyword tail
/// gate (an explicit ask deserves the full vocabulary).
pub fn complete(sql: &str, caret: usize, catalog: &Catalog, manual: bool) -> Vec<Completion> {
    if caret_in_string_or_comment(sql, caret) {
        return Vec::new();
    }
    let (toks, lex_err) = lex(sql);
    // A tokenizer error empties the token stream (lex.rs) — every position would
    // masquerade as a blank statement and mis-offer. An un-tokenizable buffer is
    // mid-edit by definition: stay quiet everywhere until it lexes again.
    if lex_err.is_some() {
        return Vec::new();
    }
    // Mid-literal (`1.` — the dot absorbed into the number token) is not a
    // qualifier: quiet, the same stance as the string/comment guard.
    if caret_extends_numeric_literal(&toks, caret) {
        return Vec::new();
    }
    let ca = analyze_caret(sql, caret, &toks);
    let replace = ca.replace.clone();
    let partial = ca.partial.clone();
    // Keywords are always followed by something, so accepting one inserts a
    // trailing space (the identifier kinds never do — `,`/`.`/`)` may follow) —
    // unless the buffer already provides whitespace right after the word.
    let kw_space = sql[replace.end.min(sql.len())..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_whitespace());

    let mut pool: Vec<Cand> = Vec::new();

    match &ca.context {
        Context::Dot(rel) => {
            // Only columns of the qualified relation: inline relations (CTEs,
            // derived-table aliases) first, then catalog. Sub-ranked by the
            // composed column forces: type affinity when completing a comparison
            // side, cross-side key likelihood at ON positions, written-demotion.
            let affinity = comparand_kind(&ca, catalog);
            let cross = (ca.governing == Clause::On)
                .then(|| other_side_columns(&ca, catalog, rel));
            let cross_miss = |name: &str| {
                cross
                    .as_ref()
                    .map(|c| !c.iter().any(|x| x.eq_ignore_ascii_case(name)))
            };
            let written = |name: &str| {
                ca.clause_refs.iter().any(|w| w.eq_ignore_ascii_case(name))
            };
            if let Some(inline) = ca.inline_relation(rel) {
                for name in &inline.columns {
                    pool.push(Cand::ordered(
                        column_item(name, Some("cte"), &replace),
                        T_PRIMARY,
                        column_ord(affinity.map(|_| true), cross_miss(name), written(name)),
                    ));
                }
            } else if let Some(t) = catalog.table(rel) {
                for c in &t.columns {
                    pool.push(Cand::ordered(
                        column_item(&c.name, Some(&c.dtype), &replace),
                        T_PRIMARY,
                        column_ord(
                            affinity.map(|k| Kind::from_arrow(&c.dtype) != k),
                            cross_miss(&c.name),
                            written(&c.name),
                        ),
                    ));
                }
            }
        }
        // An item is complete — the grammar wants operators / the onward ladder,
        // never a fresh column or function (that's what makes `SELECT * f` offer
        // `FROM` above `floor`).
        Context::At(clause, Role::Continuation) => {
            for (i, k) in continuation_keywords(*clause).into_iter().enumerate() {
                pool.push(Cand::ordered(keyword(k, &replace, kw_space), T_PRIMARY, i));
            }
            push_keywords(&mut pool, &replace, true, kw_space);
        }
        Context::At(Clause::Start, Role::Operand) => {
            for (i, &k) in STATEMENT_KEYWORDS.iter().enumerate() {
                pool.push(Cand::ordered(keyword(k, &replace, kw_space), T_PRIMARY, i));
            }
            push_keywords(&mut pool, &replace, false, kw_space);
        }
        Context::At(Clause::From | Clause::Describe, Role::Operand) => {
            // Relation targets only — CTEs, tables, views. No keyword noise here.
            // (`DESCRIBE |` inspects a relation — the same operand as a FROM
            // target; its empty projection makes the boost a no-op there.)
            // The written SELECT list ranks them: a relation containing more of the
            // projected columns sorts first (`SELECT name, tags FROM |` floats the
            // tables that have them). Rank only — never filter: column knowledge is
            // incomplete (loading registrations, scraped CTEs) and a typo must not
            // empty the list.
            let refs = &ca.projection;
            let coverage = |have: usize| refs.len().saturating_sub(have).min(60);
            // Already-joined relations sink (a self-join is legal, rarely next).
            let written_rel =
                |name: &str| ca.in_scope.iter().any(|s| s.eq_ignore_ascii_case(name)) as usize;
            for cte in &ca.ctes {
                let have = refs
                    .iter()
                    .filter(|r| cte.columns.iter().any(|c| c.eq_ignore_ascii_case(r)))
                    .count();
                pool.push(Cand::ordered(
                    cte_item(&cte.name, &replace),
                    T_PRIMARY,
                    coverage(have) * 2 + written_rel(&cte.name),
                ));
            }
            for t in &catalog.tables {
                let have = refs.iter().filter(|r| t.column(r).is_some()).count();
                pool.push(Cand::ordered(
                    table_item(t, &replace),
                    T_PRIMARY,
                    coverage(have) * 2 + written_rel(&t.name),
                ));
            }
        }
        // LIMIT / OFFSET take numbers — nothing sensible to offer.
        Context::At(Clause::Limit | Clause::Offset, Role::Operand) => {}
        // A name is being invented (`AS |`) or an unmodeled statement noun typed
        // (`SHOW |`) — the empty offer is the correct one.
        Context::At(_, Role::Binding) => {}
        // Every expression clause's operand position: columns first, then
        // aliases / functions / qualifiers / keywords.
        Context::At(clause, Role::Operand) => {
            push_scope_columns(&mut pool, &ca, catalog, &replace, &partial);
            // SELECT-list column aliases (e.g. `SUM(x) AS spend`) — referenceable
            // exactly where SQL allows them: GROUP BY / ORDER BY / HAVING /
            // QUALIFY, never back inside the SELECT list or WHERE (the validator
            // would immediately squiggle the offer).
            if matches!(
                clause,
                Clause::GroupBy | Clause::OrderBy | Clause::Having | Clause::Qualify
            ) {
                for a in &ca.select_aliases {
                    pool.push(Cand::new(
                        column_item(a, Some("alias"), &replace),
                        T_SECONDARY,
                    ));
                }
            }
            for f in catalog.functions.all() {
                pool.push(Cand::new(function_item(f, &replace), T_FUNCTION));
            }
            // Relation names as qualifiers (`orders.` → columns) — never above columns.
            for cte in &ca.ctes {
                pool.push(Cand::new(cte_item(&cte.name, &replace), T_KEYWORD));
            }
            for t in &catalog.tables {
                pool.push(Cand::new(table_item(t, &replace), T_KEYWORD));
            }
            push_keywords(&mut pool, &replace, false, kw_space);
        }
    }

    rank(pool, &partial, manual)
}

/// The all-catalog fallback stops materialising candidates past this many — it's a
/// convenience tier for the no-FROM-yet position, and with an empty partial the
/// visible 50 come from the shortest names anyway. Matching against the partial
/// happens **before** allocation, so at large catalog scale a typed prefix only
/// pays for the columns it matches.
const FALLBACK_COLUMN_CAP: usize = 2048;

/// In-scope columns at the primary tier; when the statement's FROM scope resolves to
/// no columns at all (no FROM yet, or an unregistered name), fall back to the
/// catalog's columns at the secondary tier — `SELECT na|` before FROM still
/// completes `name`, with the owning table in the detail.
fn push_scope_columns(
    pool: &mut Vec<Cand>,
    ca: &CaretAnalysis,
    catalog: &Catalog,
    replace: &Range<usize>,
    partial: &str,
) {
    let affinity = comparand_kind(ca, catalog);
    let on_clause = ca.governing == Clause::On;
    let written = |name: &str| ca.clause_refs.iter().any(|w| w.eq_ignore_ascii_case(name));
    let mut any = false;
    for tname in &ca.in_scope {
        let cross = on_clause.then(|| other_side_columns(ca, catalog, tname));
        let cross_miss = |name: &str| {
            cross
                .as_ref()
                .map(|c| !c.iter().any(|x| x.eq_ignore_ascii_case(name)))
        };
        if let Some(inline) = ca.inline_relation(tname) {
            for name in &inline.columns {
                any = true;
                pool.push(Cand::ordered(
                    column_item(name, Some(&format!("{} · cte", inline.name)), replace),
                    T_PRIMARY,
                    column_ord(affinity.map(|_| true), cross_miss(name), written(name)),
                ));
            }
        } else if let Some(t) = catalog.table(tname) {
            for c in &t.columns {
                any = true;
                pool.push(Cand::ordered(
                    column_item(&c.name, Some(&format!("{} · {}", t.name, c.dtype)), replace),
                    T_PRIMARY,
                    column_ord(
                        affinity.map(|k| Kind::from_arrow(&c.dtype) != k),
                        cross_miss(&c.name),
                        written(&c.name),
                    ),
                ));
            }
        }
    }
    if !any {
        // The symmetric twin of the FROM-target boost: a column ranks by how well
        // its owning table covers the columns already written — `SELECT name, |`
        // clusters the next suggestions toward the tables that could supply
        // `name` too (the candidate FROM set, inferred as you compose). Rank
        // only, never filter; and tables iterate best-covered first so the cap
        // keeps the most consistent columns, not the first-registered ones.
        let refs = &ca.projection;
        let mut order: Vec<(usize, usize)> = catalog
            .tables
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let have = refs.iter().filter(|r| t.column(r).is_some()).count();
                (i, refs.len().saturating_sub(have).min(60) * 2)
            })
            .collect();
        order.sort_by_key(|(_, ord)| *ord);
        let mut pushed = 0usize;
        for (ti, base) in order {
            let t = &catalog.tables[ti];
            for c in &t.columns {
                if pushed >= FALLBACK_COLUMN_CAP {
                    return;
                }
                if match_tier(&c.name, partial).is_none() {
                    continue;
                }
                pushed += 1;
                pool.push(Cand::ordered(
                    column_item(&c.name, Some(&format!("{} · {}", t.name, c.dtype)), replace),
                    T_SECONDARY,
                    base + written(&c.name) as usize,
                ));
            }
        }
    }
}

/// Push the query keyword set: curated multi-word phrases + the full single-word
/// `ALL_KEYWORDS` (minus blocked DDL/DML). At an **operand** position the CORE
/// vocabulary *and* the multi-word phrases ride free at the keyword tier and only
/// the obscure tail is gated.
/// At a **continuation** position the curated clause set already *is* the
/// grammar's expected tokens — everything here is the gated tail (a ≥2-char
/// prefix summons it), so `FROM` can never trail a `WHERE` clause uninvited.
fn push_keywords(pool: &mut Vec<Cand>, replace: &Range<usize>, gate_all: bool, kw_space: bool) {
    for &k in MULTI_WORD {
        pool.push(Cand {
            c: keyword(k, replace, kw_space),
            ctx: if gate_all { T_TAIL } else { T_KEYWORD },
            ord: 0,
            tail: gate_all,
        });
    }
    for &k in ALL_KEYWORDS {
        if BLOCKED_KEYWORDS.iter().any(|b| b.eq_ignore_ascii_case(k)) {
            continue;
        }
        let core = !gate_all && CORE_KEYWORDS.iter().any(|c| c.eq_ignore_ascii_case(k));
        pool.push(Cand {
            c: keyword(k, replace, kw_space),
            ctx: if core { T_KEYWORD } else { T_TAIL },
            ord: 0,
            tail: !core,
        });
    }
}

/// Whether an identifier must be double-quoted to survive DataFusion's parser
/// *and mean the column*: anything that isn't a plain lowercase `[a-z_][a-z0-9_]*`
/// word, or that collides with a reserved keyword (`order`), **or** with the
/// expression grammar's own vocabulary — a column named `null` inserted bare
/// selects the literal (silently wrong data), one named `case` breaks the parse.
/// The collision set is the union of every table the model already declares:
/// parser-reserved ∪ [`OPERAND_EXPECTING`] ∪ [`LITERAL_WORDS`]. Merely-known
/// keywords outside those — `name`, `status`, `plain` — stay unquoted.
fn needs_quoting(name: &str) -> bool {
    let plain = {
        let mut chars = name.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    !plain
        || crate::engine::sql::lex::is_reserved_in_name_position(name)
        || OPERAND_EXPECTING.iter().any(|w| w.eq_ignore_ascii_case(name))
        || LITERAL_WORDS.iter().any(|w| w.eq_ignore_ascii_case(name))
}

fn ident_insert(name: &str) -> String {
    if needs_quoting(name) {
        format!("\"{}\"", name.replace('"', "\"\""))
    } else {
        name.to_string()
    }
}

fn table_item(t: &TableSym, replace: &Range<usize>) -> Completion {
    Completion {
        label: t.name.clone(),
        insert: ident_insert(&t.name),
        kind: if t.is_view {
            CompletionKind::View
        } else {
            CompletionKind::Table
        },
        detail: Some(if t.is_view { "view" } else { "table" }.into()),
        replace: replace.clone(),
    }
}

fn cte_item(name: &str, replace: &Range<usize>) -> Completion {
    Completion {
        label: name.to_string(),
        insert: ident_insert(name),
        kind: CompletionKind::Table,
        detail: Some("cte".into()),
        replace: replace.clone(),
    }
}

fn column_item(name: &str, detail: Option<&str>, replace: &Range<usize>) -> Completion {
    Completion {
        label: name.to_string(),
        insert: ident_insert(name),
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

fn keyword(k: &str, replace: &Range<usize>, trailing_space: bool) -> Completion {
    Completion {
        label: k.to_string(),
        insert: if trailing_space {
            format!("{k} ")
        } else {
            k.to_string()
        },
        kind: CompletionKind::Keyword,
        detail: Some("keyword".into()),
        replace: replace.clone(),
    }
}
