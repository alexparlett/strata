//! **Statements the server runs** — `CREATE VIEW` (materialized or not), `DROP VIEW`,
//! `DROP TABLE`, a column-list `CREATE TABLE`, `UPDATE` and `DELETE`, each against a relation
//! inside a source. `docs/STATEMENTS_SPEC.md` §6.9.
//!
//! What DataFusion can plan against a remote catalog is planned; what only the server can run is
//! **dispatched** — the statement the user typed, with the catalog qualifier cut out. Splicing the
//! buffer rather than re-rendering the AST is what makes this a generic capability rather than a
//! clause whitelist: every clause Strata does not model travels intact and the server is the
//! clause gate.

use std::collections::HashSet;
use std::ops::{ControlFlow, Range};

use datafusion::prelude::SessionContext;
use datafusion::sql::parser::Statement as DFStatement;
use datafusion::sql::sqlparser::ast::{
    FromTable, Ident, ObjectName, ObjectNamePart, Query, Statement as SqlStatement, TableFactor,
    Visit, Visitor,
};
use datafusion::sql::sqlparser::tokenizer::Location;
use datafusion::sql::TableReference;

use crate::catalog::remote_dependents;
use crate::policy::Principal;
use crate::providers::in_workspace;
use crate::sources::{execute_text, relist_at, server_ident, writable, Live};
use crate::statements::ctx::StmtCtx;
use crate::statements::mechanism::{mechanism, Mechanism};
use crate::statements::pipeline::Qualified;
use crate::statements::report::{StatementOutcome, StoreEffect};
use crate::statements::target::{read_only, resolve_named, resolve_target, Remote, Target};
use crate::statements::StmtKind;
use crate::{fold_ident, CATALOG, SCHEMA};
use strata_core::util::plural;

use super::left_invalid;

/// The relation `kind` addresses, when it is one inside a data source **and** the kind's
/// [`Mechanism`] is to hand the statement to the server as text — the one answer every arm and
/// the editor read, so the two cannot disagree about which statements the server owns.
///
/// The mechanism is asked first, so a kind whose remote form is planned into the source's sink
/// (a CTAS, an `INSERT`) never reaches the splice, and a kind with no remote form at all reaches
/// its own refusal. The AST match below then reads the managed name **off the parsed statement**,
/// because these are the statements that must answer before anything plans.
pub(super) fn target(ctx: &SessionContext, kind: StmtKind, stmt: &DFStatement) -> Option<Remote> {
    if mechanism(kind) != Mechanism::ServerText {
        return None;
    }
    let target = match kind {
        StmtKind::Update | StmtKind::Delete => resolve_target(ctx, &dml_target(stmt).ok()?),
        _ => resolve_named(ctx, managed_name(kind, stmt)?),
    };
    match target {
        Target::Remote(at) => Some(at),
        Target::Workspace { .. } | Target::Store(_) | Target::Nowhere { .. } => None,
    }
}

/// The name a create or a drop manages, read off the parsed statement — `None` where the
/// classifier and sqlparser disagree about the shape, and for a `DROP` naming several objects,
/// which nothing here can dispatch as one relation.
fn managed_name(kind: StmtKind, stmt: &DFStatement) -> Option<&ObjectName> {
    let DFStatement::Statement(s) = stmt else {
        return None;
    };
    match (kind, s.as_ref()) {
        (StmtKind::CreateTable, SqlStatement::CreateTable(create)) => Some(&create.name),
        (StmtKind::CreateView, SqlStatement::CreateView(view)) => Some(&view.name),
        (StmtKind::DropTable | StmtKind::DropView, SqlStatement::Drop { names, .. }) => {
            match names.as_slice() {
                [one] => Some(one),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether the statement goes to a server as text, which is what tells the editor not to judge it:
/// its types, its functions and its clauses are the server's vocabulary, so a dry-plan of one
/// squiggles a statement Run performs.
pub(crate) fn dispatched(ctx: &SessionContext, kind: StmtKind, stmt: &DFStatement) -> bool {
    target(ctx, kind, stmt).is_some()
}

/// One statement, run on the server behind both gates, in an order that is load-bearing: a
/// refusal must never have reached the server, and the splice must never run over a statement the
/// body check would have stopped.
async fn dispatch(cx: &StmtCtx, at: &Remote, stmt: &DFStatement) -> Result<u64, String> {
    let sources = &cx.live;
    if !writable(sources, &at.source) {
        return Err(read_only(at));
    }
    let named = Named::of(stmt);
    named.check(&cx.ctx, &at.source)?;
    let sql = splice(&cx.sql, &named.names, &at.source, sources)?;
    execute_text(sources, &at.source, &sql).await
}

/// [`dispatch`], plus the re-enumeration a statement that changed what the server holds owes: it
/// is what puts a new relation in the tree with no ↻ and what drops the cached provider of one
/// that is gone.
async fn changed(
    cx: &StmtCtx,
    at: &Remote,
    stmt: &DFStatement,
    message: String,
) -> Result<StatementOutcome, String> {
    dispatch(cx, at, stmt).await?;
    relist_at(&cx.live, &at.source).await;
    Ok(StatementOutcome {
        message,
        count: None,
        effect: Some(StoreEffect::RemoteRelationsChanged),
    })
}

/// `CREATE VIEW` / `CREATE MATERIALIZED VIEW` inside a data source — the only arm that
/// accepts `MATERIALIZED`, the workspace having no such concept.
pub(super) async fn create_view(
    cx: &StmtCtx,
    at: &Remote,
    materialized: bool,
    stmt: &Qualified,
) -> Result<StatementOutcome, String> {
    let what = match materialized {
        true => "Materialized view",
        false => "View",
    };
    let message = format!(
        "{what} '{}' created on '{}'",
        at.server_address(),
        at.source
    );
    changed(cx, at, stmt, message).await
}

/// A **column-list** `CREATE TABLE` inside a source, whose types are the server's own
/// vocabulary (`jsonb`, `serial`) and only the server's to judge.
pub(super) async fn create_table(
    cx: &StmtCtx,
    at: &Remote,
    stmt: &Qualified,
) -> Result<StatementOutcome, String> {
    let message = format!("Table '{}' created on '{}'", at.server_address(), at.source);
    changed(cx, at, stmt, message).await
}

/// `DROP TABLE` / `DROP VIEW` inside a source, naming the workspace views left
/// reading the relation without cascading — existence is the server's question, the listing being
/// only what the data source last told us and `IF EXISTS` travelling in the statement.
pub(super) async fn drop_relation(
    cx: &StmtCtx,
    at: &Remote,
    view: bool,
    stmt: &Qualified,
) -> Result<StatementOutcome, String> {
    let dependents = remote_dependents(&cx.ctx, at.recorded()).await;
    let what = match view {
        true => "View",
        false => "Table",
    };
    let message = format!(
        "{what} '{}' dropped on '{}'{}",
        at.server_address(),
        at.source,
        left_invalid(&dependents)
    );
    changed(cx, at, stmt, message).await
}

/// `UPDATE` and `DELETE`, remote-only, reporting the **server's** own affected-row count and no
/// effect at all, rows being not relations. No `WHERE`-less guard either: the typed statement is
/// the intent and the read-only toggle is the belt, the terms `DROP TABLE` already dispatches on.
pub(super) async fn update(
    cx: &StmtCtx,
    who: &Principal,
    stmt: &Qualified,
) -> Result<StatementOutcome, String> {
    dml(cx, who, stmt, StmtKind::Update).await
}

/// `DELETE`, [`update`]'s twin — one body, because the two differ only in the verb their report
/// uses and in the preposition in front of the relation.
pub(super) async fn delete(
    cx: &StmtCtx,
    who: &Principal,
    stmt: &Qualified,
) -> Result<StatementOutcome, String> {
    dml(cx, who, stmt, StmtKind::Delete).await
}

/// The body both DML statements share.
async fn dml(
    cx: &StmtCtx,
    who: &Principal,
    stmt: &Qualified,
    kind: StmtKind,
) -> Result<StatementOutcome, String> {
    dml_target(stmt)?;
    let Some(at) = target(&cx.ctx, kind, stmt) else {
        return Err(workspace_dml(kind));
    };
    cx.require_target(who, kind, &Target::Remote(at.clone()))
        .await?;
    let rows = dispatch(cx, &at, stmt).await?;
    let verb = match kind {
        StmtKind::Delete => "Deleted",
        _ => "Updated",
    };
    let preposition = match kind {
        StmtKind::Delete => "from",
        _ => "in",
    };
    Ok(StatementOutcome {
        message: format!(
            "{verb} {} {preposition} '{}' on '{}'",
            plural(rows as usize, "row"),
            at.server_address(),
            at.source
        ),
        count: Some(rows),
        effect: None,
    })
}

/// The one relation an `UPDATE` or a `DELETE` targets, read off the parsed statement because
/// nothing here plans; the multi-table forms have no single target and are refused by name.
fn dml_target(stmt: &DFStatement) -> Result<TableReference, String> {
    let DFStatement::Statement(s) = stmt else {
        return Err(not_dml());
    };
    let table = match s.as_ref() {
        SqlStatement::Update(update) if update.table.joins.is_empty() => &update.table.relation,
        SqlStatement::Delete(delete) if delete.tables.is_empty() => match &delete.from {
            FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from) => {
                match from.as_slice() {
                    [one] if one.joins.is_empty() => &one.relation,
                    _ => return Err(one_relation()),
                }
            }
        },
        SqlStatement::Update(_) | SqlStatement::Delete(_) => return Err(one_relation()),
        _ => return Err(not_dml()),
    };
    let TableFactor::Table { name, .. } = table else {
        return Err(one_relation());
    };
    Ok(TableReference::parse_str(&name.to_string()))
}

/// The router said this was an `UPDATE` or a `DELETE` and sqlparser parses it as one; anything
/// else is the two disagreeing.
fn not_dml() -> String {
    "The statement did not parse as an UPDATE or a DELETE".to_string()
}

/// What a target Strata cannot name one relation for is refused with.
fn one_relation() -> String {
    "UPDATE and DELETE support one target relation".to_string()
}

/// What an `UPDATE` or a `DELETE` over a workspace table says — its own sentence, because
/// `Fault::Unsupported`'s generic wording stops being honest the moment the same verb works one
/// qualifier away.
fn workspace_dml(kind: StmtKind) -> String {
    format!(
        "{} works on a relation in a data source. A table in this project is stored as \
         files that cannot be changed in place; drop it and recreate it with CREATE TABLE AS",
        kind.label()
    )
}

/// Every relation name a statement carries, and the names it binds or calls rather than reads —
/// one collection for both gates, since the check asks whether each name is the data source's and
/// the splice cuts that data source's qualifier out of each.
///
/// A CTE name is held back because the server binds it identically, and a table factor carrying an
/// argument list because it is a function call (`FROM generate_series(1, 10)`) rather than a
/// relation. Both are **single identifiers**, which is what the set holds and what it is matched
/// against; it is deliberately flat across nesting, over-collecting there only leaving a name
/// alone.
#[derive(Default)]
struct Named {
    names: Vec<ObjectName>,
    held: HashSet<String>,
}

impl Named {
    /// The names in `stmt`, plus the three targets sqlparser's `visit_relations` annotations miss
    /// — the same three the snapshot fence in `sql::validate` names explicitly.
    fn of(stmt: &DFStatement) -> Self {
        let mut named = Named::default();
        let DFStatement::Statement(s) = stmt else {
            return named;
        };
        let _ = s.as_ref().visit(&mut named);
        match s.as_ref() {
            SqlStatement::CreateView(view) => named.names.push(view.name.clone()),
            SqlStatement::Drop { names, .. } => named.names.extend(names.iter().cloned()),
            SqlStatement::Delete(delete) => named.names.extend(delete.tables.iter().cloned()),
            _ => {}
        }
        named
    }

    /// Every name the statement will put in front of the server is one of `catalog`'s, refused
    /// **by name** otherwise: a server-side view cannot read across sources, and an unqualified
    /// name would resolve by the server's search path, which is a different answer from the one
    /// the editor gives the same spelling. A short name the **workspace** holds is refused as the
    /// outsider it is rather than as an unqualified one, the resolution pass having left it bare.
    fn check(&self, ctx: &SessionContext, catalog: &str) -> Result<(), String> {
        for name in &self.names {
            if self.is_held(name) {
                continue;
            }
            match parts(name) {
                Some([owner, _, _]) if fold_ident(&owner.value) == fold_ident(catalog) => {}
                Some(_) => return Err(elsewhere(name, catalog)),
                None if workspace_holds(ctx, name) => return Err(elsewhere(name, catalog)),
                None => return Err(not_qualified(name, catalog)),
            }
        }
        Ok(())
    }

    /// Whether `name` is one the statement binds or calls rather than reads — **a single part
    /// only**, because a CTE is referenced by its bare alias and an unqualified call is one
    /// identifier, so a qualified name is never either. Matching on the last part alone would
    /// exempt `warehouse.public.orders` from the check for no better reason than the statement
    /// binding a CTE called `orders`, and the splice would then carry it to the wrong server.
    fn is_held(&self, name: &ObjectName) -> bool {
        match name.0.as_slice() {
            [ObjectNamePart::Identifier(ident)] => self.held.contains(&fold_ident(&ident.value)),
            _ => false,
        }
    }

    /// Bind `name` if it is the one shape [`is_held`] can match; a qualified call needs no
    /// holding, since it passes the check as the qualified name it is.
    fn hold(&mut self, name: &ObjectName) {
        if let [ObjectNamePart::Identifier(ident)] = name.0.as_slice() {
            self.held.insert(fold_ident(&ident.value));
        }
    }
}

impl Visitor for Named {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.held.insert(fold_ident(&cte.alias.name.value));
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<()> {
        if let TableFactor::Table {
            name,
            args: Some(_),
            ..
        } = factor
        {
            self.hold(name);
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, name: &ObjectName) -> ControlFlow<()> {
        self.names.push(name.clone());
        ControlFlow::Continue(())
    }
}

/// The three identifiers a name is made of, or `None` for every other shape — bare, two-part,
/// four-part, and the dialect-specific part that is a function call.
fn parts(name: &ObjectName) -> Option<[&Ident; 3]> {
    match name.0.as_slice() {
        [ObjectNamePart::Identifier(a), ObjectNamePart::Identifier(b), ObjectNamePart::Identifier(c)] => {
            Some([a, b, c])
        }
        _ => None,
    }
}

/// Whether the workspace's one schema holds `name` — the tables, the views and the snapshot
/// spool, which is what the resolution pass asked before leaving the name bare.
fn workspace_holds(ctx: &SessionContext, name: &ObjectName) -> bool {
    let reference = TableReference::parse_str(&name.to_string());
    in_workspace(&reference)
        && ctx
            .catalog(CATALOG)
            .and_then(|catalog| catalog.schema(SCHEMA))
            .is_some_and(|schema| schema.table_exist(reference.table()))
}

/// What a name outside the target data source is refused with — the workspace's own relations,
/// another data source's, and a qualifier that names nothing.
fn elsewhere(name: &ObjectName, catalog: &str) -> String {
    format!(
        "'{name}' is not in the data source '{catalog}'. A statement that runs on the \
         server can only name relations in that source"
    )
}

/// What a name missing the data source's qualifier is refused with; a two-part `public.orders` is
/// the same fault as a bare one, since the editor reads it as the workspace's single schema.
fn not_qualified(name: &ObjectName, catalog: &str) -> String {
    format!(
        "'{name}' is not qualified. This statement runs on '{catalog}', where a bare name \
         resolves by the server's search path. Write it in full"
    )
}

/// The statement the server runs: `source` verbatim, with `catalog`'s qualifier cut out of every
/// name that carries it.
///
/// Two shapes, because a target may never have been written. A name the user qualified is cut
/// from the catalog part's start to the schema part's start, so the dot goes with it and their own
/// quoting survives for the server to judge. A name the resolution pass expanded from a bare one
/// has no three-part bytes to cut — its parts share the token's one span — so the whole token
/// becomes the server's spelling of `schema.relation`, quoted unconditionally as every identifier
/// Strata composes is.
fn splice(
    source: &str,
    names: &[ObjectName],
    catalog: &str,
    sources: &Live,
) -> Result<String, String> {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for name in names {
        let Some([owner, schema, table]) = parts(name) else {
            continue;
        };
        if fold_ident(&owner.value) != fold_ident(catalog) {
            continue;
        }
        let located = [owner, schema, table].map(|part| span_of(source, part));
        let [Some(at_owner), Some(at_schema), Some(at_table)] = located else {
            return Err(untrusted(name));
        };
        if at_owner == at_table {
            if fold_ident(&spelled(&source[at_owner.clone()])) != fold_ident(&table.value) {
                return Err(untrusted(name));
            }
            edits.push((
                at_owner,
                format!(
                    "{}.{}",
                    server_ident(sources, catalog, &schema.value),
                    server_ident(sources, catalog, &table.value)
                ),
            ));
            continue;
        }
        for (at, part) in [(&at_owner, owner), (&at_schema, schema), (&at_table, table)] {
            if source[at.clone()] != part.to_string() {
                return Err(untrusted(name));
            }
        }
        edits.push((at_owner.start..at_schema.start, String::new()));
    }
    apply(source, edits)
}

/// `source` with each edit's range replaced by its text, left to right; overlapping edits are the
/// one thing this cannot mean, so they are a refusal rather than a mangled statement.
fn apply(source: &str, mut edits: Vec<(Range<usize>, String)>) -> Result<String, String> {
    edits.sort_by_key(|(at, _)| at.start);
    let mut out = String::with_capacity(source.len());
    let mut cut = 0;
    for (at, text) in edits {
        if at.start < cut {
            return Err("Strata could not rewrite the statement for the server".to_string());
        }
        out.push_str(&source[cut..at.start]);
        out.push_str(&text);
        cut = at.end;
    }
    out.push_str(&source[cut..]);
    Ok(out)
}

/// What a name Strata cannot locate in the buffer is refused with, never a guess, which would be
/// a different statement sent to a server.
fn untrusted(name: &ObjectName) -> String {
    format!(
        "Strata could not locate '{name}' in the statement text. Write it in full and run it again"
    )
}

/// `ident`'s byte range in `source`, or `None` for the empty-span sentinel and for a position the
/// buffer does not have.
fn span_of(source: &str, ident: &Ident) -> Option<Range<usize>> {
    let start = byte_at(source, ident.span.start)?;
    let end = byte_at(source, ident.span.end)?;
    (start < end).then_some(start..end)
}

/// The byte offset of a sqlparser 1-based (line, column) in `source`, walking the line because
/// its columns count **characters** where `sql::lex`'s cheaper `rel_offset` counts bytes — an
/// approximation a squiggle tolerates and a splice must not.
fn byte_at(source: &str, at: Location) -> Option<usize> {
    if at.line == 0 || at.column == 0 {
        return None;
    }
    let start: usize = source
        .split_inclusive('\n')
        .take((at.line - 1) as usize)
        .map(str::len)
        .sum();
    let line = source.get(start..)?;
    let column = (at.column - 1) as usize;
    match line.char_indices().nth(column) {
        Some((offset, _)) => Some(start + offset),
        None => (line.chars().count() == column).then_some(start + line.len()),
    }
}

/// The name a lexeme spells, asked only of a slice a span says is one identifier and only to check
/// that it is the one the resolution pass replaced.
fn spelled(lexeme: &str) -> String {
    match lexeme.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => lexeme.to_string(),
    }
}

/// **The rewrite and the body check, against parsed statements** — the two halves a server cannot
/// settle, so they are pinned here rather than in `tests/postgres_federation.rs`.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::builder::test_context;
    use crate::fold_ident;
    use crate::policy::{Capability, CapabilityPolicyProvider};
    use crate::providers::fake_source;
    use crate::statements::pipeline::{resolved_one, Pipeline};

    use super::*;

    /// A session holding a workspace table `local` and two sources, `pg` and `warehouse`.
    fn session() -> SessionContext {
        let ctx = test_context(&BTreeMap::new());
        ctx.register_table(
            "local",
            std::sync::Arc::new(datafusion::datasource::empty::EmptyTable::new(
                std::sync::Arc::new(datafusion::arrow::datatypes::Schema::empty()),
            )),
        )
        .expect("workspace table");
        fake_source(&ctx, "pg", &["orders", "customers"]);
        fake_source(&ctx, "warehouse", &["shipments"]);
        ctx
    }

    /// `sql` through the one parse every surface enters, then spliced for `pg`.
    fn rewritten(sql: &str) -> Result<String, String> {
        let ctx = session();
        let stmt = resolved_one(&ctx, sql).map_err(|e| e.message())?;
        let named = Named::of(&stmt);
        named.check(&ctx, "pg")?;
        splice(sql, &named.names, "pg", &Live::default())
    }

    /// **Byte-identical outside the removed qualifiers**, which is the whole claim of splicing
    /// rather than rendering: the comment, the odd spacing, the storage parameters the local arm
    /// refuses by name, and the user's own quoting all survive.
    #[test]
    fn only_the_qualifiers_are_cut() {
        let sql = "CREATE VIEW pg.public.active WITH (security_barrier = true) /* keep me */ \
                   AS\n  SELECT id\n  FROM pg.public.\"orders\"";
        assert_eq!(
            rewritten(sql).expect("spliced"),
            "CREATE VIEW public.active WITH (security_barrier = true) /* keep me */ AS\n  \
             SELECT id\n  FROM public.\"orders\""
        );
    }

    /// A name written with spaces around its dots still loses exactly its catalog and the dot
    /// after it, because the cut runs to the schema part's own start.
    #[test]
    fn the_cut_runs_to_the_schema_part() {
        assert_eq!(
            rewritten("DROP TABLE pg . public . orders").expect("spliced"),
            "DROP TABLE public . orders"
        );
    }

    /// A bare name the resolution pass expanded has no three-part bytes to cut, so the token
    /// becomes the server's own spelling, quoted.
    #[test]
    fn a_resolved_bare_name_becomes_the_servers_spelling() {
        assert_eq!(
            rewritten("DELETE FROM orders WHERE id = 1").expect("spliced"),
            "DELETE FROM \"public\".\"orders\" WHERE id = 1"
        );
        assert_eq!(
            rewritten("UPDATE orders SET total = 0").expect("spliced"),
            "UPDATE \"public\".\"orders\" SET total = 0"
        );
    }

    /// Several names in one statement are each cut, left to right.
    #[test]
    fn every_name_in_the_statement_is_cut() {
        assert_eq!(
            rewritten(
                "CREATE VIEW pg.public.joined AS SELECT o.id FROM pg.public.orders o JOIN \
                 pg.public.customers c ON c.id = o.id"
            )
            .expect("spliced"),
            "CREATE VIEW public.joined AS SELECT o.id FROM public.orders o JOIN public.customers \
             c ON c.id = o.id"
        );
    }

    /// A body naming the workspace, another source, or nothing at all is refused **by name**:
    /// a statement the server runs can only reach that server. The workspace table is named as the
    /// outsider it is even though the resolution pass left it bare.
    #[test]
    fn a_body_reaching_outside_the_source_is_refused_by_name() {
        for (sql, named) in [
            ("CREATE VIEW pg.public.v AS SELECT * FROM local", "'local'"),
            (
                "CREATE VIEW pg.public.v AS SELECT * FROM warehouse.public.shipments",
                "warehouse.public.shipments",
            ),
            (
                "CREATE VIEW pg.public.v AS SELECT * FROM nosuch.public.t",
                "nosuch.public.t",
            ),
        ] {
            let why = rewritten(sql).expect_err("refused");
            assert!(why.contains(named), "'{sql}': {why}");
            assert!(why.contains("'pg'"), "'{sql}': {why}");
        }
    }

    /// A name the data source does not have stays bare through the resolution pass, and a bare
    /// name would resolve by the server's search path — so it is refused, saying so.
    #[test]
    fn an_unqualified_name_is_refused() {
        let why = rewritten("CREATE VIEW pg.public.v AS SELECT * FROM missing").expect_err("bare");
        assert!(
            why.contains("'missing'") && why.contains("search path"),
            "{why}"
        );

        let why =
            rewritten("CREATE VIEW pg.public.v AS SELECT * FROM public.orders").expect_err("part");
        assert!(why.contains("public.orders"), "{why}");
    }

    /// **A binding holds back the bare name and nothing else.** Matching a held name on its last
    /// part alone exempted `warehouse.public.orders` from the check whenever the statement bound a
    /// CTE called `orders` — and since the splice skips a name outside the target catalog, that
    /// out-of-source relation went to the `pg` server verbatim.
    #[test]
    fn a_binding_does_not_exempt_a_qualified_name_that_ends_in_it() {
        let why = rewritten(
            "CREATE VIEW pg.public.leak AS WITH orders AS (SELECT 1 AS id) SELECT * FROM \
             orders, warehouse.public.orders",
        )
        .expect_err("the other data source is still refused");
        assert!(why.contains("warehouse.public.orders"), "{why}");
        assert!(why.contains("'pg'"), "{why}");
    }

    /// The same rule from the call side: a **qualified** function call needs no holding, since it
    /// passes the check as the qualified name it is and is spliced like any other, and holding it
    /// would have exempted a bare name of that spelling elsewhere in the statement.
    #[test]
    fn a_qualified_call_is_spliced_and_holds_nothing() {
        assert_eq!(
            rewritten("SELECT * FROM pg.public.readings(1)").expect("spliced"),
            "SELECT * FROM public.readings(1)"
        );
        let why = rewritten("SELECT * FROM pg.public.readings(1), readings")
            .expect_err("the bare name is nobody's binding");
        assert!(
            why.contains("'readings'") && why.contains("search path"),
            "{why}"
        );
    }

    /// A CTE and a table function are the statement's own, not the data source's, so neither is
    /// refused and neither is rewritten.
    #[test]
    fn a_cte_and_a_function_call_are_left_alone() {
        let sql = "CREATE VIEW pg.public.v AS WITH orders AS (SELECT 1 AS id) SELECT * FROM \
                   orders, generate_series(1, 10)";
        assert_eq!(
            rewritten(sql).expect("spliced"),
            "CREATE VIEW public.v AS WITH orders AS (SELECT 1 AS id) SELECT * FROM orders, \
             generate_series(1, 10)"
        );
    }

    /// **What the editor may judge and what it may not**, off the one answer the arms take their
    /// branch from. A statement bound for the server is skipped by the dry-plan, so DataFusion
    /// never refuses SQL written in the server's vocabulary; everything else is judged as before,
    /// the create target of a plainly local `CREATE VIEW v` included — it is never resolved.
    #[test]
    fn only_a_statement_bound_for_the_server_is_left_unjudged() {
        let ctx = session();
        let asks = |sql: &str, kind| {
            let stmt =
                resolved_one(&ctx, sql).unwrap_or_else(|e| panic!("'{sql}': {}", e.message()));
            dispatched(&ctx, kind, &stmt)
        };
        for (sql, kind) in [
            (
                "CREATE TABLE pg.public.t (payload jsonb)",
                StmtKind::CreateTable,
            ),
            (
                "CREATE VIEW pg.public.v AS SELECT id FROM pg.public.orders",
                StmtKind::CreateView,
            ),
            ("DROP TABLE pg.public.orders", StmtKind::DropTable),
            ("DROP VIEW pg.public.orders", StmtKind::DropView),
            ("UPDATE pg.public.orders SET total = 1", StmtKind::Update),
            ("DELETE FROM pg.public.orders", StmtKind::Delete),
            ("DELETE FROM orders", StmtKind::Delete),
        ] {
            assert!(asks(sql, kind), "'{sql}' runs on the server");
        }
        for (sql, kind) in [
            ("CREATE TABLE local (id INT)", StmtKind::CreateTable),
            ("CREATE TABLE pg.public.t AS SELECT 1 AS n", StmtKind::Ctas),
            (
                "CREATE VIEW v AS SELECT * FROM pg.public.orders",
                StmtKind::CreateView,
            ),
            ("DROP TABLE local", StmtKind::DropTable),
            ("UPDATE local SET id = 1", StmtKind::Update),
            (
                "INSERT INTO pg.public.orders VALUES (1, 2)",
                StmtKind::Insert,
            ),
        ] {
            assert!(!asks(sql, kind), "'{sql}' is planned here");
        }
    }

    /// An `UPDATE` or a `DELETE` over a workspace table says where the statement does work,
    /// rather than that it is unsupported.
    #[test]
    fn workspace_dml_names_where_it_works() {
        let why = workspace_dml(StmtKind::Update);
        assert!(
            why.starts_with("UPDATE works on a relation in a data source"),
            "{why}"
        );
        assert!(why.contains("CREATE TABLE AS"), "{why}");
    }

    /// Byte offsets are walked in characters, so a non-ASCII comment ahead of the name does not
    /// shift the cut — and a span that could not be trusted would refuse rather than guess.
    #[test]
    fn a_non_ascii_prefix_does_not_shift_the_cut() {
        let sql = "/* café ☕ */ DROP TABLE pg.public.orders";
        assert_eq!(
            rewritten(sql).expect("spliced"),
            "/* café ☕ */ DROP TABLE public.orders"
        );
    }

    /// The folded compare the body check runs is the same one the catalog list resolves by, so a
    /// quoted spelling of the data source is still the source.
    #[test]
    fn the_sources_own_name_folds() {
        assert_eq!(fold_ident("PG"), "pg");
        assert_eq!(
            rewritten("DROP TABLE \"pg\".public.orders").expect("spliced"),
            "DROP TABLE public.orders"
        );
    }

    /// **The editor does not squiggle what it dispatches.** A column-list create in the server's
    /// own type vocabulary is the sharp case: `jsonb` has no Arrow mapping, so a dry-plan refuses
    /// the statement Run then performs.
    #[tokio::test]
    async fn a_dispatched_statement_draws_no_diagnostic() {
        let ctx = session();
        let policy = CapabilityPolicyProvider::new(Capability::full());
        let pipeline = Pipeline::new(&ctx);
        let functions = crate::sql::FunctionCatalog::default();
        for sql in [
            "CREATE TABLE pg.public.t (id INT, payload jsonb)",
            "CREATE VIEW pg.public.v AS SELECT postgres_only(id) FROM pg.public.orders",
            "UPDATE pg.public.orders SET total = 1 WHERE id = 2",
            "DELETE FROM pg.public.orders WHERE id = 2",
        ] {
            let diags = crate::sql::analyze(&pipeline, &policy, &functions, sql).await;
            assert!(diags.is_empty(), "'{sql}': {diags:?}");
        }
        let local = crate::sql::analyze(
            &pipeline,
            &policy,
            &functions,
            "CREATE TABLE mine (payload jsonb)",
        )
        .await;
        assert!(
            !local.is_empty(),
            "a workspace table is still Strata's to judge"
        );
    }

    /// **A remote drop names its readers** for the two spellings that used to strand them: a
    /// quoted identifier and a reserved word.
    ///
    /// The address it looks them up by is the plan's own rendering rather than the quoted one a
    /// message prints — `PlanDeps::remote` holds `pg.public.Orders`, and comparing it against
    /// `pg.public.\"Orders\"` matches nothing, so a drop would report a destructive action as
    /// consequence-free. [`remote_dependents`] takes the recorded reference now, which is what
    /// makes the wrong comparison unwritable rather than merely unwritten.
    ///
    /// Pinned at the lookup, which is where the bug lived; the sentence a real drop prints around
    /// it is `tests/postgres_federation.rs`'s, where a server can take the statement.
    #[tokio::test]
    async fn a_drop_names_its_readers_for_a_quoted_and_a_reserved_name() {
        let at = Remote {
            source: "pg".into(),
            reference: TableReference::full("pg", "public", "Orders"),
        };
        assert_eq!(at.recorded().to_string(), "pg.public.Orders");
        assert_eq!(at.address(), "pg.public.\"Orders\"");

        let ctx = session();
        fake_source(&ctx, "quoted", &["Orders", "order"]);
        for (view, reads) in [
            ("quoted_reader", "quoted.public.\"Orders\""),
            ("reserved_reader", "quoted.public.\"order\""),
        ] {
            crate::statements::arms::views::create(&ctx, view, &format!("SELECT id FROM {reads}"))
                .await
                .expect("a workspace view over the remote relation");
        }

        for (relation, reader) in [("Orders", "quoted_reader"), ("order", "reserved_reader")] {
            let at = Remote {
                source: "quoted".into(),
                reference: TableReference::full("quoted", "public", relation),
            };
            assert_eq!(
                remote_dependents(&ctx, at.recorded()).await,
                vec![reader.to_string()],
                "'{}' named no readers",
                at.address()
            );
        }
    }
}
