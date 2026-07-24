//! Statement splitting + **caret clause-context** over the token stream. Heuristic,
//! not a full parse (mid-edit SQL rarely parses) — enough to drive completion:
//! what does the caret sit after, and which relations are in scope?

use std::ops::Range;

use crate::engine::sql::lex::{Tok, TokKind};

/// The clause governing the caret — one rung of the statement's clause ladder
/// (`SELECT → FROM → WHERE → GROUP BY → HAVING → QUALIFY → ORDER BY → LIMIT →
/// OFFSET`). `On` is the FROM zone's nested predicate; `Start` is an empty
/// statement position (including a `FROM (` derived-table opening).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clause {
    Start,
    Select,
    From,
    On,
    Where,
    GroupBy,
    Having,
    Qualify,
    OrderBy,
    Limit,
    Offset,
    /// `DESCRIBE <relation>` — an inspection statement whose operand is a relation
    /// name, like a FROM target.
    Describe,
    /// `OVER (PARTITION BY …)` — the window spec's key list. Its own region (the
    /// select list's refs don't demote a partition key) with window-shaped
    /// continuations.
    PartitionBy,
    Unknown,
}

/// What the grammar expects at the caret **within** its clause. Every clause
/// alternates between the two: an item is being started (after the clause keyword,
/// a comma, an operator, `(`) or the item just written is complete (after an
/// identifier, literal, `)`, `END`, the projection `*`). Operand positions want
/// columns/functions/relations; continuation positions want operators and the
/// next clauses of the ladder. This is the whole ranking model — clause × role —
/// not per-position special cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Operand,
    Continuation,
    /// A fresh name (or an unmodeled statement noun) is being written — after
    /// `AS`, after `SHOW` — and nothing existing can complete an invention:
    /// the offer is empty by definition, not by suppression.
    Binding,
}

/// What the caret position expects — the completion provider keys off this.
#[derive(Clone, Debug, PartialEq)]
pub enum Context {
    /// After `alias.` / `relation.` → columns of the resolved relation (name held
    /// here — resolution order: FROM/JOIN alias → CTE → catalog name).
    Dot(String),
    /// A grammar position: governing clause + expected role.
    At(Clause, Role),
}

/// A common-table-expression captured from `WITH name AS ( … )` — completion offers
/// the name as a relation and (best-effort) its projection as columns.
#[derive(Clone, Debug, PartialEq)]
pub struct CteSym {
    pub name: String,
    /// Projection column names: the explicit `WITH x (a, b) AS` list when given,
    /// else scraped from the body's SELECT list (`AS` aliases + bare column refs).
    pub columns: Vec<String>,
}

/// The caret's clause context plus the partial word being typed and the relations in
/// scope for the current statement.
pub struct CaretAnalysis {
    pub context: Context,
    /// The word currently under/just-before the caret (what completion filters on).
    pub partial: String,
    /// Byte span to replace when a completion is accepted (the partial word).
    pub replace: Range<usize>,
    /// `alias → relation` bindings from the current statement's FROM/JOIN.
    pub aliases: Vec<(String, String)>,
    /// Relation names in scope (FROM/JOIN targets of the current statement).
    pub in_scope: Vec<String>,
    /// Column aliases defined in the SELECT list (`expr AS name`) — referenceable in
    /// GROUP BY / ORDER BY / HAVING.
    pub select_aliases: Vec<String>,
    /// CTEs defined by the statement's `WITH` clause.
    pub ctes: Vec<CteSym>,
    /// Column **references** written in the caret's SELECT list (`SELECT name,
    /// u.tags` → `name`, `tags`) — source names, not output aliases; scoped to the
    /// caret's set-op branch and paren scope. Completion uses these to *rank*
    /// (FROM-target coverage, fallback clustering) — never to filter.
    pub projection: Vec<String>,
    /// Column references written in the **caret's own clause list** (its SELECT
    /// list, its GROUP BY list, its WHERE…) — the written-demotion region: an
    /// already-referenced candidate is the less likely next pick *in that list*.
    pub clause_refs: Vec<String>,
    /// The clause governing the caret — carried even for [`Context::Dot`]
    /// positions (an `ON e.|` wants join-key ranking; a `SELECT e.|` doesn't).
    pub governing: Clause,
    /// The column ref on the other side of a trailing comparison operator
    /// (`ON e.user_id = u.|` → `(Some("e"), "user_id")`) — completion resolves its
    /// type and ranks same-family columns first (`a.int = b.string` is legal but
    /// rarely meant).
    pub comparand: Option<(Option<String>, String)>,
    /// Derived tables — `FROM ( subquery ) [AS] alias` — captured like inline
    /// CTEs (alias + scraped projection) for dot- and scope-resolution. Never
    /// offered as FROM targets: a derived table binds one spot.
    pub derived: Vec<CteSym>,
}

impl CaretAnalysis {
    /// Resolve a CTE by name (case-insensitive).
    pub fn cte(&self, name: &str) -> Option<&CteSym> {
        self.ctes.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Resolve an inline relation — CTE first, then a derived-table alias — the
    /// shared lookup for dot-completion and scope columns.
    pub fn inline_relation(&self, name: &str) -> Option<&CteSym> {
        self.cte(name).or_else(|| {
            self.derived
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(name))
        })
    }
}

/// Keywords that lead into a JOIN (`LEFT`, `INNER`, …) — after one of these the next
/// sensible token is `JOIN` (or another modifier), not a relation name.
const JOIN_LEADINS: &[&str] = &[
    "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "NATURAL", "OUTER", "LATERAL", "SEMI", "ANTI",
];

/// Words that **expect an operand after them** — connectives, expression heads, and
/// quantifiers. Every one is a legal identifier (the parser's reserved tables
/// rightly admit them), so this is declarative expression-grammar knowledge of our
/// own, like the clause ladder. Three consumers: the continuation test (these never
/// *end* an item), the projection scrapers (these are never column refs), and
/// completion's identifier quoting (a column *named* `case` must be `"case"` —
/// bare, these words mean their grammar, not the column).
pub(crate) const OPERAND_EXPECTING: &[&str] = &[
    "AND", "OR", "NOT", "IN", "IS", "LIKE", "ILIKE", "BETWEEN", "CASE", "WHEN", "THEN", "ELSE",
    "DISTINCT", "ALL", "AS", "CAST", "INTERVAL", "OVER", "PARTITION", "BY", "EXISTS",
];

/// Literal/direction words — they *end* items (so the continuation test treats them
/// like identifiers) but are never column references (the scrapers skip them, and
/// quoting must protect a column actually named `null`).
pub(crate) const LITERAL_WORDS: &[&str] = &["NULL", "TRUE", "FALSE", "ASC", "DESC"];

/// Set operations end one SELECT and begin another: the position right after one
/// (`UNION |`, `UNION ALL |`, `EXCEPT |`) is a fresh statement start — the clause
/// ladder restarts, exactly like a derived-table `(` or an `EXPLAIN [ANALYZE]`
/// prefix.
const SET_OP_WORDS: &[&str] = &["UNION", "EXCEPT", "INTERSECT"];

/// Comparison operators as sqlparser renders them — one token each (`>=`, `<>`;
/// a source `!=` arrives as the `<>` rendering). The comparand capture keys off
/// these.
const COMPARISON_OPS: &[&str] = &["=", "<", ">", "<=", ">=", "<>"];

/// Map a clause keyword (from the [`governing_clause`] scan) to its [`Clause`].
fn clause_of(word: &str) -> Clause {
    let w = word.to_ascii_uppercase();
    match w.as_str() {
        "SELECT" => Clause::Select,
        "FROM" | "JOIN" => Clause::From,
        _ if JOIN_LEADINS.iter().any(|k| *k == w) => Clause::From,
        "ON" => Clause::On,
        "WHERE" => Clause::Where,
        "GROUP" => Clause::GroupBy,
        "HAVING" => Clause::Having,
        "QUALIFY" => Clause::Qualify,
        "ORDER" => Clause::OrderBy,
        "LIMIT" => Clause::Limit,
        "OFFSET" => Clause::Offset,
        "DESCRIBE" => Clause::Describe,
        "PARTITION" => Clause::PartitionBy,
        _ => Clause::Unknown,
    }
}

/// The uniform continuation test: the token just before the caret ends a complete
/// expression item. Identifiers, literals, a closing paren, and the projection
/// star (a `*` right after `SELECT`/`DISTINCT`/`ALL`/a list comma — distinguished
/// from multiplication, where an operand follows). Keyword tokens use the same
/// [`is_name_like`] predicate as every other name position — a column named
/// `status`, a literal `NULL`, a direction `ASC` all end an item exactly like a
/// plain identifier — *minus* the [`OPERAND_EXPECTING`] connectives (after `AND` /
/// `DISTINCT` / `WHEN` an operand starts, whatever the reserved tables say). `END`
/// is the one reserved word that also terminates (it closes a `CASE`).
fn item_complete(prev: Option<&Tok>, prev2: Option<&Tok>) -> bool {
    let Some(t) = prev else {
        return false;
    };
    match t.kind {
        TokKind::Ident | TokKind::QuotedIdent | TokKind::Str | TokKind::Num => true,
        TokKind::Punct => t.text == ")",
        TokKind::Op if t.text == "*" => prev2.is_none_or(|p| {
            (p.kind == TokKind::Keyword
                && (p.eq_ci("SELECT") || p.eq_ci("DISTINCT") || p.eq_ci("ALL")))
                || (p.kind == TokKind::Punct && p.text == ",")
        }),
        TokKind::Keyword => {
            (is_name_like(t) && !OPERAND_EXPECTING.iter().any(|w| t.eq_ci(w)))
                || t.eq_ci("END")
        }
        _ => false,
    }
}

/// The expected role at the caret. The FROM zone alternates on its own tokens
/// (targets after `FROM`/`JOIN`/a list comma); every other clause alternates on
/// [`item_complete`] — including `LIMIT`/`OFFSET`, where the written number is the
/// complete item (`LIMIT 5 |` continues with `OFFSET`).
fn role_at(clause: Clause, prev: Option<&Tok>, prev2: Option<&Tok>) -> Role {
    match clause {
        Clause::Start => Role::Operand,
        Clause::From => {
            let target = prev.is_some_and(|t| {
                (t.kind == TokKind::Keyword && (t.eq_ci("FROM") || t.eq_ci("JOIN")))
                    || (t.kind == TokKind::Punct && t.text == ",")
            });
            if target {
                Role::Operand
            } else {
                Role::Continuation
            }
        }
        Clause::Describe => {
            // `DESCRIBE |` expects the relation; after it, the statement is done.
            if prev.is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("DESCRIBE")) {
                Role::Operand
            } else {
                Role::Continuation
            }
        }
        _ => {
            if item_complete(prev, prev2) {
                Role::Continuation
            } else {
                Role::Operand
            }
        }
    }
}

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

/// Extract `alias → relation` from the FROM/JOIN items of the token slice's **own
/// level** (depth 0 relative to the slice — pass the caret's scope region and a
/// nested subquery's FROM binds nothing here). Best-effort: after a `FROM`/`JOIN`
/// keyword, read `ident [AS] [alias]`.
fn aliases_of(toks: &[Tok]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < toks.len() {
        match toks[i].kind {
            TokKind::Punct if toks[i].text == "(" => depth += 1,
            TokKind::Punct if toks[i].text == ")" => depth -= 1,
            _ => {}
        }
        let is_from = depth == 0 && toks[i].kind == TokKind::Keyword && toks[i].eq_ci("FROM");
        let is_join = depth == 0 && toks[i].kind == TokKind::Keyword && toks[i].eq_ci("JOIN");
        if is_from || is_join {
            // The relation name is the next identifier-ish token.
            if let Some(tbl) = toks.get(i + 1).filter(|t| is_name_like(t)) {
                let table = tbl.text.clone();
                // Optional `AS`, then an optional alias identifier.
                let mut j = i + 2;
                if toks.get(j).map(|t| t.eq_ci("AS")).unwrap_or(false) {
                    j += 1;
                }
                let alias = toks
                    .get(j)
                    .filter(|t| is_name_like(t))
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

/// A token acceptable as an identifier in a name position (aliases, projection
/// items, CTE names). sqlparser tags every known keyword `Keyword` regardless of
/// position, and half the world's columns are named `name`/`status`/`type` — so name
/// positions accept keywords unless the parser's own reserved-for-alias tables say
/// they terminate the slot ([`crate::engine::sql::lex::is_reserved_in_name_position`]).
fn is_name_like(t: &Tok) -> bool {
    is_name(t)
        || (t.kind == TokKind::Keyword
            && !crate::engine::sql::lex::is_reserved_in_name_position(&t.text))
}

/// Column aliases from the **main** SELECT projection list (`… AS <ident>`, between
/// SELECT and FROM at paren depth 0 — CTE bodies and subqueries scope their own) —
/// referenceable later in GROUP BY / ORDER BY / HAVING. Only explicit `AS` aliases
/// (not the ambiguous implicit `expr alias` form).
fn column_aliases(toks: &[Tok]) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_select = false;
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            TokKind::Punct if t.text == "(" => depth += 1,
            TokKind::Punct if t.text == ")" => depth -= 1,
            _ => {}
        }
        if depth != 0 {
            continue;
        }
        if t.kind == TokKind::Keyword && t.eq_ci("SELECT") {
            in_select = true;
        } else if t.kind == TokKind::Keyword && t.eq_ci("FROM") {
            in_select = false;
        } else if in_select && t.kind == TokKind::Keyword && t.eq_ci("AS") {
            if let Some(next) = toks.get(i + 1).filter(|n| is_name_like(n)) {
                out.push(next.text.clone());
            }
        }
    }
    out
}

/// Best-effort projection column names of a SELECT body: explicit `AS` aliases plus
/// bare column references (`a`, `t.a`) that end a projection item — i.e. an
/// identifier whose next depth-0 token is `,` or `FROM` (or the slice end) and which
/// isn't a function call. Good enough to make `cte.` completion useful; expressions
/// without aliases are simply not captured.
fn projection_columns(body: &[Tok]) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_select = false;
    for (i, t) in body.iter().enumerate() {
        match t.kind {
            TokKind::Punct if t.text == "(" => depth += 1,
            TokKind::Punct if t.text == ")" => depth -= 1,
            _ => {}
        }
        if depth != 0 {
            continue;
        }
        if t.kind == TokKind::Keyword && t.eq_ci("SELECT") {
            in_select = true;
            continue;
        }
        if t.kind == TokKind::Keyword && t.eq_ci("FROM") {
            break;
        }
        if !in_select {
            continue;
        }
        if t.kind == TokKind::Keyword && t.eq_ci("AS") {
            if let Some(next) = body.get(i + 1).filter(|n| is_name_like(n)) {
                out.push(next.text.clone());
            }
            continue;
        }
        // Connectives and literals are never projection columns (shared tables —
        // a CTE body of `SELECT NULL` must not yield a column named `null`).
        if t.kind == TokKind::Keyword
            && (OPERAND_EXPECTING.iter().any(|w| t.eq_ci(w))
                || LITERAL_WORDS.iter().any(|w| t.eq_ci(w)))
        {
            continue;
        }
        if is_name_like(t) {
            let next = body.get(i + 1);
            let ends_item = match next {
                None => true,
                Some(n) => {
                    (n.kind == TokKind::Punct && n.text == ",")
                        || (n.kind == TokKind::Keyword && n.eq_ci("FROM"))
                }
            };
            // Skip the item if it was already captured via `AS` (prev token is AS).
            let after_as = i > 0 && body[i - 1].kind == TokKind::Keyword && body[i - 1].eq_ci("AS");
            if ends_item && !after_as {
                out.push(t.text.clone());
            }
        }
    }
    out
}

/// Byte bounds of the set-operation **branch** containing `caret` within the
/// statement (top-level `UNION`/`EXCEPT`/`INTERSECT` split — the same technique as
/// [`statement_bounds`] on `;`). Written-reference regions never cross a branch:
/// set-op branches repeat each other's shapes *by design* (their schemas must
/// align), so one branch's references must neither demote nor coverage-boost
/// another's fresh list.
fn branch_bounds(stmt: &[Tok], lo: usize, hi: usize, caret: usize) -> (usize, usize) {
    let (mut start, mut end) = (lo, hi);
    let mut depth = 0i32;
    for t in stmt {
        match t.kind {
            TokKind::Punct if t.text == "(" => depth += 1,
            TokKind::Punct if t.text == ")" => depth -= 1,
            TokKind::Keyword if depth == 0 && SET_OP_WORDS.iter().any(|w| t.eq_ci(w)) => {
                if t.span.end <= caret {
                    start = t.span.end;
                } else {
                    end = t.span.start;
                    break;
                }
            }
            _ => {}
        }
    }
    (start, end)
}

/// The paren scope each token resides in — contents of a paren are one deeper;
/// the parens themselves belong to the outer level.
fn scopes(toks: &[Tok]) -> Vec<i32> {
    let mut out = Vec::with_capacity(toks.len());
    let mut d = 0i32;
    for t in toks {
        match t.kind {
            TokKind::Punct if t.text == "(" => {
                out.push(d);
                d += 1;
            }
            TokKind::Punct if t.text == ")" => {
                d -= 1;
                out.push(d);
            }
            _ => out.push(d),
        }
    }
    out
}

/// Walk the scope chain backwards from `limit`, rebasing outward at every scope
/// drop, and return the first token at the current scope satisfying `pred` — the
/// shared scan behind clause governance and select-list location. A grouping
/// paren (`WHERE (a AND |`) defers outward; a subquery's tokens never leak out
/// (`… > (SELECT x FROM t) AND |` is governed by WHERE, not the inner FROM).
fn scope_chain_rfind(
    branch: &[Tok],
    branch_scopes: &[i32],
    limit: usize,
    caret_scope: i32,
    pred: impl Fn(&Tok) -> bool,
) -> Option<usize> {
    let mut scope = caret_scope;
    for i in (0..branch.len()).rev() {
        if branch[i].span.end > limit {
            continue;
        }
        let s = branch_scopes[i];
        if s < scope {
            scope = s;
        }
        if s == scope && pred(&branch[i]) {
            return Some(i);
        }
    }
    None
}

/// The clause keyword governing the caret (see [`scope_chain_rfind`]).
fn governing_clause(
    branch: &[Tok],
    branch_scopes: &[i32],
    limit: usize,
    caret_scope: i32,
) -> Option<usize> {
    scope_chain_rfind(branch, branch_scopes, limit, caret_scope, |t| {
        t.kind == TokKind::Keyword && clause_of(&t.text) != Clause::Unknown
    })
}

/// Token-index range of the caret's **enclosing paren scope** within the branch —
/// the whole branch at top level, the contents of the caret's innermost paren
/// otherwise. Name binding (FROM/JOIN aliases, derived tables, select aliases) is
/// scoped here: a subquery's base tables bind *its* scope and never leak outward,
/// and the outer scope's don't shadow the inner's.
fn scope_region(
    branch: &[Tok],
    branch_scopes: &[i32],
    limit: usize,
    caret_scope: i32,
) -> std::ops::Range<usize> {
    let Some(idx) = branch.iter().rposition(|t| t.span.end <= limit) else {
        return 0..branch.len();
    };
    let mut start = idx + 1;
    while start > 0 && branch_scopes[start - 1] >= caret_scope {
        start -= 1;
    }
    let mut end = idx + 1;
    while end < branch.len() && branch_scopes[end] >= caret_scope {
        end += 1;
    }
    start..end
}

/// Token-index range of the clause list led by the clause keyword at `gov`: up to
/// the next clause keyword in the same scope, the scope's closing paren, or the
/// branch end.
fn clause_region(branch: &[Tok], branch_scopes: &[i32], gov: usize) -> std::ops::Range<usize> {
    let scope = branch_scopes[gov];
    let mut end = branch.len();
    for (i, t) in branch.iter().enumerate().skip(gov + 1) {
        if branch_scopes[i] < scope {
            end = i;
            break;
        }
        if branch_scopes[i] == scope
            && t.kind == TokKind::Keyword
            && clause_of(&t.text) != Clause::Unknown
        {
            end = i;
            break;
        }
    }
    gov + 1..end
}

/// Column references written in a clause region: identifiers at the region's own
/// scope that aren't function calls, `AS` output aliases, dot-qualifiers (`u.name`
/// contributes `name`), or grammar words (shared tables). Deliberately loose —
/// `a + b` contributes both — because consumers only *rank* with these (the
/// coverage boost and the written-demotion), never filter.
fn refs_in(
    branch: &[Tok],
    branch_scopes: &[i32],
    region: std::ops::Range<usize>,
    scope: i32,
) -> Vec<String> {
    let mut out = Vec::new();
    for i in region {
        let t = &branch[i];
        if branch_scopes[i] != scope || !is_name_like(t) {
            continue;
        }
        if t.kind == TokKind::Keyword
            && (OPERAND_EXPECTING.iter().any(|w| t.eq_ci(w))
                || LITERAL_WORDS.iter().any(|w| t.eq_ci(w)))
        {
            continue;
        }
        let prev = i.checked_sub(1).and_then(|j| branch.get(j));
        let next = branch.get(i + 1);
        let after_as = prev.is_some_and(|p| p.kind == TokKind::Keyword && p.eq_ci("AS"));
        let is_call = next.is_some_and(|n| n.kind == TokKind::Punct && n.text == "(");
        let is_qualifier = next.is_some_and(|n| n.kind == TokKind::Punct && n.text == ".");
        if !after_as && !is_call && !is_qualifier {
            out.push(t.text.clone());
        }
    }
    out
}

/// The refs of the nearest `SELECT` list in the caret's scope chain — the
/// projection driving the coverage boost (and, when the caret is *in* that list,
/// identical to its clause refs).
fn select_refs(
    branch: &[Tok],
    branch_scopes: &[i32],
    limit: usize,
    caret_scope: i32,
) -> Vec<String> {
    scope_chain_rfind(branch, branch_scopes, limit, caret_scope, |t| {
        t.kind == TokKind::Keyword && t.eq_ci("SELECT")
    })
    .map(|i| {
        let region = clause_region(branch, branch_scopes, i);
        refs_in(branch, branch_scopes, region, branch_scopes[i])
    })
    .unwrap_or_default()
}

/// Capture the statement's CTEs: `WITH [RECURSIVE] name [(col, …)] AS ( body )`,
/// chained with commas. Paren-depth tracked; a body left unclosed (mid-edit) still
/// yields the CTE name (columns from whatever body tokens exist).
fn ctes_of(stmt: &[Tok]) -> Vec<CteSym> {
    let mut out = Vec::new();
    // First WITH in the statement. (A WITH nested in a subquery would match too —
    // its CTE is then treated as statement-visible, which over-offers harmlessly.)
    let mut i = match stmt
        .iter()
        .position(|t| t.kind == TokKind::Keyword && t.eq_ci("WITH"))
    {
        Some(i) => i + 1,
        None => return out,
    };
    if stmt.get(i).map(|t| t.eq_ci("RECURSIVE")).unwrap_or(false) {
        i += 1;
    }
    loop {
        // name
        let Some(name_tok) = stmt.get(i).filter(|t| is_name_like(t)) else {
            break;
        };
        let name = name_tok.text.clone();
        i += 1;
        // optional explicit column list `(a, b)`
        let mut explicit_cols: Vec<String> = Vec::new();
        if stmt.get(i).map(|t| t.text == "(").unwrap_or(false)
            && stmt
                .get(i + 1)
                .map(|t| is_name_like(t) || t.text == ")")
                .unwrap_or(false)
            // Only a column list if `AS` follows the close paren — otherwise this
            // paren is something else entirely.
            && {
                let close = matching_paren(stmt, i);
                close
                    .and_then(|c| stmt.get(c + 1))
                    .map(|t| t.eq_ci("AS"))
                    .unwrap_or(false)
            }
        {
            let close = matching_paren(stmt, i).unwrap_or(stmt.len());
            for t in &stmt[i + 1..close.min(stmt.len())] {
                if is_name_like(t) {
                    explicit_cols.push(t.text.clone());
                }
            }
            i = close + 1;
        }
        // AS
        if !stmt.get(i).map(|t| t.eq_ci("AS")).unwrap_or(false) {
            break;
        }
        i += 1;
        // ( body )
        if !stmt.get(i).map(|t| t.text == "(").unwrap_or(false) {
            break;
        }
        let open = i;
        let close = matching_paren(stmt, open);
        let body_end = close.unwrap_or(stmt.len());
        let body = &stmt[open + 1..body_end.min(stmt.len())];
        let columns = if explicit_cols.is_empty() {
            projection_columns(body)
        } else {
            explicit_cols
        };
        out.push(CteSym { name, columns });
        let Some(close) = close else {
            break; // unterminated body — mid-edit
        };
        i = close + 1;
        // chained `, next_cte AS ( … )`
        if stmt.get(i).map(|t| t.text == ",").unwrap_or(false) {
            i += 1;
            continue;
        }
        break;
    }
    out
}

/// Derived tables — `FROM ( subquery ) [AS] alias` — captured as inline CTEs: the
/// alias binds to the subquery's scraped projection (the same scraper CTE bodies
/// use), giving `t.` and in-scope resolution. Returns the syms plus their
/// self-alias binds.
fn derived_tables(branch: &[Tok]) -> (Vec<CteSym>, Vec<(String, String)>) {
    let mut out = Vec::new();
    let mut binds = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < branch.len() {
        match branch[i].kind {
            TokKind::Punct if branch[i].text == "(" => depth += 1,
            TokKind::Punct if branch[i].text == ")" => depth -= 1,
            _ => {}
        }
        let lead = depth == 0
            && branch[i].kind == TokKind::Keyword
            && (branch[i].eq_ci("FROM") || branch[i].eq_ci("JOIN"));
        if lead
            && branch
                .get(i + 1)
                .is_some_and(|t| t.kind == TokKind::Punct && t.text == "(")
        {
            if let Some(close) = matching_paren(branch, i + 1) {
                let body = &branch[i + 2..close];
                let mut j = close + 1;
                if branch.get(j).is_some_and(|t| t.eq_ci("AS")) {
                    j += 1;
                }
                if let Some(name_tok) = branch.get(j).filter(|t| is_name_like(t)) {
                    let name = name_tok.text.clone();
                    out.push(CteSym {
                        name: name.clone(),
                        columns: projection_columns(body),
                    });
                    binds.push((name.clone(), name));
                }
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    (out, binds)
}

/// Index of the `)` matching the `(` at `open` (same nesting level), if present.
fn matching_paren(toks: &[Tok], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate().skip(open) {
        if t.kind == TokKind::Punct && t.text == "(" {
            depth += 1;
        } else if t.kind == TokKind::Punct && t.text == ")" {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Analyse the caret: partial word, clause context, and in-scope relations.
pub fn analyze_caret(sql: &str, caret: usize, toks: &[Tok]) -> CaretAnalysis {
    let caret = caret.min(sql.len());
    let (lo, hi) = statement_bounds(toks, sql.len(), caret);
    let stmt: Vec<Tok> = toks
        .iter()
        .filter(|t| t.span.start >= lo && t.span.end <= hi)
        .cloned()
        .collect();

    // Reference scoping: the caret's set-op branch (regions never cross a
    // branch) with per-token paren scopes. CTEs stay statement-scoped (a WITH
    // prefixes the whole statement); aliases/refs are branch-scoped.
    let (blo, bhi) = branch_bounds(&stmt, lo, hi, caret);
    let branch: Vec<Tok> = stmt
        .iter()
        .filter(|t| t.span.start >= blo && t.span.end <= bhi)
        .cloned()
        .collect();
    let branch_scopes = scopes(&branch);

    let ctes = ctes_of(&stmt);

    // The partial word = a name/keyword token whose span ends exactly at the caret
    // (i.e. we're typing its tail). Otherwise the caret sits after some other token.
    let partial_tok = stmt.iter().find(|t| {
        t.span.end == caret
            && matches!(
                t.kind,
                TokKind::Ident | TokKind::Keyword | TokKind::QuotedIdent
            )
    });
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

    // The caret's paren scope, then the clause governing it (scope-aware).
    let caret_scope = branch
        .iter()
        .filter(|t| t.span.end <= replace.start)
        .fold(0i32, |d, t| match t.kind {
            TokKind::Punct if t.text == "(" => d + 1,
            TokKind::Punct if t.text == ")" => d - 1,
            _ => d,
        });
    // Name binding is scoped to the caret's enclosing paren scope: inside a CTE
    // body or subquery its own FROMs bind; outside, they don't leak.
    let scope_toks = &branch[scope_region(&branch, &branch_scopes, replace.start, caret_scope)];
    let mut aliases = aliases_of(scope_toks);
    let (derived, derived_binds) = derived_tables(scope_toks);
    aliases.extend(derived_binds);
    let in_scope: Vec<String> = aliases.iter().map(|(_, t)| t.clone()).collect();
    let select_aliases = column_aliases(scope_toks);

    let gov_idx = governing_clause(&branch, &branch_scopes, replace.start, caret_scope);
    let governing = gov_idx
        .map(|i| clause_of(&branch[i].text))
        .unwrap_or(Clause::Unknown);

    // The written-reference regions: the caret's own clause list (demotion) and
    // its nearest SELECT list (coverage).
    let clause_refs = gov_idx
        .map(|i| {
            let region = clause_region(&branch, &branch_scopes, i);
            refs_in(&branch, &branch_scopes, region, branch_scopes[i])
        })
        .unwrap_or_default();
    let projection = select_refs(&branch, &branch_scopes, replace.start, caret_scope);

    // A trailing comparison — `… e.user_id = |` or `… e.user_id = u.|` (the caret
    // may already be inside the far side's qualifier): capture the other side's
    // column ref so completion can rank same-name and same-type-family candidates
    // first. The dot-qualified caret shape is trimmed first, so the capture works
    // identically at bare and dotted positions.
    let comparand = {
        let mut tail: &[&Tok] = &before;
        // Strip a trailing `qualifier .` (the caret's own side of the comparison).
        if tail
            .last()
            .is_some_and(|t| t.kind == TokKind::Punct && t.text == ".")
            && tail.len() >= 2
            && is_name_like(tail[tail.len() - 2])
        {
            tail = &tail[..tail.len() - 2];
        }
        tail.last()
            .filter(|t| t.kind == TokKind::Op && COMPARISON_OPS.iter().any(|op| t.text == *op))
            .and_then(|_| {
                let n = tail.len();
                let operand = tail.get(n.wrapping_sub(2)).copied()?;
                if !is_name_like(operand) {
                    return None;
                }
                // `qual . column` or bare `column`.
                let dotted = tail
                    .get(n.wrapping_sub(3))
                    .copied()
                    .filter(|d| d.kind == TokKind::Punct && d.text == ".");
                let qualifier = dotted
                    .and_then(|_| tail.get(n.wrapping_sub(4)).copied())
                    .filter(|q| is_name_like(q))
                    .map(|q| q.text.clone());
                Some((qualifier, operand.text.clone()))
            })
    };

    let context = if prev.is_none() {
        Context::At(Clause::Start, Role::Operand)
    } else if prev
        .map(|t| t.kind == TokKind::Punct && t.text == ".")
        .unwrap_or(false)
    {
        // `x.` → columns of x (resolution: alias → CTE → catalog name; the resolved
        // name is carried, complete() checks CTEs first).
        let owner = prev2.map(|t| t.text.clone()).unwrap_or_default();
        let resolved = aliases
            .iter()
            .find(|(a, _)| a.eq_ignore_ascii_case(&owner))
            .map(|(_, t)| t.clone())
            .unwrap_or(owner);
        Context::Dot(resolved)
    } else if prev
        .map(|t| t.kind == TokKind::Punct && t.text == "(")
        .unwrap_or(false)
        && governing == Clause::From
    {
        // `FROM ( |` — a derived table starts a fresh statement position.
        Context::At(Clause::Start, Role::Operand)
    } else if prev.is_some_and(|t| {
        t.kind == TokKind::Keyword
            && (SET_OP_WORDS.iter().any(|w| t.eq_ci(w))
                || (t.eq_ci("ALL") && prev2.is_some_and(|p| p.eq_ci("UNION")))
                || t.eq_ci("EXPLAIN")
                || (t.eq_ci("ANALYZE") && prev2.is_some_and(|p| p.eq_ci("EXPLAIN"))))
    }) {
        // `… UNION ALL |` / `EXPLAIN [ANALYZE] |` — a fresh statement begins:
        // the ladder restarts (deeper positions inside that branch already
        // resolve to its own clauses via the nearest-clause scan).
        Context::At(Clause::Start, Role::Operand)
    } else if prev.is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("AS"))
        || prev.is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("SHOW"))
        || prev2.is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("SHOW"))
    {
        // `… AS |` invents a name; `SHOW <noun> |` is an unmodeled statement
        // noun. Nothing existing completes either — the empty offer is the
        // correct one, not a suppression.
        Context::At(governing, Role::Binding)
    } else {
        Context::At(governing, role_at(governing, prev, prev2))
    };

    CaretAnalysis {
        context,
        partial,
        replace,
        aliases,
        in_scope,
        select_aliases,
        ctes,
        projection,
        clause_refs,
        governing,
        comparand,
        derived,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::sql::lex::lex;

    /// Analyse with the caret at the `|` marker.
    fn at(sql_with_caret: &str) -> CaretAnalysis {
        let caret = sql_with_caret.find('|').expect("caret marker");
        let sql = sql_with_caret.replace('|', "");
        let (toks, _) = lex(&sql);
        analyze_caret(&sql, caret, &toks)
    }

    #[test]
    fn statement_start_and_select_list() {
        assert_eq!(at("|").context, Context::At(Clause::Start, Role::Operand));
        assert_eq!(at("SELECT |").context, Context::At(Clause::Select, Role::Operand));
        assert_eq!(
            at("SELECT a, | FROM t").context,
            Context::At(Clause::Select, Role::Operand)
        );
    }

    #[test]
    fn from_target_vs_from_continuation() {
        assert_eq!(at("SELECT * FROM |").context, Context::At(Clause::From, Role::Operand));
        assert_eq!(at("SELECT * FROM eve|").context, Context::At(Clause::From, Role::Operand));
        assert_eq!(
            at("SELECT * FROM events |").context,
            Context::At(Clause::From, Role::Continuation)
        );
        assert_eq!(
            at("SELECT * FROM events, |").context,
            Context::At(Clause::From, Role::Operand)
        );
        assert_eq!(
            at("SELECT * FROM events, users |").context,
            Context::At(Clause::From, Role::Continuation)
        );
    }

    #[test]
    fn join_positions() {
        assert_eq!(
            at("SELECT * FROM a JOIN |").context,
            Context::At(Clause::From, Role::Operand)
        );
        assert_eq!(
            at("SELECT * FROM a LEFT JOIN |").context,
            Context::At(Clause::From, Role::Operand)
        );
        // After a join lead-in the JOIN keyword itself comes next, not a relation.
        assert_eq!(
            at("SELECT * FROM a LEFT |").context,
            Context::At(Clause::From, Role::Continuation)
        );
        assert_eq!(
            at("SELECT * FROM a JOIN b |").context,
            Context::At(Clause::From, Role::Continuation)
        );
    }

    #[test]
    fn expression_operand_positions() {
        assert_eq!(
            at("SELECT * FROM t WHERE |").context,
            Context::At(Clause::Where, Role::Operand)
        );
        assert_eq!(
            at("SELECT * FROM t GROUP BY |").context,
            Context::At(Clause::GroupBy, Role::Operand)
        );
        assert_eq!(
            at("SELECT * FROM a JOIN b ON |").context,
            Context::At(Clause::On, Role::Operand)
        );
        assert_eq!(
            at("SELECT * FROM t ORDER BY x, |").context,
            Context::At(Clause::OrderBy, Role::Operand)
        );
    }

    #[test]
    fn continuation_after_a_complete_item() {
        // The screenshot bug: `SELECT * f` must be a continuation (FROM), not an
        // operand (floor/flatten/…).
        assert_eq!(
            at("SELECT * f|").context,
            Context::At(Clause::Select, Role::Continuation)
        );
        assert_eq!(
            at("SELECT a |").context,
            Context::At(Clause::Select, Role::Continuation)
        );
        assert_eq!(
            at("SELECT sum(x) |").context,
            Context::At(Clause::Select, Role::Continuation)
        );
        assert_eq!(
            at("SELECT * FROM t WHERE amount > 5 |").context,
            Context::At(Clause::Where, Role::Continuation)
        );
        assert_eq!(
            at("SELECT * FROM t WHERE x IS NOT NULL |").context,
            Context::At(Clause::Where, Role::Continuation)
        );
        assert_eq!(
            at("SELECT * FROM t GROUP BY x |").context,
            Context::At(Clause::GroupBy, Role::Continuation)
        );
        assert_eq!(
            at("SELECT * FROM t LIMIT 5 |").context,
            Context::At(Clause::Limit, Role::Continuation)
        );
        // A direction keyword completes the ORDER BY item (accept-chaining must not
        // reopen an operand list after `ASC `).
        assert_eq!(
            at("SELECT * FROM t ORDER BY x ASC |").context,
            Context::At(Clause::OrderBy, Role::Continuation)
        );
    }

    #[test]
    fn multiplication_star_is_an_operand_position() {
        // `a * |` expects the right-hand operand; only the projection star
        // (`SELECT * |`) completes an item.
        assert_eq!(
            at("SELECT a * |").context,
            Context::At(Clause::Select, Role::Operand)
        );
        assert_eq!(
            at("SELECT *, |").context,
            Context::At(Clause::Select, Role::Operand)
        );
    }

    #[test]
    fn limit_operand_position() {
        assert_eq!(
            at("SELECT * FROM t LIMIT |").context,
            Context::At(Clause::Limit, Role::Operand)
        );
    }

    #[test]
    fn derived_table_paren_restarts_statement_context() {
        assert_eq!(at("SELECT * FROM (|").context, Context::At(Clause::Start, Role::Operand));
        // A paren in an expression position is not a statement start.
        assert_eq!(
            at("SELECT * FROM t WHERE (|").context,
            Context::At(Clause::Where, Role::Operand)
        );
        assert_eq!(
            at("SELECT count(|").context,
            Context::At(Clause::Select, Role::Operand)
        );
    }

    #[test]
    fn dot_resolution_prefers_alias() {
        let ca = at("SELECT o.| FROM events o");
        assert_eq!(ca.context, Context::Dot("events".into()));
        // Unaliased: the qualifier is carried as written.
        let ca = at("SELECT events.| FROM events");
        assert_eq!(ca.context, Context::Dot("events".into()));
        // Unknown qualifier is carried verbatim (complete() decides emptiness).
        let ca = at("SELECT x.| FROM events o");
        assert_eq!(ca.context, Context::Dot("x".into()));
    }

    #[test]
    fn partial_and_replace_span() {
        let ca = at("SELECT sta| FROM t");
        assert_eq!(ca.partial, "sta");
        assert_eq!(ca.replace, 7..10);
        // Mid-word caret → no partial (caret is not at the token's end).
        let ca = at("SELECT st|a FROM t");
        assert_eq!(ca.partial, "");
        assert_eq!(ca.replace, 9..9);
    }

    #[test]
    fn multi_statement_bounds() {
        // The caret's statement is the second one — its scope, not the first's.
        let ca = at("SELECT a FROM t1; SELECT b FROM t2 WHERE |");
        assert_eq!(ca.context, Context::At(Clause::Where, Role::Operand));
        assert_eq!(ca.in_scope, vec!["t2".to_string()]);
        // And the first statement is unaffected by the second.
        let ca = at("SELECT a FROM t1 WHERE |; SELECT b FROM t2");
        assert_eq!(ca.in_scope, vec!["t1".to_string()]);
    }

    #[test]
    fn aliases_and_scope() {
        let ca = at("SELECT | FROM events e JOIN users AS u ON e.id = u.id");
        assert_eq!(
            ca.aliases,
            vec![
                ("e".to_string(), "events".to_string()),
                ("u".to_string(), "users".to_string())
            ]
        );
        assert_eq!(ca.in_scope, vec!["events".to_string(), "users".to_string()]);
    }

    #[test]
    fn select_aliases_captured() {
        let ca = at("SELECT sum(x) AS total, avg(y) AS mean FROM t ORDER BY |");
        assert_eq!(ca.select_aliases, vec!["total".to_string(), "mean".to_string()]);
    }

    #[test]
    fn projection_refs_are_source_columns_only() {
        // Bare refs + the column part of qualified refs; function names, args
        // (depth > 0), and AS output aliases are not source columns.
        let ca = at("SELECT name, u.tags, sum(x) AS spend FROM |");
        assert_eq!(ca.projection, vec!["name".to_string(), "tags".to_string()]);
        // CTE bodies sit inside parens — the main statement's list only.
        let ca = at("WITH r AS (SELECT amount FROM events) SELECT total FROM |");
        assert_eq!(ca.projection, vec!["total".to_string()]);
        // `*` contributes nothing.
        assert!(at("SELECT * FROM |").projection.is_empty());
    }

    // ---- CTE capture ----

    #[test]
    fn cte_names_and_bare_projection() {
        let ca = at("WITH recent AS (SELECT amount, status FROM events) SELECT | FROM recent");
        assert_eq!(ca.ctes.len(), 1);
        assert_eq!(ca.ctes[0].name, "recent");
        assert_eq!(ca.ctes[0].columns, vec!["amount".to_string(), "status".to_string()]);
    }

    #[test]
    fn cte_as_aliases_and_qualified_refs() {
        let ca = at("WITH r AS (SELECT sum(x) AS spend, t.name FROM t) SELECT | FROM r");
        assert_eq!(ca.ctes[0].columns, vec!["spend".to_string(), "name".to_string()]);
    }

    #[test]
    fn cte_explicit_column_list() {
        let ca = at("WITH r (a, b) AS (SELECT 1, 2) SELECT | FROM r");
        assert_eq!(ca.ctes[0].columns, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn chained_ctes() {
        let ca = at("WITH a AS (SELECT x FROM t), b AS (SELECT y FROM u) SELECT | FROM b");
        let names: Vec<&str> = ca.ctes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn recursive_and_unterminated_cte_bodies() {
        let ca = at("WITH RECURSIVE r AS (SELECT x FROM t) SELECT | FROM r");
        assert_eq!(ca.ctes[0].name, "r");
        // Mid-edit: the body paren isn't closed yet — the name still registers.
        let ca = at("WITH r AS (SELECT x FROM t SELECT |");
        assert_eq!(ca.ctes[0].name, "r");
    }

    #[test]
    fn cte_resolvable_via_helper() {
        let ca = at("WITH Recent AS (SELECT x FROM t) SELECT | FROM Recent");
        assert!(ca.cte("recent").is_some(), "case-insensitive lookup");
    }

    #[test]
    fn comparand_captured_bare_and_through_the_dot() {
        // Right after the operator…
        let ca = at("SELECT * FROM events e JOIN users u ON e.user_id = |");
        assert_eq!(ca.comparand, Some((Some("e".into()), "user_id".into())));
        // …and with the caret already inside the far side's qualifier.
        let ca = at("SELECT * FROM events e JOIN users u ON e.user_id = u.|");
        assert_eq!(ca.comparand, Some((Some("e".into()), "user_id".into())));
        assert_eq!(ca.governing, Clause::On);
        // Literals are not comparands.
        assert_eq!(at("SELECT * FROM t WHERE 5 = |").comparand, None);
    }

    #[test]
    fn partition_by_is_its_own_region() {
        let ca = at("SELECT user_id, sum(amount) OVER (PARTITION BY |");
        assert_eq!(ca.context, Context::At(Clause::PartitionBy, Role::Operand));
        // The select list's refs are NOT this region's — a projected column is
        // the *likely* partition key, never demoted here.
        assert!(ca.clause_refs.is_empty(), "{:?}", ca.clause_refs);
    }

    #[test]
    fn name_binding_is_scoped_to_the_caret() {
        // Inside a CTE body: its own FROM binds.
        let ca = at("WITH r AS (SELECT | FROM events)");
        assert_eq!(ca.in_scope, vec!["events".to_string()]);
        // Outside: only the top-level FROM binds — no leak from the body.
        let ca = at("WITH r AS (SELECT amount FROM events) SELECT | FROM r");
        assert_eq!(ca.in_scope, vec!["r".to_string()]);
        // A subquery's FROM stays inside its parens.
        let ca = at("SELECT name FROM users WHERE user_id > (SELECT avg(amount) FROM events) AND |");
        assert_eq!(ca.in_scope, vec!["users".to_string()]);
    }
}
