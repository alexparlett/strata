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

use datafusion::sql::sqlparser::keywords::ALL_KEYWORDS;

use crate::config::{key_def, Kind as KeyKind, ENGINE_KEYS};
use crate::ddl::{option_keys_for, refuse_reserved_key, OptionKind, STORED_AS_FORMATS};
use crate::sql::context::{
    analyze_caret, function_arguments, refine_statement_clause, statement_tokens, CaretAnalysis,
    Clause, ColumnList, Context, ListSource, Role,
};
use crate::sql::ident::quote_verbatim;
use crate::sql::lex::{
    caret_extends_numeric_literal, caret_in_string_or_comment, lex, literal_at, TokKind,
};
use crate::sql::symbols::{Catalog, DatabaseSym, PreparedSym, RelationSym, TableSym};
use crate::sql::FunctionSym;
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
        return options_literal_completions(sql, caret, catalog, manual).unwrap_or_default();
    }
    let (toks, lex_err) = lex(sql, &catalog.dialect);
    if lex_err.is_some() {
        return Vec::new();
    }
    if caret_extends_numeric_literal(&toks, caret) {
        return Vec::new();
    }
    let ca = analyze_caret(sql, caret, &toks);
    let replace = ca.replace.clone();
    let partial = ca.partial.clone();
    let kw_space = sql[replace.end.min(sql.len())..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_whitespace());

    let mut pool = Pool::new(&partial, manual);

    match &ca.context {
        Context::Dot(chain) => push_dot_items(&mut pool, &ca, catalog, chain, &replace),
        Context::At(clause, Role::Continuation) => {
            for (i, k) in continuation_keywords(*clause).into_iter().enumerate() {
                pool.ordered(k, T_PRIMARY, i, || keyword(k, &replace, kw_space));
            }
            push_keywords(&mut pool, &replace, true, kw_space);
        }
        Context::At(Clause::Start, Role::Operand) => {
            for (i, &k) in QUERY_LEADS.iter().chain(STATEMENT_LEADS).enumerate() {
                pool.ordered(k, T_PRIMARY, i, || keyword(k, &replace, kw_space));
            }
            push_keywords(&mut pool, &replace, false, kw_space);
        }
        Context::At(Clause::Restart, Role::Operand) => {
            for (i, &k) in QUERY_LEADS.iter().enumerate() {
                pool.ordered(k, T_PRIMARY, i, || keyword(k, &replace, kw_space));
            }
            push_keywords(&mut pool, &replace, false, kw_space);
        }
        Context::At(Clause::SetOption, Role::Operand) => {
            push_set_option_items(&mut pool, ca.set_key.as_deref(), &replace);
        }
        Context::At(Clause::DropTable, Role::Operand) => {
            for t in catalog.tables.iter().filter(|t| !t.is_view) {
                pool.push(&t.name, T_PRIMARY, || table_item(t, &replace));
            }
        }
        Context::At(Clause::DropView, Role::Operand) => {
            for t in catalog.tables.iter().filter(|t| t.is_view) {
                pool.push(&t.name, T_PRIMARY, || table_item(t, &replace));
            }
        }
        Context::At(Clause::Insert, Role::Operand) => match &ca.column_list {
            Some(list) => push_list_columns(&mut pool, catalog, list, &replace, true),
            None => {
                for t in catalog.tables.iter().filter(|t| t.internal && !t.is_view) {
                    pool.push(&t.name, T_PRIMARY, || table_item(t, &replace));
                }
            }
        },
        Context::At(Clause::CreateExternal, Role::Operand) => {
            for (i, &f) in STORED_AS_FORMATS.iter().enumerate() {
                pool.ordered(f, T_PRIMARY, i, || keyword(f, &replace, kw_space));
            }
        }
        Context::At(Clause::DropFunction, Role::Operand) => {
            for f in catalog.functions.all().filter(|f| f.created) {
                pool.push(&f.name, T_PRIMARY, || Completion {
                    label: f.name.clone(),
                    insert: quote_verbatim(&f.name),
                    kind: CompletionKind::Function,
                    detail: Some("session function".into()),
                    replace: replace.clone(),
                });
            }
        }
        Context::At(Clause::CreateFunction, Role::Operand) => {
            for name in function_arguments(&toks, sql.len(), caret) {
                pool.push(&name, T_PRIMARY, || {
                    column_item(&name, Some("argument"), &replace)
                });
            }
            for f in catalog.functions.all() {
                pool.push(&f.name, T_FUNCTION, || function_item(f, &replace));
            }
        }
        Context::At(Clause::Copy, Role::Operand) => match &ca.column_list {
            Some(list) => push_list_columns(&mut pool, catalog, list, &replace, false),
            None => push_relation_targets(&mut pool, &ca, catalog, &replace),
        },
        Context::At(Clause::From | Clause::Describe, Role::Operand) => {
            push_relation_targets(&mut pool, &ca, catalog, &replace);
        }
        Context::At(Clause::Execute, Role::Operand) => {
            for p in &catalog.prepared {
                pool.push(&p.name, T_PRIMARY, || prepared_item(p, &replace));
            }
        }
        Context::At(Clause::Limit | Clause::Offset, Role::Operand) => {}
        Context::At(_, Role::Binding) => {}
        Context::At(clause, Role::Operand) => {
            push_scope_columns(&mut pool, &ca, catalog, &replace);
            if matches!(
                clause,
                Clause::GroupBy | Clause::OrderBy | Clause::Having | Clause::Qualify
            ) {
                for a in &ca.select_aliases {
                    pool.push(a, T_SECONDARY, || column_item(a, Some("alias"), &replace));
                }
            }
            for f in catalog.functions.all() {
                pool.push(&f.name, T_FUNCTION, || function_item(f, &replace));
            }
            for cte in &ca.ctes {
                pool.push(&cte.name, T_KEYWORD, || cte_item(&cte.name, &replace));
            }
            for t in &catalog.tables {
                pool.push(&t.name, T_KEYWORD, || table_item(t, &replace));
            }
            push_keywords(&mut pool, &replace, false, kw_space);
        }
    }

    rank(pool)
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
    pool: &mut Pool,
    catalog: &Catalog,
    list: &ColumnList,
    replace: &Range<usize>,
    internal_only: bool,
) {
    let listed: HashSet<String> = list.listed.iter().map(|w| w.to_ascii_lowercase()).collect();
    let written = |name: &str| listed.contains(&name.to_ascii_lowercase());
    match &list.source {
        ListSource::Table(name) => {
            let table = catalog
                .table(name)
                .filter(|t| !internal_only || (t.internal && !t.is_view));
            if let Some(t) = table {
                for c in &t.columns {
                    pool.ordered(
                        &c.name,
                        T_PRIMARY,
                        column_ord(None, None, written(&c.name)),
                        || column_item(&c.name, Some(&c.dtype), replace),
                    );
                }
            }
        }
        ListSource::Projection(cols) => {
            for name in cols {
                pool.ordered(
                    name,
                    T_PRIMARY,
                    column_ord(None, None, written(name)),
                    || column_item(name, None, replace),
                );
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
///
/// The database connections' **catalog names** ride at the end of the pool (DB-06): they are the
/// first segment of a three-part name rather than a relation, so they rank behind everything that
/// can stand alone, and a connection offers its name whether or not it is live — the name is the
/// def's, and a connection nobody has reached is still what the query has to say.
fn push_relation_targets(
    pool: &mut Pool,
    ca: &CaretAnalysis,
    catalog: &Catalog,
    replace: &Range<usize>,
) {
    let refs = &ca.projection;
    let coverage = |have: usize| refs.len().saturating_sub(have).min(60);
    let written_rel =
        |name: &str| ca.in_scope.iter().any(|s| s.eq_ignore_ascii_case(name)) as usize;
    for cte in &ca.ctes {
        let have = refs
            .iter()
            .filter(|r| cte.columns.iter().any(|c| c.eq_ignore_ascii_case(r)))
            .count();
        let ord = coverage(have) * 2 + written_rel(&cte.name);
        pool.ordered(&cte.name, T_PRIMARY, ord, || cte_item(&cte.name, replace));
    }
    for t in &catalog.tables {
        let have = match refs.is_empty() {
            true => 0,
            false => refs.iter().filter(|r| t.column(r).is_some()).count(),
        };
        let ord = coverage(have) * 2 + written_rel(&t.name);
        pool.ordered(&t.name, T_PRIMARY, ord, || table_item(t, replace));
    }
    for db in &catalog.databases {
        pool.push(&db.name, T_SECONDARY, || database_item(db, replace));
    }
}

/// The `Dot(chain)` pool — what is *inside* the qualifier the caret sits behind.
///
/// Two namespaces, and the chain's **head** decides which, because only one of them
/// can be addressed by a catalog name at all. A head naming a database connection
/// (DB-06) makes the whole chain remote: one segment offers that connection's
/// enabled schemas, two offers the relations in a schema, and three offers
/// **nothing** — a remote relation's columns are an introspection round trip, and
/// the completion path does no I/O (§7). Anything else is the workspace, where the
/// last segment names the relation (the single-namespace rule: `strata.public.t.` is
/// `t`) and the pool is that relation's columns.
///
/// Reading the head rather than the tail is also what stops `pg.public.orders.` from
/// answering with a *workspace* table that happens to be called `orders`.
///
/// The one place the workspace goes first is a **single** segment that names something
/// in scope: `orders.` is that relation's columns even on a project whose connection
/// is also called `orders`, because a one-segment qualifier is what a relation or an
/// alias is written as and a catalog never is (DataFusion resolves a two-part name
/// inside the default catalog, so a remote relation is always spelled with three).
fn push_dot_items(
    pool: &mut Pool,
    ca: &CaretAnalysis,
    catalog: &Catalog,
    chain: &[String],
    replace: &Range<usize>,
) {
    let [head, rest @ ..] = chain else {
        return;
    };
    let named_here =
        rest.is_empty() && (ca.inline_relation(head).is_some() || catalog.table(head).is_some());
    match catalog.database(head).filter(|_| !named_here) {
        None => push_dot_columns(pool, ca, catalog, rest.last().unwrap_or(head), replace),
        Some(db) => match rest {
            [] => {
                for schema in &db.schemas {
                    pool.push(&schema.name, T_PRIMARY, || {
                        schema_item(&db.name, &schema.name, replace)
                    });
                }
            }
            [schema] => {
                for relation in db.schema(schema).map(|s| &s.relations[..]).unwrap_or(&[]) {
                    pool.push(&relation.name, T_PRIMARY, || {
                        relation_item(relation, replace)
                    });
                }
            }
            _ => {}
        },
    }
}

/// The workspace half of [`push_dot_items`]: only columns of the qualified relation
/// — inline relations (CTEs, derived-table aliases) first, then catalog. Sub-ranked
/// by the composed column forces: type affinity when completing a comparison side,
/// cross-side key likelihood at ON positions, written-demotion.
fn push_dot_columns(
    pool: &mut Pool,
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
            .map(|c| !c.contains(&name.to_ascii_lowercase()))
    };
    let refs = folded_set(&ca.clause_refs);
    let written = |name: &str| refs.contains(&name.to_ascii_lowercase());
    if let Some(inline) = ca.inline_relation(rel) {
        for name in &inline.columns {
            let ord = column_ord(affinity.map(|_| true), cross_miss(name), written(name));
            pool.ordered(name, T_PRIMARY, ord, || {
                column_item(name, Some("cte"), replace)
            });
        }
    } else if let Some(t) = catalog.table(rel) {
        for c in &t.columns {
            let ord = column_ord(
                affinity.map(|k| Kind::from_arrow(&c.dtype) != k),
                cross_miss(&c.name),
                written(&c.name),
            );
            pool.ordered(&c.name, T_PRIMARY, ord, || {
                column_item(&c.name, Some(&c.dtype), replace)
            });
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
    pool: &mut Pool,
    ca: &CaretAnalysis,
    catalog: &Catalog,
    replace: &Range<usize>,
) {
    let affinity = comparand_kind(ca, catalog);
    let on_clause = ca.governing == Clause::On;
    let refs = folded_set(&ca.clause_refs);
    let written = |name: &str| refs.contains(&name.to_ascii_lowercase());
    let mut any = false;
    for tname in &ca.in_scope {
        let cross = on_clause.then(|| other_side_columns(ca, catalog, tname));
        let cross_miss = |name: &str| {
            cross
                .as_ref()
                .map(|c| !c.contains(&name.to_ascii_lowercase()))
        };
        if let Some(inline) = ca.inline_relation(tname) {
            for name in &inline.columns {
                any = true;
                let ord = column_ord(affinity.map(|_| true), cross_miss(name), written(name));
                pool.ordered(name, T_PRIMARY, ord, || {
                    column_item(name, Some(&format!("{} · cte", inline.name)), replace)
                });
            }
        } else if let Some(t) = catalog.table(tname) {
            for c in &t.columns {
                any = true;
                let ord = column_ord(
                    affinity.map(|k| Kind::from_arrow(&c.dtype) != k),
                    cross_miss(&c.name),
                    written(&c.name),
                );
                pool.ordered(&c.name, T_PRIMARY, ord, || {
                    column_item(&c.name, Some(&format!("{} · {}", t.name, c.dtype)), replace)
                });
            }
        }
    }
    if !any {
        let projected = &ca.projection;
        let mut order: Vec<(usize, usize)> = catalog
            .tables
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let have = match projected.is_empty() {
                    true => 0,
                    false => projected.iter().filter(|r| t.column(r).is_some()).count(),
                };
                (i, projected.len().saturating_sub(have).min(60) * 2)
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
                if !pool.admits(&c.name) {
                    continue;
                }
                pushed += 1;
                let ord = base + written(&c.name) as usize;
                pool.ordered(&c.name, T_SECONDARY, ord, || {
                    column_item(&c.name, Some(&format!("{} · {}", t.name, c.dtype)), replace)
                });
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
fn push_keywords(pool: &mut Pool, replace: &Range<usize>, gate_all: bool, kw_space: bool) {
    for &k in MULTI_WORD {
        let ctx = if gate_all { T_TAIL } else { T_KEYWORD };
        pool.keyword(k, ctx, gate_all, || keyword(k, replace, kw_space));
    }
    let tail_possible = pool.tail_possible();
    if gate_all && !tail_possible {
        return;
    }
    for &k in ALL_KEYWORDS {
        let core = !gate_all && CORE_KEYWORDS.iter().any(|c| c.eq_ignore_ascii_case(k));
        if !core && !tail_possible {
            continue;
        }
        if !pool.admits(k) {
            continue;
        }
        if BLOCKED_KEYWORDS.iter().any(|b| b.eq_ignore_ascii_case(k)) {
            continue;
        }
        pool.keyword(k, if core { T_KEYWORD } else { T_TAIL }, !core, || {
            keyword(k, replace, kw_space)
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
fn push_set_option_items(pool: &mut Pool, set_key: Option<&str>, replace: &Range<usize>) {
    match set_key {
        None => {
            for (i, k) in ENGINE_KEYS
                .iter()
                .filter(|k| refuse_reserved_key(k.key).is_ok())
                .enumerate()
            {
                pool.ordered(k.key, T_PRIMARY, i, || Completion {
                    label: k.key.to_string(),
                    insert: k.key.to_string(),
                    kind: CompletionKind::Column,
                    detail: Some(k.default.to_string()),
                    replace: replace.clone(),
                });
            }
        }
        Some(key) => {
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
fn push_value_words(pool: &mut Pool, values: &[&str], replace: &Range<usize>) {
    for (i, v) in values.iter().enumerate() {
        pool.ordered(v, T_PRIMARY, i, || Completion {
            label: v.to_string(),
            insert: v.to_string(),
            kind: CompletionKind::Keyword,
            detail: None,
            replace: replace.clone(),
        });
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
    if !sql.as_bytes()[..open]
        .windows(7)
        .any(|w| w.eq_ignore_ascii_case(b"OPTIONS"))
    {
        return None;
    }
    let toks = match close {
        Some(_) => {
            let (toks, err) = lex(sql, &catalog.dialect);
            err.is_none().then_some(toks)?
        }
        None => {
            let (toks, err) = lex(&sql[..open], &catalog.dialect);
            err.is_none().then_some(toks)?
        }
    };

    let stmt = statement_tokens(&toks, sql.len(), open);
    let head = stmt.first()?;
    if !(head.kind == TokKind::Keyword && head.eq_ci("CREATE")) {
        return None;
    }
    if refine_statement_clause(stmt, Clause::Create) != Clause::CreateExternal {
        return None;
    }
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
    let replace = match close {
        Some(c) => content..c,
        None => content..sql.len(),
    };

    let pred = stmt.iter().rev().find(|t| t.span.end <= open)?;
    let mut pool = Pool::new(&partial, manual);
    if pred.kind == TokKind::Punct && (pred.text == "(" || pred.text == ",") {
        for (i, (key, _, what)) in keys.iter().enumerate() {
            pool.ordered(key, T_PRIMARY, i, || Completion {
                label: key.to_string(),
                insert: key.to_string(),
                kind: CompletionKind::Column,
                detail: Some(what.to_string()),
                replace: replace.clone(),
            });
        }
    } else if pred.kind == TokKind::Str {
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
    Some(rank(pool))
}

fn table_item(t: &TableSym, replace: &Range<usize>) -> Completion {
    Completion {
        label: t.name.clone(),
        insert: quote_verbatim(&t.name),
        kind: if t.is_view {
            CompletionKind::View
        } else {
            CompletionKind::Table
        },
        detail: Some(if t.is_view { "view" } else { "table" }.into()),
        replace: replace.clone(),
    }
}

/// A **database connection's catalog** at a relation-target position (DB-06) — the first segment
/// of a qualified name, which is why its detail says what it is rather than what it holds:
/// accepting it leaves a name that needs a `.` after it, and the row should say so.
///
/// `Table` is the nearest existing kind and the glyph reads right; the kind is a glyph, not a
/// taxonomy (see [`cte_item`]).
fn database_item(db: &DatabaseSym, replace: &Range<usize>) -> Completion {
    Completion {
        label: db.name.clone(),
        insert: quote_verbatim(&db.name),
        kind: CompletionKind::Table,
        detail: Some("database".into()),
        replace: replace.clone(),
    }
}

/// One remote schema, offered after `catalog.` — the detail names the connection, because a
/// schema called `public` is otherwise indistinguishable from every other connection's.
fn schema_item(database: &str, name: &str, replace: &Range<usize>) -> Completion {
    Completion {
        label: name.to_string(),
        insert: quote_verbatim(name),
        kind: CompletionKind::Table,
        detail: Some(format!("{database} · schema")),
        replace: replace.clone(),
    }
}

/// One remote relation, offered after `catalog.schema.`.
fn relation_item(relation: &RelationSym, replace: &Range<usize>) -> Completion {
    Completion {
        label: relation.name.clone(),
        insert: quote_verbatim(&relation.name),
        kind: match relation.view {
            true => CompletionKind::View,
            false => CompletionKind::Table,
        },
        detail: Some(if relation.view { "view" } else { "table" }.into()),
        replace: replace.clone(),
    }
}

fn cte_item(name: &str, replace: &Range<usize>) -> Completion {
    Completion {
        label: name.to_string(),
        insert: quote_verbatim(name),
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
        insert: quote_verbatim(&p.name),
        kind: CompletionKind::Function,
        detail: Some(p.detail()),
        replace: replace.clone(),
    }
}

fn column_item(name: &str, detail: Option<&str>, replace: &Range<usize>) -> Completion {
    Completion {
        label: name.to_string(),
        insert: quote_verbatim(name),
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
