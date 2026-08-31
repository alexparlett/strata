//! The native **name resolver** — every unknown table/column in a parsed
//! statement, not just the first.
//!
//! The DataFusion dry-plan behind [`super::validate`] is engine-authoritative but
//! fail-fast: it stops at the first unresolved name, and it resolves mid-edit drafts
//! (`SELECT draft_col`, with no FROM) against an empty schema. This module walks
//! the **sqlparser AST** of a statement that parsed, resolves every table and column
//! reference against the live session (catalog + CTEs + aliases + derived tables),
//! and reports **all** unknown names with byte spans. The dry-plan stays behind it as
//! the authority for types/casts/arity/ambiguity, where fail-fast is acceptable
//! because name faults were already caught here.
//!
//! The stance throughout is *report only what is provably wrong*: a column is flagged
//! only when every relation in its scope chain is fully known and the name matches
//! nothing (columns, select aliases where legal, outer scopes). Any unknowable
//! element — a FROM-less draft scope, a table function, an underivable projection —
//! makes the walk go quiet there instead ([`Resolution::complete`] turns false, and
//! the caller suppresses the planner's premature field errors in exchange). sqlparser
//! has no visitor pruning, so the walk is hand-rolled at the query/select level; the
//! ~80-variant [`Expr`] enum is bounded by a catch-all that flat-checks identifiers
//! and skips any subtree carrying a subquery (mis-scoping risk — the planner still
//! covers those fail-fast).

use std::collections::HashMap;
use std::iter;
use std::ops::{ControlFlow, Range};

use datafusion::prelude::SessionContext;
use datafusion::sql::parser::Statement as DFStatement;
use datafusion::sql::planner::object_name_to_table_reference;
use datafusion::sql::sqlparser::ast::{
    visit_expressions, visit_relations, AccessExpr, Cte, Expr, Function, FunctionArg,
    FunctionArgExpr, FunctionArguments, GroupByExpr, Ident, JoinConstraint, JoinOperator,
    ObjectName, OrderBy, OrderByExpr, OrderByKind, Query, Select, SelectItem, SetExpr, Spanned,
    Statement as SqlStatement, Subscript, TableAlias, TableFactor, WindowType,
};
use datafusion::sql::sqlparser::tokenizer::Span;

use crate::sql::fuzzy;
use crate::sql::lex::byte_span;
use crate::sql::oracle::{Columns as OracleColumns, NameOracle};
use crate::sql::spans::diag;
use strata_model::{Diagnostic, Severity};

/// The outcome of resolving one statement's names.
pub(crate) struct Resolution {
    /// Every unknown table/column reference, byte-spanned into the full buffer.
    pub diags: Vec<Diagnostic>,
    /// True iff every scope the walk touched had fully-known relations and columns.
    /// When false, a planner `FieldNotFound` is mid-edit noise, not truth — the
    /// caller suppresses it (the generalized no-FROM grace).
    pub complete: bool,
}

impl Resolution {
    /// Nothing to report, planner is the authority (non-query statements).
    fn clean() -> Self {
        Resolution {
            diags: Vec::new(),
            complete: true,
        }
    }
}

/// Relation key → what the [`NameOracle`] knows, prefetched before the sync walk.
type SchemaMap = HashMap<String, OracleColumns>;

/// Resolve all table/column names in `stmt` (the statement at `stmt_start` whose
/// text is `slice`) against the live session. Read-only; never plans.
pub(crate) async fn resolve(
    names: &NameOracle<'_>,
    ctx: &SessionContext,
    stmt: &DFStatement,
    slice: &str,
    stmt_start: usize,
    sql: &str,
) -> Resolution {
    let Some(inner) = unwrap_statement(stmt) else {
        return Resolution::clean();
    };
    let normalize = ctx
        .state()
        .config_options()
        .sql_parser
        .enable_ident_normalization;
    let schemas = prefetch(names, inner, normalize).await;
    resolve_statement(inner, &schemas, normalize, slice, stmt_start, sql)
}

/// The sqlparser statement inside a DataFusion statement, with `EXPLAIN` layers
/// unwrapped; `None` for DataFusion extensions (policy handles those).
///
/// Shared with the statement layer's `read_policy`, which asks the same question of
/// the same two wrappers: DataFusion spells `EXPLAIN` twice (its own extension statement and
/// sqlparser's), and a consumer that unwrapped only one would answer differently about
/// `EXPLAIN EXECUTE p` depending on which parser arm produced it.
pub(crate) fn unwrap_statement(stmt: &DFStatement) -> Option<&SqlStatement> {
    match stmt {
        DFStatement::Statement(s) => {
            let mut s: &SqlStatement = s;
            while let SqlStatement::Explain { statement, .. } = s {
                s = statement;
            }
            Some(s)
        }
        DFStatement::Explain(e) => unwrap_statement(&e.statement),
        _ => None,
    }
}

/// Fetch what the session knows about every relation the statement references, through the one
/// [`NameOracle`] every rung asks. CTE names get looked up too and simply miss — they are
/// shadowed at walk time.
async fn prefetch(oracle: &NameOracle<'_>, stmt: &SqlStatement, normalize: bool) -> SchemaMap {
    let mut names: Vec<ObjectName> = Vec::new();
    let _ = visit_relations(stmt, |name: &ObjectName| {
        names.push(name.clone());
        ControlFlow::<()>::Continue(())
    });
    let mut map = SchemaMap::new();
    for name in names {
        let Ok(table_ref) = object_name_to_table_reference(name, normalize) else {
            continue;
        };
        let key = table_ref.to_string();
        if map.contains_key(&key) {
            continue;
        }
        map.insert(key, oracle.columns(table_ref).await);
    }
    map
}

/// Sync entry point over a prefetched [`SchemaMap`] (unit-testable without a session).
fn resolve_statement(
    stmt: &SqlStatement,
    schemas: &SchemaMap,
    normalize: bool,
    slice: &str,
    stmt_start: usize,
    sql: &str,
) -> Resolution {
    let SqlStatement::Query(query) = stmt else {
        return Resolution::clean();
    };
    let mut r = Resolver {
        schemas,
        normalize,
        slice,
        stmt_start,
        sql,
        ctes: Vec::new(),
        diags: Vec::new(),
        complete: true,
    };
    r.query(query, None);
    Resolution {
        diags: r.diags,
        complete: r.complete,
    }
}

/// What is known about a relation's output columns.
#[derive(Clone)]
enum Cols {
    /// Every column name (as written in the schema; matched case-insensitively).
    Known(Vec<String>),
    /// Underivable (table function, opaque provider, complex projection) — any
    /// scope containing one goes quiet rather than guess.
    Unknown,
}

impl Cols {
    fn contains(&self, name: &str) -> bool {
        matches!(self, Cols::Known(cols) if cols.iter().any(|c| c.eq_ignore_ascii_case(name)))
    }
}

/// One relation bound into a scope.
#[derive(Clone)]
struct Rel {
    /// The name a qualifier resolves against: the alias, else the last name part.
    /// `None` for unnamed derived tables (unaddressable, columns still in scope).
    binding: Option<String>,
    cols: Cols,
}

/// One query scope, chained to its parent for correlated subqueries.
struct Scope<'p> {
    parent: Option<&'p Scope<'p>>,
    relations: Vec<Rel>,
    /// `SELECT expr AS name` aliases — legal reference targets in the
    /// post-projection clauses (GROUP BY / HAVING / QUALIFY / ORDER BY).
    aliases: Vec<String>,
}

impl<'p> Scope<'p> {
    fn chain(&self) -> impl Iterator<Item = &Scope<'p>> {
        iter::successors(Some(self), |s| s.parent)
    }
    /// No relations anywhere in the chain — a pure draft; nothing to check against.
    fn chain_is_empty(&self) -> bool {
        self.chain().all(|s| s.relations.is_empty())
    }
    /// Any relation in the chain with unknowable columns — proof is impossible.
    fn chain_has_unknown(&self) -> bool {
        self.chain()
            .any(|s| s.relations.iter().any(|r| matches!(r.cols, Cols::Unknown)))
    }
    fn chain_has_column(&self, name: &str) -> bool {
        self.chain()
            .any(|s| s.relations.iter().any(|r| r.cols.contains(name)))
    }
    fn chain_binding(&self, name: &str) -> Option<&Rel> {
        self.chain().find_map(|s| {
            s.relations.iter().find(|r| {
                r.binding
                    .as_deref()
                    .is_some_and(|b| b.eq_ignore_ascii_case(name))
            })
        })
    }
}

struct Resolver<'a> {
    schemas: &'a SchemaMap,
    normalize: bool,
    slice: &'a str,
    stmt_start: usize,
    sql: &'a str,
    /// CTEs in scope, innermost last (a stack: pushed per WITH, truncated on exit).
    ctes: Vec<(String, Cols)>,
    diags: Vec<Diagnostic>,
    complete: bool,
}

impl Resolver<'_> {
    /// Walk one query; returns its output columns (for CTE/derived-table binding).
    fn query(&mut self, q: &Query, outer: Option<&Scope>) -> Cols {
        if !q.pipe_operators.is_empty() {
            self.complete = false;
            return Cols::Unknown;
        }
        let cte_mark = self.ctes.len();
        if let Some(with) = &q.with {
            for cte in &with.cte_tables {
                self.cte(cte, with.recursive, outer);
            }
        }
        let cols = self.set_expr(&q.body, q.order_by.as_ref(), outer);
        self.ctes.truncate(cte_mark);
        cols
    }

    /// Resolve a CTE body and push its name + output columns onto the CTE stack.
    fn cte(&mut self, cte: &Cte, recursive: bool, outer: Option<&Scope>) {
        let name = cte.alias.name.value.clone();
        if recursive {
            self.ctes.push((name.clone(), Cols::Unknown));
        }
        let derived = self.query(&cte.query, outer);
        let cols = alias_cols(&cte.alias).unwrap_or(derived);
        if recursive {
            self.ctes.truncate(self.ctes.len() - 1);
        }
        self.ctes.push((name, cols));
    }

    /// Walk a query body. `order_by` is the owning query's ORDER BY — checked only
    /// against a plain-select body (each set-op branch is its own scope, and an
    /// ORDER BY over the combined result is left to the planner).
    fn set_expr(
        &mut self,
        body: &SetExpr,
        order_by: Option<&OrderBy>,
        outer: Option<&Scope>,
    ) -> Cols {
        match body {
            SetExpr::Select(s) => self.select(s, order_by, outer),
            SetExpr::Query(q) => self.query(q, outer),
            SetExpr::SetOperation { left, right, .. } => {
                let cols = self.set_expr(left, None, outer);
                self.set_expr(right, None, outer);
                cols
            }
            _ => Cols::Unknown,
        }
    }

    /// Walk one SELECT; returns its projection as output columns.
    fn select(&mut self, s: &Select, order_by: Option<&OrderBy>, outer: Option<&Scope>) -> Cols {
        let mut relations: Vec<Rel> = Vec::new();
        for twj in &s.from {
            self.table_factor(&twj.relation, &mut relations, outer);
            for join in &twj.joins {
                self.table_factor(&join.relation, &mut relations, outer);
            }
        }
        if relations.iter().any(|r| matches!(r.cols, Cols::Unknown)) {
            self.complete = false;
        }

        let mut scope = Scope {
            parent: outer,
            relations,
            aliases: Vec::new(),
        };
        let checkable = !scope.chain_is_empty();
        if s.from.is_empty() && !checkable {
            self.complete = false;
        }

        for twj in &s.from {
            for join in &twj.joins {
                if let Some(constraint) = join_constraint(&join.join_operator) {
                    match constraint {
                        JoinConstraint::On(e) => self.expr(e, &scope, false, checkable),
                        JoinConstraint::Using(names) => {
                            for name in names {
                                if let [part] = name.0.as_slice() {
                                    if let Some(id) = part.as_ident() {
                                        self.check_unqualified(id, &scope, false, checkable);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let mut aliases = Vec::new();
        for item in &s.projection {
            match item {
                SelectItem::UnnamedExpr(e) => self.expr(e, &scope, false, checkable),
                SelectItem::ExprWithAlias { expr, alias } => {
                    self.expr(expr, &scope, false, checkable);
                    aliases.push(alias.value.clone());
                }
                _ => {}
            }
        }
        scope.aliases = aliases;

        if let Some(e) = &s.selection {
            self.expr(e, &scope, false, checkable);
        }
        if let GroupByExpr::Expressions(exprs, _) = &s.group_by {
            for e in exprs {
                if !matches!(e, Expr::Value(_)) {
                    self.expr(e, &scope, true, checkable);
                }
            }
        }
        if let Some(e) = &s.having {
            self.expr(e, &scope, true, checkable);
        }
        if let Some(e) = &s.qualify {
            self.expr(e, &scope, true, checkable);
        }
        for obe in &s.sort_by {
            self.order_by_expr(obe, &scope, checkable);
        }
        if let Some(OrderBy {
            kind: OrderByKind::Expressions(exprs),
            ..
        }) = order_by
        {
            for obe in exprs {
                self.order_by_expr(obe, &scope, checkable);
            }
        }

        self.projection_cols(s, &scope)
    }

    fn order_by_expr(&mut self, obe: &OrderByExpr, scope: &Scope, checkable: bool) {
        if !matches!(obe.expr, Expr::Value(_)) {
            self.expr(&obe.expr, scope, true, checkable);
        }
    }

    /// Bind one FROM/JOIN item into `relations`.
    fn table_factor(&mut self, tf: &TableFactor, relations: &mut Vec<Rel>, outer: Option<&Scope>) {
        match tf {
            TableFactor::Table {
                name, alias, args, ..
            } => {
                if args.is_some() {
                    relations.push(Rel {
                        binding: binding_of(alias, name),
                        cols: Cols::Unknown,
                    });
                    return;
                }
                let cols = self.relation_cols(name);
                let cols = alias_cols_opt(alias).unwrap_or(cols);
                relations.push(Rel {
                    binding: binding_of(alias, name),
                    cols,
                });
            }
            TableFactor::Derived {
                subquery, alias, ..
            } => {
                let tmp = Scope {
                    parent: outer,
                    relations: relations.clone(),
                    aliases: Vec::new(),
                };
                let derived = self.query(subquery, Some(&tmp));
                let cols = alias_cols_opt(alias).unwrap_or(derived);
                relations.push(Rel {
                    binding: alias.as_ref().map(|a| a.name.value.clone()),
                    cols,
                });
            }
            TableFactor::NestedJoin {
                table_with_joins,
                alias,
            } => {
                if alias.is_some() {
                    relations.push(Rel {
                        binding: alias.as_ref().map(|a| a.name.value.clone()),
                        cols: Cols::Unknown,
                    });
                    return;
                }
                self.table_factor(&table_with_joins.relation, relations, outer);
                for join in &table_with_joins.joins {
                    self.table_factor(&join.relation, relations, outer);
                }
            }
            _ => relations.push(Rel {
                binding: None,
                cols: Cols::Unknown,
            }),
        }
    }

    /// The columns a table reference exposes: CTEs shadow the catalog; a missing
    /// catalog entry is the *table* diagnostic (its columns then go quiet).
    fn relation_cols(&mut self, name: &ObjectName) -> Cols {
        if let [part] = name.0.as_slice() {
            if let Some(id) = part.as_ident() {
                if let Some((_, cols)) = self
                    .ctes
                    .iter()
                    .rev()
                    .find(|(n, _)| n.eq_ignore_ascii_case(&id.value))
                {
                    return cols.clone();
                }
            }
        }
        let Ok(table_ref) = object_name_to_table_reference(name.clone(), self.normalize) else {
            return Cols::Unknown;
        };
        match self.schemas.get(&table_ref.to_string()) {
            Some(OracleColumns::Known(cols)) => Cols::Known(cols.clone()),
            Some(OracleColumns::Missing) => {
                let span = self.byte_span(name.span());
                self.push(format!("Table or view '{table_ref}' not found"), span);
                Cols::Unknown
            }
            _ => Cols::Unknown,
        }
    }

    /// The output columns of a SELECT, for binding as a CTE/derived table. Any
    /// underivable item makes the whole projection unknowable.
    fn projection_cols(&self, s: &Select, scope: &Scope) -> Cols {
        let mut cols = Vec::new();
        for item in &s.projection {
            match item {
                SelectItem::ExprWithAlias { alias, .. } => cols.push(alias.value.clone()),
                SelectItem::UnnamedExpr(Expr::Identifier(id)) => cols.push(id.value.clone()),
                SelectItem::UnnamedExpr(Expr::CompoundIdentifier(parts)) => match parts.last() {
                    Some(id) => cols.push(id.value.clone()),
                    None => return Cols::Unknown,
                },
                SelectItem::Wildcard(_) => {
                    for rel in &scope.relations {
                        match &rel.cols {
                            Cols::Known(c) => cols.extend(c.iter().cloned()),
                            Cols::Unknown => return Cols::Unknown,
                        }
                    }
                }
                _ => return Cols::Unknown,
            }
        }
        Cols::Known(cols)
    }

    /// Walk a function call's four expression-bearing parts: its arguments, the `OVER` window
    /// spec, a `FILTER` clause, and `WITHIN GROUP`.
    ///
    /// The function *name* is the planner's to judge (`FunctionCatalog` + engine arity); only
    /// these carry column refs.
    fn function(&mut self, f: &Function, scope: &Scope, allow_aliases: bool, checkable: bool) {
        match &f.args {
            FunctionArguments::List(list) => {
                for arg in &list.args {
                    let fae = match arg {
                        FunctionArg::Named { arg, .. } => arg,
                        FunctionArg::ExprNamed { arg, .. } => arg,
                        FunctionArg::Unnamed(fae) => fae,
                    };
                    if let FunctionArgExpr::Expr(e) = fae {
                        self.expr(e, scope, allow_aliases, checkable);
                    }
                }
            }
            FunctionArguments::Subquery(q) => {
                self.query(q, Some(scope));
            }
            FunctionArguments::None => {}
        }
        if let Some(WindowType::WindowSpec(spec)) = &f.over {
            for e in &spec.partition_by {
                self.expr(e, scope, allow_aliases, checkable);
            }
            for obe in &spec.order_by {
                self.order_by_expr(obe, scope, checkable);
            }
        }
        if let Some(filter) = &f.filter {
            self.expr(filter, scope, allow_aliases, checkable);
        }
        for obe in &f.within_group {
            self.order_by_expr(obe, scope, checkable);
        }
    }

    /// Walk one expression against `scope`. `allow_aliases` marks the
    /// post-projection clauses where select aliases are legal targets;
    /// `checkable` is false in draft scopes (walk still recurses for subqueries).
    fn expr(&mut self, e: &Expr, scope: &Scope, allow_aliases: bool, checkable: bool) {
        match e {
            Expr::Identifier(id) => self.check_unqualified(id, scope, allow_aliases, checkable),
            Expr::CompoundIdentifier(parts) => self.check_qualified(parts, scope, checkable),
            Expr::Subquery(q) => {
                self.query(q, Some(scope));
            }
            Expr::Exists { subquery, .. } => {
                self.query(subquery, Some(scope));
            }
            Expr::InSubquery { expr, subquery, .. } => {
                self.expr(expr, scope, allow_aliases, checkable);
                self.query(subquery, Some(scope));
            }
            Expr::Function(f) => self.function(f, scope, allow_aliases, checkable),
            Expr::BinaryOp { left, right, .. } => {
                self.expr(left, scope, allow_aliases, checkable);
                self.expr(right, scope, allow_aliases, checkable);
            }
            Expr::UnaryOp { expr, .. }
            | Expr::Nested(expr)
            | Expr::Cast { expr, .. }
            | Expr::Collate { expr, .. }
            | Expr::IsNull(expr)
            | Expr::IsNotNull(expr)
            | Expr::IsTrue(expr)
            | Expr::IsNotTrue(expr)
            | Expr::IsFalse(expr)
            | Expr::IsNotFalse(expr)
            | Expr::IsUnknown(expr)
            | Expr::IsNotUnknown(expr) => self.expr(expr, scope, allow_aliases, checkable),
            Expr::IsDistinctFrom(a, b) | Expr::IsNotDistinctFrom(a, b) => {
                self.expr(a, scope, allow_aliases, checkable);
                self.expr(b, scope, allow_aliases, checkable);
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.expr(expr, scope, allow_aliases, checkable);
                self.expr(low, scope, allow_aliases, checkable);
                self.expr(high, scope, allow_aliases, checkable);
            }
            Expr::InList { expr, list, .. } => {
                self.expr(expr, scope, allow_aliases, checkable);
                for e in list {
                    self.expr(e, scope, allow_aliases, checkable);
                }
            }
            Expr::Like { expr, pattern, .. }
            | Expr::ILike { expr, pattern, .. }
            | Expr::SimilarTo { expr, pattern, .. } => {
                self.expr(expr, scope, allow_aliases, checkable);
                self.expr(pattern, scope, allow_aliases, checkable);
            }
            Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => {
                if let Some(op) = operand {
                    self.expr(op, scope, allow_aliases, checkable);
                }
                for when in conditions {
                    self.expr(&when.condition, scope, allow_aliases, checkable);
                    self.expr(&when.result, scope, allow_aliases, checkable);
                }
                if let Some(e) = else_result {
                    self.expr(e, scope, allow_aliases, checkable);
                }
            }
            Expr::Tuple(exprs) => {
                for e in exprs {
                    self.expr(e, scope, allow_aliases, checkable);
                }
            }
            Expr::CompoundFieldAccess { root, access_chain } => {
                let mut path: Vec<&Ident> = Vec::new();
                match root.as_ref() {
                    Expr::Identifier(id) => path.push(id),
                    other => self.expr(other, scope, allow_aliases, checkable),
                }
                if !path.is_empty() {
                    for access in access_chain {
                        match access {
                            AccessExpr::Dot(Expr::Identifier(id)) => path.push(id),
                            _ => break,
                        }
                    }
                    match path.as_slice() {
                        [id] => self.check_unqualified(id, scope, allow_aliases, checkable),
                        [q, c] => {
                            self.check_qualified(&[(*q).clone(), (*c).clone()], scope, checkable);
                        }
                        _ => {}
                    }
                }
                for access in access_chain {
                    if let AccessExpr::Subscript(sub) = access {
                        match sub {
                            Subscript::Index { index } => {
                                self.expr(index, scope, allow_aliases, checkable);
                            }
                            Subscript::Slice {
                                lower_bound,
                                upper_bound,
                                stride,
                            } => {
                                for e in [lower_bound, upper_bound, stride].into_iter().flatten() {
                                    self.expr(e, scope, allow_aliases, checkable);
                                }
                            }
                        }
                    }
                }
            }
            Expr::AnyOp { left, right, .. } | Expr::AllOp { left, right, .. } => {
                self.expr(left, scope, allow_aliases, checkable);
                self.expr(right, scope, allow_aliases, checkable);
            }
            other => {
                if contains_unwalkable(other) {
                    return;
                }
                let mut idents: Vec<Expr> = Vec::new();
                let _ = visit_expressions(other, |x: &Expr| {
                    if matches!(x, Expr::Identifier(_) | Expr::CompoundIdentifier(_)) {
                        idents.push(x.clone());
                    }
                    ControlFlow::<()>::Continue(())
                });
                for x in &idents {
                    match x {
                        Expr::Identifier(id) => {
                            self.check_unqualified(id, scope, allow_aliases, checkable);
                        }
                        Expr::CompoundIdentifier(parts) => {
                            self.check_qualified(parts, scope, checkable);
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
    }

    /// A bare column reference: flag only when the whole scope chain is fully
    /// known and nothing — column, legal alias, outer scope — matches.
    fn check_unqualified(
        &mut self,
        id: &Ident,
        scope: &Scope,
        allow_aliases: bool,
        checkable: bool,
    ) {
        if !checkable || scope.chain_has_unknown() {
            return;
        }
        if scope.chain_has_column(&id.value) {
            return;
        }
        if allow_aliases
            && scope
                .aliases
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&id.value))
        {
            return;
        }
        let span = self.byte_span(id.span);
        self.push(self.column_message(&id.value, scope), span);
    }

    /// A qualified `q.c` reference. Three-part-and-longer names, struct-field
    /// access (`address.city` where `address` is a column), and unknowable
    /// qualifiers all stay quiet.
    fn check_qualified(&mut self, parts: &[Ident], scope: &Scope, checkable: bool) {
        if !checkable || parts.len() != 2 {
            return;
        }
        let (qualifier, column) = (&parts[0], &parts[1]);
        if scope.chain_has_column(&qualifier.value) {
            return;
        }
        match scope.chain_binding(&qualifier.value) {
            Some(rel) => match &rel.cols {
                Cols::Unknown => {}
                cols @ Cols::Known(_) => {
                    if !cols.contains(&column.value) {
                        let span = self.byte_span(qualifier.span.union(&column.span));
                        self.push(
                            format!("Column '{}.{}' not found", qualifier.value, column.value),
                            span,
                        );
                    }
                }
            },
            None => {
                if !scope.chain_has_unknown() {
                    let span = self.byte_span(qualifier.span.union(&column.span));
                    self.push(
                        format!("Column '{}.{}' not found", qualifier.value, column.value),
                        span,
                    );
                }
            }
        }
    }

    /// Unknown-column wording, with a best-effort suggestion from the in-scope
    /// column names (subsequence match — typo-shaped misses only).
    fn column_message(&self, name: &str, scope: &Scope) -> String {
        let mut best: Option<(u8, &str)> = None;
        for s in scope.chain() {
            for rel in &s.relations {
                if let Cols::Known(cols) = &rel.cols {
                    for c in cols {
                        if let Some(tier) = fuzzy::match_tier(c, name) {
                            if best.is_none_or(|(t, _)| tier < t) {
                                best = Some((tier, c));
                            }
                        }
                    }
                }
            }
        }
        match best {
            Some((_, suggestion)) => {
                format!("Column '{name}' not found. Did you mean '{suggestion}'?")
            }
            None => format!("Column '{name}' not found"),
        }
    }

    /// This walk's spans as byte ranges into the full buffer — [`lex::byte_span`] against the
    /// statement slice it is walking.
    fn byte_span(&self, span: Span) -> Option<Range<usize>> {
        byte_span(self.slice, self.stmt_start, span)
    }

    /// Emit one diagnostic. A spanless fault (empty-span sentinel) points at the
    /// statement head instead — every error must squiggle somewhere.
    fn push(&mut self, message: String, span: Option<Range<usize>>) {
        let span = span.unwrap_or_else(|| {
            let head = self.stmt_start;
            head..(head + self.slice.trim_end().len()).max(head + 1)
        });
        self.diags
            .push(diag(Severity::Error, message, span, self.sql));
    }
}

/// The explicit column list of a table alias (`AS x(a, b)`), if one was written.
fn alias_cols(alias: &TableAlias) -> Option<Cols> {
    (!alias.columns.is_empty())
        .then(|| Cols::Known(alias.columns.iter().map(|c| c.name.value.clone()).collect()))
}

fn alias_cols_opt(alias: &Option<TableAlias>) -> Option<Cols> {
    alias.as_ref().and_then(alias_cols)
}

/// The name a qualifier resolves against: alias first, else the table's last part.
fn binding_of(alias: &Option<TableAlias>, name: &ObjectName) -> Option<String> {
    if let Some(a) = alias {
        return Some(a.name.value.clone());
    }
    name.0
        .last()
        .and_then(|p| p.as_ident())
        .map(|id| id.value.clone())
}

/// The join's ON/USING constraint, where its operator carries one.
fn join_constraint(op: &JoinOperator) -> Option<&JoinConstraint> {
    match op {
        JoinOperator::Join(c)
        | JoinOperator::Inner(c)
        | JoinOperator::Left(c)
        | JoinOperator::LeftOuter(c)
        | JoinOperator::Right(c)
        | JoinOperator::RightOuter(c)
        | JoinOperator::FullOuter(c)
        | JoinOperator::Semi(c)
        | JoinOperator::LeftSemi(c)
        | JoinOperator::RightSemi(c)
        | JoinOperator::Anti(c)
        | JoinOperator::LeftAnti(c)
        | JoinOperator::RightAnti(c)
        | JoinOperator::StraightJoin(c) => Some(c),
        _ => None,
    }
}

/// Whether the expression tree contains a shape the flat identifier visit would
/// mis-read: a subquery (its identifiers belong to another scope) or a
/// field-access chain (its root identifier is a path head, not a bare column).
fn contains_unwalkable(e: &Expr) -> bool {
    let mut found = false;
    let _ = visit_expressions(e, |x: &Expr| {
        if matches!(
            x,
            Expr::Subquery(_)
                | Expr::Exists { .. }
                | Expr::InSubquery { .. }
                | Expr::CompoundFieldAccess { .. }
        ) {
            found = true;
            return ControlFlow::Break(());
        }
        ControlFlow::<()>::Continue(())
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::sql::sqlparser::dialect::GenericDialect;
    use datafusion::sql::sqlparser::parser::Parser;

    /// Resolve `sql` (one statement) against a hand-built schema map — the sync
    /// walk only, no session.
    fn run_with(sql: &str, tables: &[(&str, &[&str])]) -> Resolution {
        let mut schemas = SchemaMap::new();
        for (name, cols) in tables {
            schemas.insert(
                (*name).to_string(),
                OracleColumns::Known(cols.iter().map(ToString::to_string).collect()),
            );
        }
        let stmt = Parser::parse_sql(&GenericDialect {}, sql)
            .expect("parse")
            .pop()
            .expect("one statement");
        resolve_statement(&stmt, &schemas, true, sql, 0, sql)
    }

    fn spanned<'a>(sql: &'a str, d: &Diagnostic) -> &'a str {
        &sql[d.span.clone().expect("span")]
    }

    #[test]
    fn every_unknown_name_is_reported() {
        let sql = "SELECT nme, product_idd FROM t";
        let r = run_with(sql, &[("t", &["id", "name"])]);
        assert_eq!(
            r.diags.len(),
            2,
            "{:?}",
            r.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert_eq!(spanned(sql, &r.diags[0]), "nme");
        assert_eq!(spanned(sql, &r.diags[1]), "product_idd");
        assert!(r.complete);
    }

    #[test]
    fn typo_shaped_misses_get_a_suggestion() {
        let r = run_with("SELECT nme FROM t", &[("t", &["id", "name"])]);
        assert!(
            r.diags[0].message.contains("Did you mean 'name'?"),
            "{}",
            r.diags[0].message
        );
    }

    #[test]
    fn struct_field_access_stays_quiet() {
        let r = run_with("SELECT address.city FROM s", &[("s", &["address"])]);
        assert!(
            r.diags.is_empty(),
            "{:?}",
            r.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn three_part_names_are_left_to_the_planner() {
        let r = run_with("SELECT a.b.c FROM s", &[("s", &["x"])]);
        assert!(r.diags.is_empty());
    }

    #[test]
    fn table_functions_make_the_scope_unknowable() {
        let r = run_with("SELECT whatever FROM generate_series(1, 10)", &[]);
        assert!(r.diags.is_empty());
        assert!(!r.complete);
    }

    #[test]
    fn wildcard_derived_tables_expand() {
        let sql = "SELECT d.missing FROM (SELECT * FROM s) d";
        let r = run_with(sql, &[("s", &["id", "name"])]);
        assert_eq!(r.diags.len(), 1);
        assert_eq!(spanned(sql, &r.diags[0]), "d.missing");
    }

    #[test]
    fn computed_derived_projections_stay_quiet() {
        let r = run_with(
            "SELECT d.x FROM (SELECT id + 1 FROM s) d",
            &[("s", &["id"])],
        );
        assert!(r.diags.is_empty());
        assert!(!r.complete);
    }

    #[test]
    fn recursive_cte_self_reference_stays_quiet() {
        let r = run_with(
            "WITH RECURSIVE r AS (SELECT id FROM r) SELECT id FROM r",
            &[("s", &["id"])],
        );
        assert!(
            r.diags.is_empty(),
            "{:?}",
            r.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn non_recursive_cte_reads_the_real_table_it_shadows() {
        let sql = "WITH t AS (SELECT missing FROM t) SELECT missing FROM t";
        let r = run_with(sql, &[("t", &["id", "name"])]);
        assert_eq!(
            r.diags.len(),
            1,
            "{:?}",
            r.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        assert_eq!(spanned(sql, &r.diags[0]), "missing");
        assert_eq!(
            r.diags[0].span.clone().unwrap().start,
            "WITH t AS (SELECT ".len()
        );
    }

    #[test]
    fn alias_column_lists_rename() {
        let sql = "SELECT x.id FROM t AS x(a, b)";
        let r = run_with(sql, &[("t", &["id", "name"])]);
        assert_eq!(r.diags.len(), 1);
        assert_eq!(spanned(sql, &r.diags[0]), "x.id");
        assert!(
            run_with("SELECT x.a FROM t AS x(a, b)", &[("t", &["id", "name"])])
                .diags
                .is_empty()
        );
    }

    #[test]
    fn empty_span_sentinel_falls_back_to_the_statement_head() {
        let sql = "SELECT 1";
        let r = Resolver {
            schemas: &SchemaMap::new(),
            normalize: true,
            slice: sql,
            stmt_start: 0,
            sql,
            ctes: Vec::new(),
            diags: Vec::new(),
            complete: true,
        };
        assert_eq!(r.byte_span(Span::empty()), None);
        let mut r = r;
        r.push("synthesized".into(), None);
        let span = r.diags[0].span.clone().expect("fallback span");
        assert_eq!(span, 0..sql.len());
    }
}
