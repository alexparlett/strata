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

use std::collections::HashSet;
use std::ops::Range;

// The full keyword set DataFusion's parser recognises (sqlparser's own table, via the
// datafusion re-export) — the authoritative list, not a hand-picked subset.
use datafusion::sql::sqlparser::keywords::ALL_KEYWORDS;

use crate::engine::config::{key_def, Kind as KeyKind, ENGINE_KEYS};
use crate::engine::ddl::{option_keys_for, refuse_reserved_key, OptionKind, STORED_AS_FORMATS};
use crate::engine::sql::context::{
    analyze_caret, function_arguments, refine_statement_clause, statement_tokens, CaretAnalysis,
    Clause, ColumnList, Context, ListSource, Role, LITERAL_WORDS, OPERAND_EXPECTING,
};
use crate::engine::sql::fuzzy::match_tier;
use crate::engine::sql::lex::{
    caret_extends_numeric_literal, caret_in_string_or_comment, is_reserved_in_name_position, lex,
    literal_at, TokKind,
};
use crate::engine::sql::symbols::{Catalog, PreparedSym, TableSym};
use crate::engine::sql::FunctionSym;
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
        // The one exception to the string guard (ED-11): the `OPTIONS ('…')` key —
        // or the value of one — of a typed `CREATE EXTERNAL TABLE` completes inside
        // its quotes. Every other string and comment position stays quiet.
        return options_literal_completions(sql, caret, catalog, manual).unwrap_or_default();
    }
    let (toks, lex_err) = lex(sql, &catalog.dialect);
    // A tokenizer error empties the token stream (lex.rs) — every position would
    // masquerade as a blank statement and mis-offer. An un-tokenizable buffer is
    // mid-edit by definition: stay quiet everywhere until it lexes again. (The one
    // recovery — an `OPTIONS` key literal left unterminated — is the arm above:
    // an unterminated string contains the caret to end-of-input, so it never gets
    // this far.)
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
        Context::Dot(rel) => push_dot_columns(&mut pool, &ca, catalog, rel, &replace),
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
            // Query leads first — a blank tab is usually a query — then the statement
            // leads, the curated ord continuing across the two tables.
            for (i, &k) in QUERY_LEADS.iter().chain(STATEMENT_LEADS).enumerate() {
                pool.push(Cand::ordered(keyword(k, &replace, kw_space), T_PRIMARY, i));
            }
            push_keywords(&mut pool, &replace, false, kw_space);
        }
        // A restart is a fresh *query* (`EXPLAIN |`, after a set op, `FROM (|`,
        // `COPY (|`, `CREATE TABLE t AS |`): the statement leads would promise
        // something Run refuses there.
        Context::At(Clause::Restart, Role::Operand) => {
            for (i, &k) in QUERY_LEADS.iter().enumerate() {
                pool.push(Cand::ordered(keyword(k, &replace, kw_space), T_PRIMARY, i));
            }
            push_keywords(&mut pool, &replace, false, kw_space);
        }
        // `SET |` / `RESET |` — the config keys dispatch would accept, and the value
        // vocabulary a key's kind names at `SET k = |`. A closed pool.
        Context::At(Clause::SetOption, Role::Operand) => {
            push_set_option_items(&mut pool, ca.set_key.as_deref(), &replace);
        }
        // `DROP TABLE |` offers tables and **not** views — `DROP VIEW` is the other
        // statement, and `ddl::tables` says so by name; `DROP VIEW |` the mirror.
        Context::At(Clause::DropTable, Role::Operand) => {
            for t in catalog.tables.iter().filter(|t| !t.is_view) {
                pool.push(Cand::new(table_item(t, &replace), T_PRIMARY));
            }
        }
        Context::At(Clause::DropView, Role::Operand) => {
            for t in catalog.tables.iter().filter(|t| t.is_view) {
                pool.push(Cand::new(table_item(t, &replace), T_PRIMARY));
            }
        }
        // `INSERT INTO |` — only tables whose data Strata owns: the same answer
        // `Engine::is_internal` gives dispatch, read from the store that built the
        // snapshot. Inside the target's **column list**, the target's own columns —
        // and only for a target an INSERT may reach, because offering columns of a
        // statement dispatch refuses would be dishonest. (VALUES tuples are the
        // Binding arm: the content is the user's data.)
        Context::At(Clause::Insert, Role::Operand) => match &ca.column_list {
            Some(list) => push_list_columns(&mut pool, catalog, list, &replace, true),
            None => {
                for t in catalog.tables.iter().filter(|t| t.internal && !t.is_view) {
                    pool.push(Cand::new(table_item(t, &replace), T_PRIMARY));
                }
            }
        },
        // `STORED AS |` — exactly the formats `read_format` parses, as keyword items
        // (uppercase + trailing space is right here).
        Context::At(Clause::CreateExternal, Role::Operand) => {
            for (i, &f) in STORED_AS_FORMATS.iter().enumerate() {
                pool.push(Cand::ordered(keyword(f, &replace, kw_space), T_PRIMARY, i));
            }
        }
        // `DROP FUNCTION |` — only what this session created: a built-in is refused to
        // the statement, because nothing can put one back.
        Context::At(Clause::DropFunction, Role::Operand) => {
            for f in catalog.functions.all().filter(|f| f.created) {
                pool.push(Cand::new(
                    Completion {
                        label: f.name.clone(),
                        // The bare name — a DROP takes the name, never a call.
                        insert: ident_insert(&f.name),
                        kind: CompletionKind::Function,
                        detail: Some("session function".into()),
                        replace: replace.clone(),
                    },
                    T_PRIMARY,
                ));
            }
        }
        // The body of a `CREATE FUNCTION`, after its `RETURN`: the declared argument
        // names, then functions. **Never catalog columns or relations** — the body may
        // reference only its arguments (`ddl/functions.rs`), so offering scope columns
        // would offer exactly what `Definition::check` refuses.
        Context::At(Clause::CreateFunction, Role::Operand) => {
            for name in function_arguments(&toks, sql.len(), caret) {
                pool.push(Cand::new(
                    column_item(&name, Some("argument"), &replace),
                    T_PRIMARY,
                ));
            }
            for f in catalog.functions.all() {
                pool.push(Cand::new(function_item(f, &replace), T_FUNCTION));
            }
        }
        // `COPY |` reads a relation like a FROM target (CTEs vacuously absent, the
        // projection boost a no-op). Inside its `PARTITIONED BY (…)` group, the
        // **source's** columns — the catalog's when the source is a named table, the
        // scraped projection when it is a query.
        Context::At(Clause::Copy, Role::Operand) => match &ca.column_list {
            Some(list) => push_list_columns(&mut pool, catalog, list, &replace, false),
            None => push_relation_targets(&mut pool, &ca, catalog, &replace),
        },
        Context::At(Clause::From | Clause::Describe, Role::Operand) => {
            push_relation_targets(&mut pool, &ca, catalog, &replace);
        }
        // `EXECUTE |` / `DEALLOCATE |` — the session's prepared statements and nothing else.
        // Empty until something has been prepared, which is the correct offer: no table,
        // column or keyword can stand in the operand of either statement.
        Context::At(Clause::Execute, Role::Operand) => {
            for p in &catalog.prepared {
                pool.push(Cand::new(prepared_item(p, &replace), T_PRIMARY));
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

/// The single-relation **column-list** pools — an INSERT's column list, a COPY's
/// `PARTITIONED BY` group. The same shape as a `Dot` position, resolved the same way:
/// that relation's columns and nothing else, empty when the relation cannot be
/// resolved (precision over noise), with the group's already-listed names
/// written-demoting through the same [`column_ord`] composition a clause region's
/// refs do — rank only, never filter, exactly as in a SELECT list. `internal_only`
/// is the INSERT gate: offering columns of a target dispatch refuses would be
/// dishonest.
fn push_list_columns(
    pool: &mut Vec<Cand>,
    catalog: &Catalog,
    list: &ColumnList,
    replace: &Range<usize>,
    internal_only: bool,
) {
    // A set, not a linear scan: a wide table with a mostly-written list would make
    // the per-candidate membership test quadratic in the table's width.
    let listed: HashSet<String> = list.listed.iter().map(|w| w.to_ascii_lowercase()).collect();
    let written = |name: &str| listed.contains(&name.to_ascii_lowercase());
    match &list.source {
        ListSource::Table(name) => {
            let table = catalog
                .table(name)
                .filter(|t| !internal_only || (t.internal && !t.is_view));
            if let Some(t) = table {
                for c in &t.columns {
                    pool.push(Cand::ordered(
                        column_item(&c.name, Some(&c.dtype), replace),
                        T_PRIMARY,
                        column_ord(None, None, written(&c.name)),
                    ));
                }
            }
        }
        ListSource::Projection(cols) => {
            for name in cols {
                pool.push(Cand::ordered(
                    column_item(name, None, replace),
                    T_PRIMARY,
                    column_ord(None, None, written(name)),
                ));
            }
        }
    }
}

/// The relation-target pool (`FROM |`, `DESCRIBE |`, `COPY |`): CTEs, tables and
/// views only — no keyword noise. The written SELECT list ranks them: a relation
/// containing more of the projected columns sorts first (`SELECT name, tags FROM |`
/// floats the tables that have them). Rank only — never filter: column knowledge is
/// incomplete (loading registrations, scraped CTEs) and a typo must not empty the
/// list. (`DESCRIBE` and `COPY` have no projection before the caret, so the boost is
/// a no-op there.)
fn push_relation_targets(
    pool: &mut Vec<Cand>,
    ca: &CaretAnalysis,
    catalog: &Catalog,
    replace: &Range<usize>,
) {
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
            cte_item(&cte.name, replace),
            T_PRIMARY,
            coverage(have) * 2 + written_rel(&cte.name),
        ));
    }
    for t in &catalog.tables {
        let have = refs.iter().filter(|r| t.column(r).is_some()).count();
        pool.push(Cand::ordered(
            table_item(t, replace),
            T_PRIMARY,
            coverage(have) * 2 + written_rel(&t.name),
        ));
    }
}

/// The `Dot(rel)` pool: only columns of the qualified relation — inline relations
/// (CTEs, derived-table aliases) first, then catalog. Sub-ranked by the composed
/// column forces: type affinity when completing a comparison side, cross-side key
/// likelihood at ON positions, written-demotion.
fn push_dot_columns(
    pool: &mut Vec<Cand>,
    ca: &CaretAnalysis,
    catalog: &Catalog,
    rel: &str,
    replace: &Range<usize>,
) {
    let affinity = comparand_kind(ca, catalog);
    let cross = (ca.governing == Clause::On).then(|| other_side_columns(ca, catalog, rel));
    let cross_miss = |name: &str| {
        cross
            .as_ref()
            .map(|c| !c.iter().any(|x| x.eq_ignore_ascii_case(name)))
    };
    let written = |name: &str| ca.clause_refs.iter().any(|w| w.eq_ignore_ascii_case(name));
    if let Some(inline) = ca.inline_relation(rel) {
        for name in &inline.columns {
            pool.push(Cand::ordered(
                column_item(name, Some("cte"), replace),
                T_PRIMARY,
                column_ord(affinity.map(|_| true), cross_miss(name), written(name)),
            ));
        }
    } else if let Some(t) = catalog.table(rel) {
        for c in &t.columns {
            pool.push(Cand::ordered(
                column_item(&c.name, Some(&c.dtype), replace),
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

/// The `SET` / `RESET` pools (ED-11), one closed pool per position. The **key** pool is
/// `ENGINE_KEYS` filtered by the dispatch's own fence ([`refuse_reserved_key`]) so the offer
/// and the refusal cannot drift; keys are inserted verbatim (never quoted, never uppercased),
/// ordered by `ENGINE_KEYS` index, with the key's `default` as the detail — short and
/// non-empty for every offerable key (the empty defaults are all `runtime.*`, which the
/// fence removes; `desc` is a sentence and too long). The **value** pool (`set_key` carried
/// from the caret analysis) is the key's own kind vocabulary — `Bool` and `Enum` only,
/// inserted verbatim lowercase with no trailing space; every other kind takes the user's
/// own value, the correct empty offer.
fn push_set_option_items(pool: &mut Vec<Cand>, set_key: Option<&str>, replace: &Range<usize>) {
    match set_key {
        None => {
            for (i, k) in ENGINE_KEYS
                .iter()
                .filter(|k| refuse_reserved_key(k.key).is_ok())
                .enumerate()
            {
                pool.push(Cand::ordered(
                    Completion {
                        label: k.key.to_string(),
                        insert: k.key.to_string(),
                        // The kind is a glyph, not a taxonomy (`prepared_item`).
                        kind: CompletionKind::Column,
                        detail: Some(k.default.to_string()),
                        replace: replace.clone(),
                    },
                    T_PRIMARY,
                    i,
                ));
            }
        }
        Some(key) => {
            // Lowercased the way the planner folds a `SET` key, so an uppercase
            // spelling dispatch accepts still gets its vocabulary.
            let values: &[&str] = match key_def(&key.to_ascii_lowercase()).map(|k| k.kind) {
                Some(KeyKind::Bool) => &["true", "false"],
                Some(KeyKind::Enum(options)) => options,
                _ => &[],
            };
            push_value_words(pool, values, replace);
        }
    }
}

/// Push a closed value vocabulary — the shared shape of the `SET` and `OPTIONS`
/// value pools (external.rs: the option kinds "mirror the SET value design"):
/// verbatim lowercase, no trailing space, table order.
fn push_value_words(pool: &mut Vec<Cand>, values: &[&str], replace: &Range<usize>) {
    for (i, v) in values.iter().enumerate() {
        pool.push(Cand::ordered(
            Completion {
                label: v.to_string(),
                insert: v.to_string(),
                kind: CompletionKind::Keyword,
                detail: None,
                replace: replace.clone(),
            },
            T_PRIMARY,
            i,
        ));
    }
}

/// The `OPTIONS ('…')` carve-out (ED-11) — the one exception to the string guard, scoped to
/// exactly one position: the caret inside a single-quoted literal that is an `OPTIONS` **key**
/// — or the value of one — inside the `OPTIONS (…)` group of a statement whose head refines to
/// `CREATE EXTERNAL TABLE`. `None` for every other literal, and for every lex error that is
/// not this literal's own unterminated quote: those stay guards.
///
/// Two lexing cases, both required. A **terminated** literal rides the ordinary token stream:
/// partial = the content up to the caret, replace = the content span between the quotes (the
/// quotes are already there, so the bare key is inserted). An **unterminated** one
/// (`OPTIONS ('format.h|`) errors the whole tokenizer — the recovery lexes the prefix before
/// the opening quote, which must be clean, and treats the text after the quote as the partial.
fn options_literal_completions(
    sql: &str,
    caret: usize,
    catalog: &Catalog,
    manual: bool,
) -> Option<Vec<Completion>> {
    let (open, close) = literal_at(sql, caret)?;
    // The overwhelmingly common literal is plain data (`WHERE name = 'foo|`), and
    // this path used to be free — bail before paying a full lex unless an OPTIONS
    // keyword precedes the quote (it must: the group's `(` sits between the two).
    if !sql.as_bytes()[..open]
        .windows(7)
        .any(|w| w.eq_ignore_ascii_case(b"OPTIONS"))
    {
        return None;
    }
    let toks = match close {
        // Terminated: the whole buffer must lex — an error elsewhere is not ours to recover.
        Some(_) => {
            let (toks, err) = lex(sql, &catalog.dialect);
            err.is_none().then_some(toks)?
        }
        // Unterminated to end-of-input: the literal itself is the lex error, so the prefix
        // before its quote lexing clean proves the error is exactly this literal.
        None => {
            let (toks, err) = lex(&sql[..open], &catalog.dialect);
            err.is_none().then_some(toks)?
        }
    };

    // The literal's statement must refine to `CREATE EXTERNAL TABLE` …
    let stmt = statement_tokens(&toks, sql.len(), open);
    let head = stmt.first()?;
    if !(head.kind == TokKind::Keyword && head.eq_ci("CREATE")) {
        return None;
    }
    if refine_statement_clause(stmt, Clause::Create) != Clause::CreateExternal {
        return None;
    }
    // … and the literal must sit inside its `OPTIONS ( … )` group: the innermost paren
    // still open at the literal, with the OPTIONS keyword in front of it.
    let mut stack: Vec<usize> = Vec::new();
    for (i, t) in stmt.iter().enumerate() {
        if t.span.start >= open {
            break;
        }
        if t.kind == TokKind::Punct && t.text == "(" {
            stack.push(i);
        }
        if t.kind == TokKind::Punct && t.text == ")" {
            stack.pop();
        }
    }
    let group = *stack.last()?;
    if group == 0 || !(stmt[group - 1].kind == TokKind::Keyword && stmt[group - 1].eq_ci("OPTIONS"))
    {
        return None;
    }

    // Which format's keys ride the offer: `STORED AS <word>` scanned from the statement's
    // tokens, mapped through the dispatch module's own projection (`option_keys_for` —
    // it owns the NDJSON drop and the empty answer for the no-option formats), so the
    // offer cannot drift from what `read_format`/`apply` accept. (Store-namespace keys
    // and the client options are never offered for the same reason: the arm refuses
    // them toward Connections, and absence from the offer is the same policy.)
    let format_word = stmt
        .iter()
        .enumerate()
        .find(|(i, t)| {
            t.kind == TokKind::Keyword
                && t.eq_ci("STORED")
                && stmt.get(i + 1).is_some_and(|a| a.eq_ci("AS"))
        })
        .and_then(|(i, _)| stmt.get(i + 2))
        .map(|t| t.text.to_ascii_uppercase());
    let keys = option_keys_for(format_word.as_deref().unwrap_or(""));

    let content = open + 1;
    if caret < content {
        return None;
    }
    let partial = sql.get(content..caret)?.to_string();
    // The whole content span in both cases: an unterminated literal runs to
    // end-of-input, so text after the caret is still the literal's — a replace that
    // stopped at the caret would splice that tail onto the accepted key.
    let replace = match close {
        Some(c) => content..c,
        None => content..sql.len(),
    };

    // Key vs value inside the group: a literal whose predecessor is `(` or `,` is a key;
    // one whose predecessor is another string is a value (DataFusion's `'key' 'value'`
    // pairs, comma between pairs). Anything else is not a position this carve-out serves.
    let pred = stmt.iter().rev().find(|t| t.span.end <= open)?;
    let mut pool: Vec<Cand> = Vec::new();
    if pred.kind == TokKind::Punct && (pred.text == "(" || pred.text == ",") {
        for (i, (key, _, what)) in keys.iter().enumerate() {
            pool.push(Cand::ordered(
                Completion {
                    label: key.to_string(),
                    insert: key.to_string(),
                    kind: CompletionKind::Column,
                    detail: Some(what.to_string()),
                    replace: replace.clone(),
                },
                T_PRIMARY,
                i,
            ));
        }
    } else if pred.kind == TokKind::Str {
        // The value offer is the preceding key's own vocabulary — `Bool` and `Enum`
        // kinds only; everything else is the user's data and stays silent. The
        // case-insensitive lookup matches dispatch, which lowercases every key.
        let kind = keys
            .iter()
            .find(|(k, ..)| k.eq_ignore_ascii_case(&pred.text))
            .map(|(_, kind, _)| *kind);
        let values: &[&str] = match kind {
            Some(OptionKind::Bool) => &["true", "false"],
            Some(OptionKind::Enum(words)) => words,
            _ => &[],
        };
        push_value_words(&mut pool, values, &replace);
    } else {
        return None;
    }
    Some(rank(pool, &partial, manual))
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
        || is_reserved_in_name_position(name)
        || OPERAND_EXPECTING
            .iter()
            .any(|w| w.eq_ignore_ascii_case(name))
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

/// A prepared statement at an `EXECUTE` / `DEALLOCATE` operand (ED-08).
///
/// `Function` is the nearest existing kind and the row reads right — a prepared statement is a
/// name invoked with parenthesised arguments — but the **bare** name is inserted, not `name(`:
/// `DEALLOCATE p` takes none, and so does an `EXECUTE` of a statement with no placeholders. Its
/// parameter shape is the detail column instead, which is what says which of the two it is. (The
/// same reuse `cte_item` makes of `Table`, and for the same reason: the kind is a glyph, not a
/// taxonomy.)
fn prepared_item(p: &PreparedSym, replace: &Range<usize>) -> Completion {
    Completion {
        label: p.name.clone(),
        insert: ident_insert(&p.name),
        kind: CompletionKind::Function,
        detail: Some(p.detail()),
        replace: replace.clone(),
    }
}

fn column_item(name: &str, detail: Option<&str>, replace: &Range<usize>) -> Completion {
    Completion {
        label: name.to_string(),
        insert: ident_insert(name),
        kind: CompletionKind::Column,
        detail: detail.map(ToString::to_string),
        replace: replace.clone(),
    }
}

fn function_item(f: &FunctionSym, replace: &Range<usize>) -> Completion {
    Completion {
        label: f.name.clone(),
        insert: format!("{}(", f.name),
        kind: CompletionKind::Function,
        // The arity form (`(Float64[, Int64])`) when we rendered one; the flat
        // "function" only for a name-only symbol (no signatures resolved).
        detail: Some(if f.signatures.is_empty() {
            "function".into()
        } else {
            f.detail()
        }),
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
