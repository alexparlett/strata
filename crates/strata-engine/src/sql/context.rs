//! Statement splitting + **caret clause-context** over the token stream. Heuristic,
//! not a full parse (mid-edit SQL rarely parses) — enough to drive completion:
//! what does the caret sit after, and which relations are in scope?

use std::ops::Range;

use crate::sql::lex::{is_reserved_in_name_position, statement_at, Tok, TokKind};

/// The clause governing the caret — one rung of the statement's clause ladder
/// (`SELECT → FROM → WHERE → GROUP BY → HAVING → QUALIFY → ORDER BY → LIMIT →
/// OFFSET`), or the statement position the caret's own statement lead puts it in
/// (ED-11). `On` is the FROM zone's nested predicate; `Start` is a truly blank
/// statement position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clause {
    Start,
    /// A **restart** position — `EXPLAIN [ANALYZE] |`, after a set operation,
    /// `FROM (|`, `COPY (|`, and the `AS |` of CTAS / `CREATE VIEW` / `PREPARE`:
    /// a fresh *query* begins. Role and continuations are `Start`'s exactly; only
    /// the lead pool differs — query leads and never statement leads, because
    /// offering `DROP TABLE` after `EXPLAIN` promises something Run refuses.
    Restart,
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
    /// `EXECUTE <name>` / `DEALLOCATE <name>` — a session statement whose operand is a
    /// **prepared statement** name (ED-08). One clause for both, because the operand is
    /// the same set of names and nothing else can complete either.
    Execute,
    /// `CREATE |` — the object word not yet written, so the offer is the object
    /// words themselves. [`refine_statement_clause`] narrows it to the specific
    /// variant once the head names one.
    Create,
    CreateTable,
    CreateView,
    CreateExternal,
    CreateFunction,
    /// `DROP |` — the object word not yet written, refined like [`Clause::Create`].
    Drop,
    DropTable,
    DropView,
    DropFunction,
    Insert,
    Copy,
    /// `SET` / `RESET` — one clause for both, because both operate on the same
    /// config-key vocabulary. The key/value positions are decided in
    /// [`analyze_caret`], not [`role_at`]: a config key is one dotted name, so the
    /// generic `Dot` rule must never read `SET datafusion.|` as a relation's columns.
    SetOption,
    Prepare,
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
    /// After `alias.` / `relation.` / `catalog.schema.` — the whole qualifier chain
    /// before the caret, outermost segment first. A **single** segment is resolved
    /// through the statement's aliases here (FROM/JOIN alias → CTE → catalog name);
    /// the longer chains are a qualified name, and only a database connection's
    /// catalog can say what is inside one, so their resolution is the pool's.
    Dot(Vec<String>),
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
    /// For a caret in a `SET k = |` **value** position, the dotted key text left of
    /// the `=` — the shape [`comparand`](Self::comparand) already has. `None`
    /// everywhere else, key positions included.
    pub set_key: Option<String>,
    /// The single-relation **column list** the caret sits inside — an INSERT's
    /// column list, a COPY's `PARTITIONED BY` group — resolved once here so the
    /// role and the pool read one answer rather than two token scans that must
    /// agree forever. `None` everywhere else, VALUES tuples and `OPTIONS` groups
    /// included.
    pub column_list: Option<ColumnList>,
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
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "LIKE",
    "ILIKE",
    "BETWEEN",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "DISTINCT",
    "ALL",
    "AS",
    "CAST",
    "INTERVAL",
    "OVER",
    "PARTITION",
    "BY",
    "EXISTS",
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

/// Map a clause keyword (from the [`last_clause`] scan) to its [`Clause`].
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
        "EXECUTE" | "DEALLOCATE" => Clause::Execute,
        "CREATE" => Clause::Create,
        "DROP" => Clause::Drop,
        "INSERT" => Clause::Insert,
        "COPY" => Clause::Copy,
        "SET" | "RESET" => Clause::SetOption,
        "PREPARE" => Clause::Prepare,
        _ => Clause::Unknown,
    }
}

/// Whether `clause`'s keyword only governs when it **leads the statement**.
///
/// Every other entry in [`clause_of`] is a word that can only appear as a clause keyword, so the
/// nearest-clause scan can take it wherever it sits. The statement leads are not: sqlparser
/// classes every word in its dictionary as a `Keyword` — including the non-reserved ones that are
/// perfectly legal column names — so a table with an `execute`, `set`, `copy`, `drop`, `insert`,
/// `create` or `prepare` column would otherwise have that column govern the rest of its SELECT
/// list, and the offer there is the statement's own (prepared names, config keys, …), not the
/// list's. A statement lead cannot be reached mid-list, so position is the whole test — exactly
/// as ED-08 built it for `EXECUTE`.
fn leads_statement_only(clause: Clause) -> bool {
    match clause {
        Clause::Execute
        | Clause::Create
        | Clause::Drop
        | Clause::Insert
        | Clause::Copy
        | Clause::SetOption
        | Clause::Prepare => true,
        Clause::Start
        | Clause::Restart
        | Clause::Select
        | Clause::From
        | Clause::On
        | Clause::Where
        | Clause::GroupBy
        | Clause::Having
        | Clause::Qualify
        | Clause::OrderBy
        | Clause::Limit
        | Clause::Offset
        | Clause::Describe
        | Clause::CreateTable
        | Clause::CreateView
        | Clause::CreateExternal
        | Clause::CreateFunction
        | Clause::DropTable
        | Clause::DropView
        | Clause::DropFunction
        | Clause::Unknown => false,
    }
}

/// Refine an unrefined statement-lead clause by the statement's **head keywords**:
/// `CREATE [OR REPLACE] TABLE|VIEW|FUNCTION`, `CREATE [OR REPLACE] EXTERNAL TABLE`,
/// `DROP TABLE|VIEW|FUNCTION`. Unrefined (`CREATE |`, `DROP |`) stays
/// [`Clause::Create`]/[`Clause::Drop`], whose role is always `Continuation` — the
/// object word comes next. `stmt` starts at the lead keyword (the position-0 guard
/// is what puts it there).
pub(crate) fn refine_statement_clause(stmt: &[Tok], clause: Clause) -> Clause {
    let word = |i: usize, w: &str| {
        stmt.get(i)
            .is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci(w))
    };
    match clause {
        Clause::Create => {
            let mut i = 1;
            if word(i, "OR") && word(i + 1, "REPLACE") {
                i += 2;
            }
            if word(i, "EXTERNAL") {
                return if word(i + 1, "TABLE") {
                    Clause::CreateExternal
                } else {
                    Clause::Create
                };
            }
            if word(i, "TABLE") {
                Clause::CreateTable
            } else if word(i, "VIEW") {
                Clause::CreateView
            } else if word(i, "FUNCTION") {
                Clause::CreateFunction
            } else {
                Clause::Create
            }
        }
        Clause::Drop => {
            if word(1, "TABLE") {
                Clause::DropTable
            } else if word(1, "VIEW") {
                Clause::DropView
            } else if word(1, "FUNCTION") {
                Clause::DropFunction
            } else {
                Clause::Drop
            }
        }
        other => other,
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
            (is_name_like(t) && !OPERAND_EXPECTING.iter().any(|w| t.eq_ci(w))) || t.eq_ci("END")
        }
        _ => false,
    }
}

/// The expected role at the caret. The FROM zone alternates on its own tokens
/// (targets after `FROM`/`JOIN`/a list comma); every other clause alternates on
/// [`item_complete`] — including `LIMIT`/`OFFSET`, where the written number is the
/// complete item (`LIMIT 5 |` continues with `OFFSET`). The statement clauses
/// (ED-11) alternate on their own head tokens instead, since a statement's grammar
/// is positional, not an expression list. `before` (every statement token before
/// the caret) exists for the one clause whose role depends on more than the two
/// preceding tokens — a `CREATE FUNCTION` body begins at its `RETURN` — and
/// `column_list` is [`analyze_caret`]'s already-resolved answer to "is the caret in
/// a single-relation column list", so the role and the pool are **one** decision:
/// re-deriving it here from tokens would be a second encoding of the same boundary,
/// free to disagree with the resolver the pool reads.
fn role_at(
    clause: Clause,
    prev: Option<&Tok>,
    prev2: Option<&Tok>,
    before: &[&Tok],
    column_list: bool,
) -> Role {
    let prev_kw = |words: &[&str]| {
        prev.is_some_and(|t| t.kind == TokKind::Keyword && words.iter().any(|w| t.eq_ci(w)))
    };
    let prev_punct = |marks: &[&str]| {
        prev.is_some_and(|t| t.kind == TokKind::Punct && marks.contains(&t.text.as_str()))
    };
    match clause {
        Clause::Start | Clause::Restart => Role::Operand,
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
            if prev.is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("DESCRIBE")) {
                Role::Operand
            } else {
                Role::Continuation
            }
        }
        Clause::Execute => {
            let named = prev.is_some_and(|t| {
                t.kind == TokKind::Keyword
                    && (t.eq_ci("EXECUTE") || t.eq_ci("DEALLOCATE") || t.eq_ci("PREPARE"))
            });
            if named {
                Role::Operand
            } else {
                Role::Continuation
            }
        }
        Clause::Create | Clause::Drop => Role::Continuation,
        Clause::DropTable | Clause::DropView | Clause::DropFunction => {
            let object = match clause {
                Clause::DropTable => "TABLE",
                Clause::DropView => "VIEW",
                _ => "FUNCTION",
            };
            if prev_kw(&[object, "EXISTS"]) || prev_punct(&[","]) {
                Role::Operand
            } else {
                Role::Continuation
            }
        }
        Clause::Insert => {
            if prev_kw(&["INTO"]) {
                Role::Operand
            } else if prev_punct(&["(", ","]) {
                if column_list {
                    Role::Operand
                } else {
                    Role::Binding
                }
            } else {
                Role::Continuation
            }
        }
        Clause::Copy => {
            if prev_kw(&["COPY"]) {
                Role::Operand
            } else if prev_punct(&["(", ","]) {
                if column_list {
                    Role::Operand
                } else {
                    Role::Binding
                }
            } else {
                Role::Continuation
            }
        }
        Clause::CreateTable | Clause::CreateView => {
            let object = if clause == Clause::CreateTable {
                "TABLE"
            } else {
                "VIEW"
            };
            if prev_kw(&[object]) || prev_punct(&["(", ","]) {
                Role::Binding
            } else {
                Role::Continuation
            }
        }
        Clause::CreateExternal => {
            if prev_kw(&["TABLE"]) || prev_punct(&["(", ","]) {
                Role::Binding
            } else {
                Role::Continuation
            }
        }
        Clause::CreateFunction => {
            let after_return = before
                .iter()
                .any(|t| t.kind == TokKind::Keyword && t.eq_ci("RETURN"));
            if after_return {
                if prev_kw(&["RETURN"]) || !item_complete(prev, prev2) {
                    Role::Operand
                } else {
                    Role::Continuation
                }
            } else if prev_kw(&["FUNCTION"]) || prev_punct(&["(", ","]) {
                Role::Binding
            } else {
                Role::Continuation
            }
        }
        Clause::Prepare => {
            if prev_kw(&["PREPARE"]) {
                Role::Binding
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
    let r = statement_at(toks, sql_len, caret);
    (r.start, r.end)
}

/// The tokens of the statement containing `at`, as a **subslice** of `toks` — tokens
/// are span-ordered, so a statement is a contiguous run and nothing needs cloning.
/// The one extraction every statement-scoped reader shares ([`analyze_caret`], the
/// column-list resolvers, [`function_arguments`], the OPTIONS carve-out), so the
/// bounds convention lives in one place.
pub(crate) fn statement_tokens(toks: &[Tok], sql_len: usize, at: usize) -> &[Tok] {
    let (lo, hi) = statement_bounds(toks, sql_len, at.min(sql_len));
    let start = toks.partition_point(|t| t.span.start < lo);
    let end = toks.partition_point(|t| t.span.end <= hi);
    &toks[start.min(end)..end]
}

/// Extract `alias → relation` from the FROM/JOIN items of the token slice. Best-effort:
/// after a `FROM`/`JOIN` keyword, read `ident [AS] [alias]`.
fn aliases_of(toks: &[Tok]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let is_from = toks[i].kind == TokKind::Keyword && toks[i].eq_ci("FROM");
        let is_join = toks[i].kind == TokKind::Keyword && toks[i].eq_ci("JOIN");
        if is_from || is_join {
            if let Some(tbl) = toks.get(i + 1).filter(|t| is_name_like(t)) {
                let table = tbl.text.clone();
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
/// they terminate the slot ([`crate::sql::lex::is_reserved_in_name_position`]).
fn is_name_like(t: &Tok) -> bool {
    is_name(t) || (t.kind == TokKind::Keyword && !is_reserved_in_name_position(&t.text))
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

/// The clause keyword governing the caret: the nearest one **in the caret's own
/// paren scope**, scanning back. Leaving an enclosing group rebases the scope
/// outward — so a grouping paren (`WHERE (a AND |`) defers to the outer clause,
/// while a subquery's inner clauses govern only inside it and never leak out
/// (`… > (SELECT x FROM t) AND |` is governed by WHERE, not the subquery's FROM).
fn governing_clause(
    branch: &[Tok],
    branch_scopes: &[i32],
    limit: usize,
    caret_scope: i32,
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
        if s == scope && branch[i].kind == TokKind::Keyword {
            let clause = clause_of(&branch[i].text);
            if clause != Clause::Unknown && !(leads_statement_only(clause) && i != 0) {
                return Some(i);
            }
        }
    }
    None
}

/// Token-index range of the clause list led by the clause keyword at `gov`: up to
/// the next clause keyword in the same scope, the scope's closing paren, or the
/// branch end.
fn clause_region(branch: &[Tok], branch_scopes: &[i32], gov: usize) -> Range<usize> {
    let scope = branch_scopes[gov];
    let mut end = branch.len();
    for (i, t) in branch.iter().enumerate().skip(gov + 1) {
        if branch_scopes[i] < scope {
            end = i;
            break;
        }
        if branch_scopes[i] == scope && t.kind == TokKind::Keyword {
            let clause = clause_of(&t.text);
            if clause != Clause::Unknown && !leads_statement_only(clause) {
                end = i;
                break;
            }
        }
    }
    gov + 1..end
}

/// Column references written in a clause region: identifiers at the region's own
/// scope that aren't function calls, `AS` output aliases, dot-qualifiers (`u.name`
/// contributes `name`), or grammar words (shared tables). Deliberately loose —
/// `a + b` contributes both — because consumers only *rank* with these (the
/// coverage boost and the written-demotion), never filter.
fn refs_in(branch: &[Tok], branch_scopes: &[i32], region: Range<usize>, scope: i32) -> Vec<String> {
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
    let mut scope = caret_scope;
    for i in (0..branch.len()).rev() {
        if branch[i].span.end > limit {
            continue;
        }
        let s = branch_scopes[i];
        if s < scope {
            scope = s;
        }
        if s == scope && branch[i].kind == TokKind::Keyword && branch[i].eq_ci("SELECT") {
            let region = clause_region(branch, branch_scopes, i);
            return refs_in(branch, branch_scopes, region, s);
        }
    }
    Vec::new()
}

/// Capture the statement's CTEs: `WITH [RECURSIVE] name [(col, …)] AS ( body )`,
/// chained with commas. Paren-depth tracked; a body left unclosed (mid-edit) still
/// yields the CTE name (columns from whatever body tokens exist).
fn ctes_of(stmt: &[Tok]) -> Vec<CteSym> {
    let mut out = Vec::new();
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
    while let Some(name_tok) = stmt.get(i).filter(|t| is_name_like(t)) {
        let name = name_tok.text.clone();
        i += 1;
        let mut explicit_cols: Vec<String> = Vec::new();
        if stmt.get(i).map(|t| t.text == "(").unwrap_or(false)
            && stmt
                .get(i + 1)
                .map(|t| is_name_like(t) || t.text == ")")
                .unwrap_or(false)
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
        if !stmt.get(i).map(|t| t.eq_ci("AS")).unwrap_or(false) {
            break;
        }
        i += 1;
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
            break;
        };
        i = close + 1;
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
    let mut i = 0;
    while i < branch.len() {
        let lead = branch[i].kind == TokKind::Keyword
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
    let stmt: Vec<Tok> = statement_tokens(toks, sql.len(), caret).to_vec();

    let (blo, bhi) = branch_bounds(&stmt, lo, hi, caret);
    let branch: Vec<Tok> = stmt
        .iter()
        .filter(|t| t.span.start >= blo && t.span.end <= bhi)
        .cloned()
        .collect();
    let branch_scopes = scopes(&branch);

    let mut aliases = aliases_of(&branch);
    let (derived, derived_binds) = derived_tables(&branch);
    aliases.extend(derived_binds);
    let in_scope: Vec<String> = aliases.iter().map(|(_, t)| t.clone()).collect();
    let select_aliases = column_aliases(&branch);
    let ctes = ctes_of(&stmt);

    let partial_tok = stmt.iter().find(|t| {
        t.span.end == caret
            && matches!(
                t.kind,
                TokKind::Ident | TokKind::Keyword | TokKind::QuotedIdent
            )
    });
    let (mut partial, mut replace) = match partial_tok {
        Some(t) => (t.text.clone(), t.span.clone()),
        None => (String::new(), caret..caret),
    };

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

    let caret_scope = branch
        .iter()
        .filter(|t| t.span.end <= replace.start)
        .fold(0i32, |d, t| match t.kind {
            TokKind::Punct if t.text == "(" => d + 1,
            TokKind::Punct if t.text == ")" => d - 1,
            _ => d,
        });
    let gov_idx = governing_clause(&branch, &branch_scopes, replace.start, caret_scope);
    let governing = gov_idx
        .map(|i| clause_of(&branch[i].text))
        .unwrap_or(Clause::Unknown);
    let governing = refine_statement_clause(&branch, governing);

    let column_list = match governing {
        Clause::Insert => insert_column_list(&stmt, caret),
        Clause::Copy => copy_partition_list(&stmt, caret),
        _ => None,
    };

    let clause_refs = gov_idx
        .map(|i| {
            let region = clause_region(&branch, &branch_scopes, i);
            refs_in(&branch, &branch_scopes, region, branch_scopes[i])
        })
        .unwrap_or_default();
    let projection = select_refs(&branch, &branch_scopes, replace.start, caret_scope);

    let comparand = prev
        .filter(|t| {
            t.kind == TokKind::Op
                && matches!(t.text.as_str(), "=" | "<" | ">" | "<=" | ">=" | "<>" | "!=")
        })
        .and_then(|_| {
            let n = before.len();
            let operand = before.get(n.wrapping_sub(2)).copied()?;
            if !is_name_like(operand) {
                return None;
            }
            let dotted = before
                .get(n.wrapping_sub(3))
                .copied()
                .filter(|d| d.kind == TokKind::Punct && d.text == ".");
            let qualifier = dotted
                .and_then(|_| before.get(n.wrapping_sub(4)).copied())
                .filter(|q| is_name_like(q))
                .map(|q| q.text.clone());
            Some((qualifier, operand.text.clone()))
        });

    let mut set_key: Option<String> = None;
    let prev_as = prev.is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("AS"));
    let context = if prev.is_none() {
        Context::At(Clause::Start, Role::Operand)
    } else if governing == Clause::SetOption {
        match before
            .iter()
            .position(|t| t.kind == TokKind::Op && t.text == "=")
        {
            None => {
                let mut i = before.len();
                while i >= 2
                    && before[i - 1].kind == TokKind::Punct
                    && before[i - 1].text == "."
                    && is_name_like(before[i - 2])
                {
                    i -= 2;
                }
                if i < before.len() {
                    let chain: String = before[i..].iter().map(|t| t.text.as_str()).collect();
                    partial = format!("{chain}{partial}");
                    replace.start = before[i].span.start;
                }
                if i == 1 {
                    Context::At(Clause::SetOption, Role::Operand)
                } else {
                    Context::At(Clause::SetOption, Role::Continuation)
                }
            }
            Some(0) => Context::At(Clause::SetOption, Role::Continuation),
            Some(eq) => {
                set_key = Some(before[1..eq].iter().map(|t| t.text.as_str()).collect());
                if item_complete(prev, prev2) {
                    Context::At(Clause::SetOption, Role::Continuation)
                } else {
                    Context::At(Clause::SetOption, Role::Operand)
                }
            }
        }
    } else if prev
        .map(|t| t.kind == TokKind::Punct && t.text == ".")
        .unwrap_or(false)
    {
        Context::Dot(dot_chain(&before, &aliases))
    } else if restarts_ladder(prev, prev2, prev_as, governing) {
        Context::At(Clause::Restart, Role::Operand)
    } else if prev_as
        && governing == Clause::CreateExternal
        && prev2.is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("STORED"))
    {
        Context::At(Clause::CreateExternal, Role::Operand)
    } else if prev_as
        || prev.is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("SHOW"))
        || prev2.is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("SHOW"))
    {
        Context::At(governing, Role::Binding)
    } else {
        Context::At(
            governing,
            role_at(governing, prev, prev2, &before, column_list.is_some()),
        )
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
        set_key,
        column_list,
        derived,
    }
}

/// The qualifier chain the caret sits behind — the name segments before the trailing `.`,
/// outermost first. `before` ends at that dot.
///
/// Absorbed backwards through `name . name .`, the shape the `SET` dotted-key rule reads for its
/// own key chain, and for the same reason: a qualified name is one address, and reading only its
/// last segment makes `pg.public.` indistinguishable from a relation called `public`. The
/// [`replace`](CaretAnalysis::replace) span is deliberately **not** widened with it — an accept
/// replaces the word being typed after the dot, never the qualifier that led to it.
///
/// A single segment is alias-resolved (`FROM events e` → `e.` is `events`); a longer chain is
/// not, because an alias binds one name and a catalog-qualified address has none.
fn dot_chain(before: &[&Tok], aliases: &[(String, String)]) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut i = before.len();
    while i >= 2
        && before[i - 1].kind == TokKind::Punct
        && before[i - 1].text == "."
        && is_name_like(before[i - 2])
    {
        parts.push(before[i - 2].text.clone());
        i -= 2;
    }
    parts.reverse();
    match parts.as_slice() {
        [owner] => vec![aliases
            .iter()
            .find(|(alias, _)| alias.eq_ignore_ascii_case(owner))
            .map_or_else(|| owner.clone(), |(_, relation)| relation.clone())],
        _ => parts,
    }
}

/// Whether the caret sits where a **fresh query begins**, so the clause ladder restarts.
///
/// Three positions, all of them an operand at [`Clause::Restart`]: a derived table or a `COPY`'s
/// query source (`FROM ( |`, `COPY ( |` — and only that paren, since a later group such as
/// `PARTITIONED BY (` is the statement's own and takes [`role_at`]'s Copy arm); a set operation or
/// `EXPLAIN [ANALYZE]`; and the `AS` that opens the query body of CTAS, `CREATE [OR REPLACE] VIEW`
/// or `PREPARE`, parenthesized or not — a parenthesized body must not read as a column-definition
/// Binding.
fn restarts_ladder(
    prev: Option<&Tok>,
    prev2: Option<&Tok>,
    prev_as: bool,
    governing: Clause,
) -> bool {
    let open_paren = |t: &Tok| t.kind == TokKind::Punct && t.text == "(";
    let keyword = |t: &Tok, word: &str| t.kind == TokKind::Keyword && t.eq_ci(word);

    let query_source = prev.is_some_and(open_paren)
        && (governing == Clause::From
            || (governing == Clause::Copy && prev2.is_some_and(|t| keyword(t, "COPY"))));

    let set_operation = prev.is_some_and(|t| {
        t.kind == TokKind::Keyword
            && (SET_OP_WORDS.iter().any(|w| t.eq_ci(w))
                || (t.eq_ci("ALL") && prev2.is_some_and(|p| p.eq_ci("UNION")))
                || t.eq_ci("EXPLAIN")
                || (t.eq_ci("ANALYZE") && prev2.is_some_and(|p| p.eq_ci("EXPLAIN"))))
    });

    let query_body = (prev_as
        || (prev.is_some_and(open_paren) && prev2.is_some_and(|t| keyword(t, "AS"))))
        && matches!(
            governing,
            Clause::CreateTable | Clause::CreateView | Clause::Prepare
        );

    query_source || set_operation || query_body
}

/// The target table of the `INSERT INTO t (…)` **column list** the caret sits inside —
/// the paren group directly after the target's name, before any `VALUES`. `None`
/// anywhere else, VALUES tuples included: the column list names existing columns of
/// the target, a tuple's content is the user's own data. A dotted target answers its
/// last segment, which is the name the single-namespace catalog resolves.
pub(crate) fn insert_column_list(stmt: &[Tok], caret: usize) -> Option<ColumnList> {
    if !stmt
        .first()
        .is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("INSERT"))
    {
        return None;
    }
    let into = stmt
        .iter()
        .position(|t| t.kind == TokKind::Keyword && t.eq_ci("INTO"))?;
    let mut i = into + 1;
    if !stmt.get(i).is_some_and(is_name_like) {
        return None;
    }
    while stmt.get(i + 1).is_some_and(|t| t.text == ".")
        && stmt.get(i + 2).is_some_and(is_name_like)
    {
        i += 2;
    }
    let target = stmt[i].text.clone();
    let open = i + 1;
    if !stmt
        .get(open)
        .is_some_and(|t| t.kind == TokKind::Punct && t.text == "(")
    {
        return None;
    }
    if caret <= stmt[open].span.start {
        return None;
    }
    if let Some(close) = matching_paren(stmt, open) {
        if caret > stmt[close].span.start {
            return None;
        }
    }
    Some(ColumnList {
        source: ListSource::Table(target),
        listed: group_names(stmt, open),
    })
}

/// A statement position whose operand is a **column of one known relation** — an
/// INSERT's column list, a COPY's `PARTITIONED BY` group. The same shape as a `Dot`
/// position, and the pool resolves it the same way: that relation's columns and
/// nothing else, empty when the relation cannot be resolved (precision over noise).
pub struct ColumnList {
    pub source: ListSource,
    /// Names already written in the group — the position's own written-demotion
    /// region, exactly as a clause region's refs are (rank only, never filter).
    pub listed: Vec<String>,
}

/// Where the list's columns come from.
pub enum ListSource {
    /// A named relation, resolved against the catalog by the caller.
    Table(String),
    /// `COPY (SELECT …) …` — the query source's scraped projection, best-effort
    /// exactly as a CTE body's.
    Projection(Vec<String>),
}

/// The name-like entries of the paren group at `open` — the already-listed names.
fn group_names(stmt: &[Tok], open: usize) -> Vec<String> {
    let close = matching_paren(stmt, open).unwrap_or(stmt.len());
    stmt[open + 1..close.min(stmt.len())]
        .iter()
        .filter(|t| is_name_like(t))
        .map(|t| t.text.clone())
        .collect()
}

/// The `PARTITIONED BY (…)` list of the `COPY` statement the caret sits inside —
/// its entries name existing columns of the statement's source. `None` for every
/// other position, `OPTIONS (…)` included (DataFusion's open key namespace is the
/// user's own content, not ours to offer).
pub(crate) fn copy_partition_list(stmt: &[Tok], caret: usize) -> Option<ColumnList> {
    if !stmt
        .first()
        .is_some_and(|t| t.kind == TokKind::Keyword && t.eq_ci("COPY"))
    {
        return None;
    }
    let mut stack: Vec<usize> = Vec::new();
    for (i, t) in stmt.iter().enumerate() {
        if t.span.start >= caret {
            break;
        }
        if t.kind == TokKind::Punct && t.text == "(" {
            stack.push(i);
        }
        if t.kind == TokKind::Punct && t.text == ")" {
            stack.pop();
        }
    }
    let open = *stack.last()?;
    if !(open >= 2 && stmt[open - 1].eq_ci("BY") && stmt[open - 2].eq_ci("PARTITIONED")) {
        return None;
    }
    let source = stmt.get(1)?;
    let source = if source.kind == TokKind::Punct && source.text == "(" {
        let close = matching_paren(stmt, 1).unwrap_or(stmt.len());
        ListSource::Projection(projection_columns(&stmt[2..close.min(stmt.len())]))
    } else if is_name_like(source) {
        let mut i = 1;
        while stmt.get(i + 1).is_some_and(|t| t.text == ".")
            && stmt.get(i + 2).is_some_and(is_name_like)
        {
            i += 2;
        }
        ListSource::Table(stmt[i].text.clone())
    } else {
        return None;
    };
    Some(ColumnList {
        source,
        listed: group_names(stmt, open),
    })
}

/// The declared argument names of the `CREATE FUNCTION` statement under the caret —
/// the identifiers of the first paren group after the `FUNCTION` keyword, each the
/// first name of its comma-separated item. The `At(CreateFunction, Operand)` pool
/// offers these because the body may reference **only** its arguments
/// (`statements/arms/functions.rs`) — catalog columns and relations there would offer exactly
/// what `Definition::check` refuses.
pub(crate) fn function_arguments(toks: &[Tok], sql_len: usize, caret: usize) -> Vec<String> {
    let stmt = statement_tokens(toks, sql_len, caret);
    let Some(f) = stmt
        .iter()
        .position(|t| t.kind == TokKind::Keyword && t.eq_ci("FUNCTION"))
    else {
        return Vec::new();
    };
    let Some(open) =
        (f + 1..stmt.len()).find(|&i| stmt[i].kind == TokKind::Punct && stmt[i].text == "(")
    else {
        return Vec::new();
    };
    let close = matching_paren(stmt, open).unwrap_or(stmt.len());
    let mut out = Vec::new();
    let mut depth = 0i32;
    for i in open..close.min(stmt.len()) {
        let t = &stmt[i];
        if t.kind == TokKind::Punct && t.text == "(" {
            depth += 1;
            continue;
        }
        if t.kind == TokKind::Punct && t.text == ")" {
            depth -= 1;
            continue;
        }
        if depth != 1 {
            continue;
        }
        let starts_item = {
            let p = &stmt[i - 1];
            p.kind == TokKind::Punct && (p.text == "(" || p.text == ",")
        };
        if starts_item && is_name_like(t) {
            out.push(t.text.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::lex::lex;

    /// Analyse with the caret at the `|` marker.
    fn at(sql_with_caret: &str) -> CaretAnalysis {
        let caret = sql_with_caret.find('|').expect("caret marker");
        let sql = sql_with_caret.replace('|', "");
        let (toks, _) = lex(&sql, "generic");
        analyze_caret(&sql, caret, &toks)
    }

    #[test]
    fn statement_start_and_select_list() {
        assert_eq!(at("|").context, Context::At(Clause::Start, Role::Operand));
        assert_eq!(
            at("SELECT |").context,
            Context::At(Clause::Select, Role::Operand)
        );
        assert_eq!(
            at("SELECT a, | FROM t").context,
            Context::At(Clause::Select, Role::Operand)
        );
    }

    #[test]
    fn from_target_vs_from_continuation() {
        assert_eq!(
            at("SELECT * FROM |").context,
            Context::At(Clause::From, Role::Operand)
        );
        assert_eq!(
            at("SELECT * FROM eve|").context,
            Context::At(Clause::From, Role::Operand)
        );
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
        assert_eq!(
            at("SELECT * FROM t ORDER BY x ASC |").context,
            Context::At(Clause::OrderBy, Role::Continuation)
        );
    }

    #[test]
    fn multiplication_star_is_an_operand_position() {
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
        assert_eq!(
            at("SELECT * FROM (|").context,
            Context::At(Clause::Restart, Role::Operand)
        );
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
        assert_eq!(ca.context, Context::Dot(vec!["events".into()]));
        let ca = at("SELECT events.| FROM events");
        assert_eq!(ca.context, Context::Dot(vec!["events".into()]));
        let ca = at("SELECT x.| FROM events o");
        assert_eq!(ca.context, Context::Dot(vec!["x".into()]));
    }

    #[test]
    fn a_qualified_dot_keeps_every_segment() {
        let ca = at("SELECT * FROM pg.|");
        assert_eq!(ca.context, Context::Dot(vec!["pg".into()]));
        let ca = at("SELECT * FROM pg.public.|");
        assert_eq!(ca.context, Context::Dot(vec!["pg".into(), "public".into()]));
        let ca = at("SELECT pg.public.orders.| FROM pg.public.orders");
        assert_eq!(
            ca.context,
            Context::Dot(vec!["pg".into(), "public".into(), "orders".into()]),
        );
        assert_eq!(ca.replace, 24..24, "the qualifier is read, never replaced");
    }

    #[test]
    fn partial_and_replace_span() {
        let ca = at("SELECT sta| FROM t");
        assert_eq!(ca.partial, "sta");
        assert_eq!(ca.replace, 7..10);
        let ca = at("SELECT st|a FROM t");
        assert_eq!(ca.partial, "");
        assert_eq!(ca.replace, 9..9);
    }

    #[test]
    fn multi_statement_bounds() {
        let ca = at("SELECT a FROM t1; SELECT b FROM t2 WHERE |");
        assert_eq!(ca.context, Context::At(Clause::Where, Role::Operand));
        assert_eq!(ca.in_scope, vec!["t2".to_string()]);
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
        assert_eq!(
            ca.select_aliases,
            vec!["total".to_string(), "mean".to_string()]
        );
    }

    #[test]
    fn projection_refs_are_source_columns_only() {
        let ca = at("SELECT name, u.tags, sum(x) AS spend FROM |");
        assert_eq!(ca.projection, vec!["name".to_string(), "tags".to_string()]);
        let ca = at("WITH r AS (SELECT amount FROM events) SELECT total FROM |");
        assert_eq!(ca.projection, vec!["total".to_string()]);
        assert!(at("SELECT * FROM |").projection.is_empty());
    }

    #[test]
    fn cte_names_and_bare_projection() {
        let ca = at("WITH recent AS (SELECT amount, status FROM events) SELECT | FROM recent");
        assert_eq!(ca.ctes.len(), 1);
        assert_eq!(ca.ctes[0].name, "recent");
        assert_eq!(
            ca.ctes[0].columns,
            vec!["amount".to_string(), "status".to_string()]
        );
    }

    #[test]
    fn cte_as_aliases_and_qualified_refs() {
        let ca = at("WITH r AS (SELECT sum(x) AS spend, t.name FROM t) SELECT | FROM r");
        assert_eq!(
            ca.ctes[0].columns,
            vec!["spend".to_string(), "name".to_string()]
        );
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
        let ca = at("WITH r AS (SELECT x FROM t SELECT |");
        assert_eq!(ca.ctes[0].name, "r");
    }

    #[test]
    fn cte_resolvable_via_helper() {
        let ca = at("WITH Recent AS (SELECT x FROM t) SELECT | FROM Recent");
        assert!(ca.cte("recent").is_some(), "case-insensitive lookup");
    }

    #[test]
    fn statement_heads_refine_and_role() {
        for (sql, clause, role) in [
            ("CREATE |", Clause::Create, Role::Continuation),
            ("CREATE OR REPLACE |", Clause::Create, Role::Continuation),
            ("CREATE TABLE |", Clause::CreateTable, Role::Binding),
            ("CREATE TABLE t |", Clause::CreateTable, Role::Continuation),
            ("CREATE TABLE t (|", Clause::CreateTable, Role::Binding),
            (
                "CREATE OR REPLACE VIEW |",
                Clause::CreateView,
                Role::Binding,
            ),
            (
                "CREATE EXTERNAL TABLE |",
                Clause::CreateExternal,
                Role::Binding,
            ),
            (
                "CREATE EXTERNAL TABLE t |",
                Clause::CreateExternal,
                Role::Continuation,
            ),
            (
                "CREATE EXTERNAL TABLE t STORED AS |",
                Clause::CreateExternal,
                Role::Operand,
            ),
            ("DROP |", Clause::Drop, Role::Continuation),
            ("DROP TABLE |", Clause::DropTable, Role::Operand),
            ("DROP TABLE IF EXISTS |", Clause::DropTable, Role::Operand),
            ("DROP TABLE a, |", Clause::DropTable, Role::Operand),
            ("DROP TABLE t |", Clause::DropTable, Role::Continuation),
            ("DROP VIEW |", Clause::DropView, Role::Operand),
            ("DROP FUNCTION |", Clause::DropFunction, Role::Operand),
            ("INSERT INTO |", Clause::Insert, Role::Operand),
            ("INSERT INTO t |", Clause::Insert, Role::Continuation),
            ("INSERT INTO t (|", Clause::Insert, Role::Operand),
            ("INSERT INTO t (a, |", Clause::Insert, Role::Operand),
            ("INSERT INTO t VALUES (1, |", Clause::Insert, Role::Binding),
            ("INSERT INTO t (a) VALUES (|", Clause::Insert, Role::Binding),
            ("COPY |", Clause::Copy, Role::Operand),
            ("COPY t |", Clause::Copy, Role::Continuation),
            (
                "COPY t TO 'x' PARTITIONED BY (|",
                Clause::Copy,
                Role::Operand,
            ),
            (
                "COPY t TO 'x' PARTITIONED BY (a, |",
                Clause::Copy,
                Role::Operand,
            ),
            ("COPY t TO 'x' OPTIONS (|", Clause::Copy, Role::Binding),
            ("PREPARE |", Clause::Prepare, Role::Binding),
            ("PREPARE p |", Clause::Prepare, Role::Continuation),
        ] {
            assert_eq!(at(sql).context, Context::At(clause, role), "{sql}");
        }
    }

    #[test]
    fn statement_as_positions_restart_the_ladder() {
        for sql in [
            "CREATE TABLE t AS |",
            "CREATE OR REPLACE VIEW v AS |",
            "PREPARE p AS |",
            "PREPARE p(INT) AS |",
            "COPY (|",
            "CREATE TABLE t AS (|",
            "CREATE OR REPLACE VIEW v AS (|",
        ] {
            assert_eq!(
                at(sql).context,
                Context::At(Clause::Restart, Role::Operand),
                "{sql}"
            );
        }
        assert_eq!(
            at("SELECT amount AS |").context,
            Context::At(Clause::Select, Role::Binding)
        );
        assert_eq!(
            at("SELECT * FROM t AS |").context,
            Context::At(Clause::From, Role::Binding)
        );
        assert_eq!(
            at("COPY t TO 'x' STORED AS |").context,
            Context::At(Clause::Copy, Role::Binding)
        );
    }

    #[test]
    fn create_function_body_roles() {
        for (sql, role) in [
            ("CREATE FUNCTION |", Role::Binding),
            ("CREATE FUNCTION f(|", Role::Binding),
            ("CREATE FUNCTION f(price DOUBLE, |", Role::Binding),
            ("CREATE FUNCTION f(x BIGINT) |", Role::Continuation),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN |",
                Role::Operand,
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN x |",
                Role::Continuation,
            ),
            (
                "CREATE FUNCTION f(x BIGINT) RETURNS BIGINT RETURN x * |",
                Role::Operand,
            ),
        ] {
            assert_eq!(
                at(sql).context,
                Context::At(Clause::CreateFunction, role),
                "{sql}"
            );
        }
    }

    #[test]
    fn set_key_positions_absorb_the_dotted_chain() {
        let ca = at("SET |");
        assert_eq!(ca.context, Context::At(Clause::SetOption, Role::Operand));
        assert_eq!(ca.set_key, None);
        let ca = at("SET dat|");
        assert_eq!(ca.context, Context::At(Clause::SetOption, Role::Operand));
        assert_eq!((ca.partial.as_str(), ca.replace.clone()), ("dat", 4..7));
        let ca = at("SET datafusion.|");
        assert_eq!(ca.partial, "datafusion.");
        assert_eq!(ca.replace, 4..15);
        let ca = at("SET datafusion.execution.b|");
        assert_eq!(ca.partial, "datafusion.execution.b");
        assert_eq!(ca.replace, 4..26);
        let ca = at("SET datafusion.execution.batch_size |");
        assert_eq!(
            ca.context,
            Context::At(Clause::SetOption, Role::Continuation)
        );
        let ca = at("SET datafusion.execution.batch_size = |");
        assert_eq!(ca.context, Context::At(Clause::SetOption, Role::Operand));
        assert_eq!(
            ca.set_key.as_deref(),
            Some("datafusion.execution.batch_size")
        );
        let ca = at("SET datafusion.execution.batch_size = 1024 |");
        assert_eq!(
            ca.context,
            Context::At(Clause::SetOption, Role::Continuation)
        );
        assert_eq!(
            at("RESET datafusion.|").context,
            Context::At(Clause::SetOption, Role::Operand)
        );
    }

    #[test]
    fn a_mid_edit_equals_before_the_set_lead_does_not_panic() {
        let ca = at("= 1 UNION SET |");
        assert_eq!(
            ca.context,
            Context::At(Clause::SetOption, Role::Continuation)
        );
        assert_eq!(ca.set_key, None);
        let _ = at("= 1 EXCEPT RESET |");
    }

    #[test]
    fn a_column_named_values_stays_in_the_column_list() {
        assert_eq!(
            at("INSERT INTO t (values, |").context,
            Context::At(Clause::Insert, Role::Operand)
        );
    }

    #[test]
    fn deallocate_prepare_keeps_the_execute_clause() {
        assert_eq!(
            at("DEALLOCATE PREPARE |").context,
            Context::At(Clause::Execute, Role::Operand)
        );
    }

    #[test]
    fn insert_column_list_and_copy_partition_lists_resolve() {
        let target = |sql: &str| {
            let caret = sql.find('|').expect("caret marker");
            let sql = sql.replace('|', "");
            let (toks, _) = lex(&sql, "generic");
            insert_column_list(&toks, caret).map(|l| {
                let ListSource::Table(name) = l.source else {
                    panic!("an INSERT list's source is its target table");
                };
                (name, l.listed)
            })
        };
        assert_eq!(target("INSERT INTO t (|"), Some(("t".into(), vec![])));
        assert_eq!(
            target("INSERT INTO t (a, |"),
            Some(("t".into(), vec!["a".into()]))
        );
        assert_eq!(target("INSERT INTO s.t (|"), Some(("t".into(), vec![])));
        assert_eq!(target("INSERT INTO t |"), None);
        assert_eq!(target("INSERT INTO t (a) VALUES (|"), None);
        assert_eq!(target("INSERT INTO t (a) |"), None);

        let list = |sql: &str| {
            let caret = sql.find('|').expect("caret marker");
            let sql = sql.replace('|', "");
            let (toks, _) = lex(&sql, "generic");
            copy_partition_list(&toks, caret)
        };
        match list("COPY events TO 'x' PARTITIONED BY (year, |") {
            Some(ColumnList {
                source: ListSource::Table(name),
                listed,
            }) => {
                assert_eq!(name, "events");
                assert_eq!(listed, ["year"]);
            }
            other => panic!("{:?}", other.is_some()),
        }
        match list("COPY s.events TO 'x' PARTITIONED BY (|") {
            Some(ColumnList {
                source: ListSource::Table(name),
                ..
            }) => assert_eq!(name, "events"),
            other => panic!("{:?}", other.is_some()),
        }
        match list("COPY (SELECT user_id, amount FROM events) TO 'x' PARTITIONED BY (|") {
            Some(ColumnList {
                source: ListSource::Projection(cols),
                ..
            }) => assert_eq!(cols, ["user_id", "amount"]),
            other => panic!("{:?}", other.is_some()),
        }
        assert!(list("COPY t TO 'x' OPTIONS (|").is_none());
        assert!(list("COPY t TO 'x' PARTITIONED BY (a) OPTIONS (|").is_none());
    }

    #[test]
    fn function_arguments_scraped_from_the_first_paren_group() {
        let args = |sql: &str| {
            let (toks, _) = lex(sql, "generic");
            function_arguments(&toks, sql.len(), sql.len())
        };
        assert_eq!(
            args("CREATE FUNCTION f(price DOUBLE, qty BIGINT) RETURNS DOUBLE RETURN "),
            ["price", "qty"]
        );
        assert_eq!(
            args("CREATE FUNCTION f(price DOUBLE, qty"),
            ["price", "qty"]
        );
        assert_eq!(
            args("CREATE FUNCTION f(price DECIMAL(10, 2)) RETURNS DOUBLE RETURN "),
            ["price"]
        );
        assert!(args("SELECT 1").is_empty());
    }
}
