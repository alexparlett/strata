//! **Bare-name resolution across the connected databases** — what turns
//! `SELECT * FROM orders` into `SELECT * FROM "pg"."public"."orders"` before anything plans it.
//!
//! DataFusion has one default catalog and one default schema and no search path, so a bare name
//! resolves to the workspace or to nothing. Moving the default is the other design, and it was
//! rejected: `providers::in_workspace` answers `true` for every bare name, and four rules turn on
//! that answer — the `__snap_` fence, what a write may target, and the two halves of a view's
//! recorded dependencies. A moved default makes all four wrong at once, and the fix has to reach
//! every reader.
//!
//! Resolving on the **statement** instead leaves all four untouched, because the plan then carries
//! the name the read actually reached: `PlanDeps` records `pg.public.orders` in its remote half
//! for free, and a bare name that is still bare after this pass is the workspace's, exactly as
//! before. There is no mode, nothing to display and nothing a restart has to clear.
//!
//! The rule, in one line: **a bare name is the workspace's wherever a statement can *make* one,
//! and resolved across sources everywhere else.**
//!
//! 1. The workspace wins. Asked of [`SchemaProvider::table_exist`], which sees tables, views and
//!    the snapshot spool — so `__snap_3` is the workspace's whether or not a run has minted it.
//! 2. Exactly one relation of that name across the connected catalogs: the name is rewritten
//!    whole. Views and materialized views included — the search asks the providers, and a
//!    database connection's listing is `relkind IN ('r','p','v','m','f')`.
//! 3. More than one: [`Refusal::ambiguous`], naming every candidate. Never a coin flip between
//!    two servers.
//! 4. None: left bare, which is the error DataFusion already gives.
//!
//! **Resolvable positions: everything but a create target.** A target that addresses a relation
//! which already exists resolves like a read, so `INSERT INTO orders`, `DROP TABLE orders`,
//! `UPDATE orders` and `DELETE FROM orders` all reach the relation `SELECT * FROM orders` reads.
//! Three things make that safe with no second gate here: a connection is **read-only by default**
//! and the user opted this one in, an ambiguous name still refuses by name so a write never picks
//! between two servers, and the arm is reached with a qualified name — so `ddl::remote_target`
//! answers identically whether or not the qualifier was typed.
//!
//! **A create target is never resolved**, permanently, and it is the one carve-out.
//! `CREATE TABLE orders` names a relation that does not exist, so there is nothing to resolve
//! *to*, and resolving would read a plainly local intent as "make it on the server" — which then
//! fails as already existing. `CREATE TABLE pg.public.orders AS SELECT …` is how the server is
//! addressed, by typing the qualifier.

use std::collections::HashSet;
use std::ops::ControlFlow;

use datafusion::prelude::SessionContext;
use datafusion::sql::parser::{CopyToSource, Statement as DFStatement};
use datafusion::sql::sqlparser::ast::{
    visit_relations_mut, Ident, ObjectName, ObjectNamePart, Query, Statement as SqlStatement,
    TableObject, Visit, VisitMut, Visitor,
};
use datafusion::sql::sqlparser::tokenizer::Span;

use crate::fold_ident;
use crate::providers::shown_schemas;
use crate::sql::qualified;

/// A bare name this pass refused rather than resolved, spanned into the statement it was read
/// from so the editor can squiggle the name itself.
pub(crate) struct Refusal {
    pub message: String,
    pub span: Span,
}

impl Refusal {
    /// One name, several relations. The fix is the user's to make and the message says so: every
    /// candidate is printed in the spelling that reaches it.
    fn ambiguous(name: &Ident, candidates: &[Qualified]) -> Self {
        let list: Vec<String> = candidates.iter().map(Qualified::rendered).collect();
        Refusal {
            message: format!(
                "'{}' is ambiguous: {}. Qualify it",
                name.value,
                list.join(", ")
            ),
            span: name.span,
        }
    }
}

/// Resolve every bare name in `stmt` that is not a create target against the connected databases,
/// in place.
///
/// Runs per statement on every re-validation, so it reads the session under its own lock rather
/// than through `SessionContext::state` (which deep-clones every function registry), and collects
/// the table-function names only once a database is registered to resolve into.
pub(crate) fn qualify(ctx: &SessionContext, stmt: &mut DFStatement) -> Vec<Refusal> {
    let names = Names::of(ctx);
    if names.databases.is_empty() {
        return Vec::new();
    }
    let mut pass = Pass {
        names,
        functions: table_functions(ctx),
        refusals: Vec::new(),
    };
    pass.statement(stmt);
    pass.refusals
}

/// Every registered catalog that is not [`Home`]'s, in its registered spelling.
fn databases(ctx: &SessionContext, home: &Home) -> Vec<String> {
    let folded = fold_ident(&home.catalog);
    ctx.catalog_names()
        .into_iter()
        .filter(|name| fold_ident(name) != folded)
        .collect()
}

/// The session's registered table function names, folded. `FROM range(1, 10)` parses as a
/// relation, and a server with a relation of that name would otherwise capture the call.
fn table_functions(ctx: &SessionContext) -> HashSet<String> {
    let state = ctx.state_ref();
    let state = state.read();
    state
        .table_functions()
        .keys()
        .map(|name| fold_ident(name))
        .collect()
}

/// One relation, addressed the way the thing that holds it spells it: the catalog as the
/// connection registered it, the schema and the relation as the server does.
struct Qualified {
    catalog: String,
    schema: String,
    table: String,
}

impl Qualified {
    /// The name written back into the statement, **every part quoted** — the only rendering that
    /// means the same thing under either `enable_ident_normalization`, which would otherwise
    /// lower-case a server's `Orders`. [`rendered`](Self::rendered) is what a message uses.
    ///
    /// Every part carries the **bare name's** span, because the name does have a place in the
    /// buffer and a statement dispatched to a server is spliced out of it; the synthesized node's
    /// own [`Span::empty`] would say there is none.
    fn object_name(&self, span: Span) -> ObjectName {
        ObjectName(
            [&self.catalog, &self.schema, &self.table]
                .into_iter()
                .map(|part| ObjectNamePart::Identifier(Ident::with_quote_and_span('"', span, part)))
                .collect(),
        )
    }

    /// The address as a message prints it — `qualified`, because these three parts are a server's
    /// spelling and quoting them whole would name one relation with dots in it.
    fn rendered(&self) -> String {
        format!(
            "'{}'",
            qualified([
                self.catalog.as_str(),
                self.schema.as_str(),
                self.table.as_str()
            ])
        )
    }
}

/// Where a bare name already resolves — this session's `datafusion.catalog.default_catalog` and
/// `default_schema`.
///
/// Read from the config rather than the crate's `CATALOG`/`SCHEMA`, because the question is
/// "would the planner have found it" and the planner asks the config — so a context built any
/// other way cannot have its own default read as a database connection.
struct Home {
    catalog: String,
    schema: String,
}

impl Home {
    fn of(ctx: &SessionContext) -> Self {
        let state = ctx.state_ref();
        let state = state.read();
        let catalog = &state.config_options().catalog;
        Home {
            catalog: catalog.default_catalog.clone(),
            schema: catalog.default_schema.clone(),
        }
    }
}

/// **What a bare name resolves to**, built once and asked many times — the read half of the pass,
/// which the keyword-typo lint borrows so it can agree with the resolver about what is a known
/// name without rewriting anything.
///
/// Built once per caller on purpose: the lint asks per identifier token on every keystroke, and
/// building this is a lock, a config read and a `Vec` per catalog.
pub(crate) struct Names<'a> {
    ctx: &'a SessionContext,
    home: Home,
    /// Every registered catalog that is not [`Home`]'s, in its registered spelling.
    databases: Vec<String>,
}

impl<'a> Names<'a> {
    pub(crate) fn of(ctx: &'a SessionContext) -> Self {
        let home = Home::of(ctx);
        Names {
            databases: databases(ctx, &home),
            home,
            ctx,
        }
    }

    /// Whether a **bare** `name` names a relation at all — here, or in a connected database. An
    /// ambiguous name answers `true`: it names relations, and the statement pass has the better
    /// sentence for what is wrong with it.
    pub(crate) fn resolves(&self, name: &str) -> bool {
        self.home_has(name) || self.candidates(name).is_some()
    }

    /// Where a bare name resolves outside the workspace — `None` when the workspace has it (it
    /// wins) or when nothing does.
    ///
    /// **Scoped to the schemas each connection shows** ([`shown_schemas`]): a schema switched
    /// off neither captures a bare name nor collides with one in a schema left on, where a name
    /// written in full still resolves into any of them. `table_exist` throughout, so only a hit
    /// pays for `table_names` — and only to recover the server's spelling.
    fn candidates(&self, name: &str) -> Option<Vec<Qualified>> {
        if self.home_has(name) {
            return None;
        }
        let folded = fold_ident(name);
        let mut found = Vec::new();
        for catalog in &self.databases {
            let Some(provider) = self.ctx.catalog(catalog) else {
                continue;
            };
            let shown = shown_schemas(provider.as_ref());
            for schema in provider.schema_names() {
                if shown
                    .as_ref()
                    .is_some_and(|shown| !shown.contains(&fold_ident(&schema)))
                {
                    continue;
                }
                let Some(relations) = provider.schema(&schema) else {
                    continue;
                };
                if !relations.table_exist(name) {
                    continue;
                }
                let Some(table) = relations
                    .table_names()
                    .into_iter()
                    .find(|listed| fold_ident(listed) == folded)
                else {
                    continue;
                };
                found.push(Qualified {
                    catalog: catalog.clone(),
                    schema,
                    table,
                });
            }
        }
        (!found.is_empty()).then_some(found)
    }

    /// Whether the bare name already resolves where it resolves today — the workspace's one
    /// schema, holding its tables, its views and the snapshot spool, which is what keeps
    /// `__snap_` names inside the fence that reserves them.
    fn home_has(&self, name: &str) -> bool {
        self.ctx
            .catalog(&self.home.catalog)
            .and_then(|catalog| catalog.schema(&self.home.schema))
            .is_some_and(|schema| schema.table_exist(name))
    }
}

/// One run of the pass over one statement.
struct Pass<'a> {
    names: Names<'a>,
    /// See [`table_functions`].
    functions: HashSet<String>,
    refusals: Vec<Refusal>,
}

impl Pass<'_> {
    /// The read positions of a DataFusion statement. `CREATE EXTERNAL TABLE` has none — its
    /// `LOCATION` is a path, never a relation.
    fn statement(&mut self, stmt: &mut DFStatement) {
        match stmt {
            DFStatement::Statement(inner) => self.sql_statement(inner),
            DFStatement::CopyTo(copy) => match &mut copy.source {
                CopyToSource::Relation(name) => self.read(name, &HashSet::new()),
                CopyToSource::Query(query) => self.query(query),
            },
            DFStatement::Explain(explain) => self.statement(&mut explain.statement),
            DFStatement::CreateExternalTable(_) | DFStatement::Reset(_) => {}
        }
    }

    /// The read positions of a sqlparser statement, each named. The catch-all is the *narrow*
    /// direction: an unnamed kind keeps today's meaning, where a bare name is the workspace's.
    fn sql_statement(&mut self, stmt: &mut SqlStatement) {
        match stmt {
            SqlStatement::Query(query) => self.query(query),
            SqlStatement::Explain { statement, .. } => self.sql_statement(statement),
            SqlStatement::Prepare { statement, .. } => self.sql_statement(statement),
            SqlStatement::ExplainTable { table_name, .. } => self.read(table_name, &HashSet::new()),
            SqlStatement::Insert(insert) => {
                if let TableObject::TableName(name) = &mut insert.table {
                    self.read(name, &HashSet::new());
                }
                if let Some(source) = &mut insert.source {
                    self.query(source);
                }
            }
            SqlStatement::CreateTable(create) => {
                if let Some(query) = &mut create.query {
                    self.query(query);
                }
            }
            SqlStatement::CreateView(view) => self.query(&mut view.query),
            SqlStatement::Update(_) | SqlStatement::Delete(_) => self.relations(stmt),
            SqlStatement::Drop { names, .. } => {
                for name in names {
                    self.read(name, &HashSet::new());
                }
            }
            _ => {}
        }
    }

    /// Every relation `query` reads, its own CTE names held back.
    fn query(&mut self, query: &mut Query) {
        self.relations(query);
    }

    /// Every relation `node` names, its own CTE names and nested create targets held back.
    ///
    /// Whole-statement for an `UPDATE` and a `DELETE`, because everything in them is a read
    /// position: the target addresses a relation that already exists, and the
    /// `SET`/`WHERE`/`FROM`/`USING` clauses are ordinary reads. `MySQL`'s multi-table
    /// `Delete::tables` carries no `visit_relation` annotation and is left bare here, which
    /// `ddl::remote`'s body check reads explicitly.
    fn relations<N: Visit + VisitMut>(&mut self, node: &mut N) {
        let mut held = HeldBack::default();
        let _ = Visit::visit(node, &mut held);
        let mut refusals = Vec::new();
        let _ = visit_relations_mut(node, |name: &mut ObjectName| {
            refusals.extend(self.resolve(name, &held.0));
            ControlFlow::<()>::Continue(())
        });
        self.refusals.extend(refusals);
    }

    /// One relation in a read position: rewritten when it resolves to exactly one, refused when
    /// it resolves to several, left alone otherwise.
    fn read(&mut self, name: &mut ObjectName, ctes: &HashSet<String>) {
        if let Some(refusal) = self.resolve(name, ctes) {
            self.refusals.push(refusal);
        }
    }

    fn resolve(&self, name: &mut ObjectName, ctes: &HashSet<String>) -> Option<Refusal> {
        let bare = single(name)?.clone();
        let folded = fold_ident(&bare.value);
        if ctes.contains(&folded) || self.functions.contains(&folded) {
            return None;
        }
        match self.names.candidates(&bare.value)?.as_slice() {
            [one] => {
                *name = one.object_name(bare.span);
                None
            }
            many => Some(Refusal::ambiguous(&bare, many)),
        }
    }
}

/// The single identifier a one-part name is made of — `None` for a name already qualified, and
/// for the dialect-specific form whose part is a function call.
fn single(name: &ObjectName) -> Option<&Ident> {
    match name.0.as_slice() {
        [ObjectNamePart::Identifier(ident)] => Some(ident),
        _ => None,
    }
}

/// The names a read position must **not** resolve, folded: every CTE alias, and every **create**
/// target of a statement nested in the query.
///
/// The CTE half is deliberately flat — over-collecting leaves a name bare, which is what it is
/// today, where a miss would rewrite a reference to a CTE into a table on a server. The target
/// half is why this is a visitor: `WITH x AS (…) CREATE TABLE t AS …` parses as a **query** whose
/// body is the create, so a create target sits inside what [`Pass::query`] treats as pure read.
/// An `INSERT`'s target is deliberately *not* held back — it resolves like a read.
#[derive(Default)]
struct HeldBack(HashSet<String>);

impl HeldBack {
    fn hold(&mut self, name: &ObjectName) {
        if let Some(ident) = single(name) {
            self.0.insert(fold_ident(&ident.value));
        }
    }
}

impl Visitor for HeldBack {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.0.insert(fold_ident(&cte.alias.name.value));
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_statement(&mut self, stmt: &SqlStatement) -> ControlFlow<()> {
        match stmt {
            SqlStatement::CreateTable(create) => self.hold(&create.name),
            SqlStatement::CreateView(view) => self.hold(&view.name),
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use datafusion::arrow::array::Int32Array;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;

    use super::SessionContext;
    use crate::builder::test_context;
    use crate::providers::fake_source;
    use crate::statements::pipeline::resolved_one;
    use crate::{Engine, RunTag, WsId};

    /// A session with one workspace table (`events`) and whichever database connections the test
    /// names, each holding the relations it lists.
    fn session(databases: &[(&str, &[&str])]) -> SessionContext {
        let ctx = test_context(&BTreeMap::new());
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)])),
            vec![Arc::new(Int32Array::from(vec![1]))],
        )
        .expect("batch");
        ctx.register_batch("events", batch).expect("table");
        for (catalog, relations) in databases {
            fake_source(&ctx, catalog, relations);
        }
        ctx
    }

    /// `sql` through the one funnel every surface enters, rendered back — the statement as the
    /// planner will receive it.
    fn resolved(ctx: &SessionContext, sql: &str) -> Result<String, String> {
        resolved_one(ctx, sql).map(|stmt| stmt.to_string())
    }

    /// The point of the task: a name only a database connection has is reached without the
    /// qualifier, and the statement that goes on to plan says which relation that was.
    #[test]
    fn a_name_only_the_database_has_is_qualified() {
        let ctx = session(&[("pg", &["orders"])]);
        assert_eq!(
            resolved(&ctx, "SELECT * FROM orders").expect("resolves"),
            r#"SELECT * FROM "pg"."public"."orders""#
        );
    }

    /// **The server's spelling is what lands, quoted** — the reason every part is quoted rather
    /// than written bare. An unquoted `Orders` is lower-cased by the planner under
    /// `enable_ident_normalization`, so the statement would name a relation by a spelling the
    /// plan then carries into `PlanDeps` and every message built off it.
    #[test]
    fn the_rewrite_carries_the_spelling_the_server_uses() {
        let ctx = session(&[("pg", &["Orders"])]);
        assert_eq!(
            resolved(&ctx, r#"SELECT * FROM "Orders""#).expect("resolves"),
            r#"SELECT * FROM "pg"."public"."Orders""#
        );
    }

    /// The workspace wins, silently — which is today's behaviour kept, and the reason a clash
    /// with the project's own tables needs no refusal.
    #[test]
    fn the_workspace_wins_over_a_database_of_the_same_name() {
        let ctx = session(&[("pg", &["events"])]);
        assert_eq!(
            resolved(&ctx, "SELECT * FROM events").expect("resolves"),
            "SELECT * FROM events"
        );
    }

    /// Two servers, one name: refused by name rather than resolved to whichever catalog sorts
    /// first, and the message is the whole fix.
    #[test]
    fn a_name_two_databases_have_is_refused_naming_both() {
        let ctx = session(&[("pg", &["orders"]), ("warehouse", &["orders"])]);
        let err = resolved(&ctx, "SELECT * FROM orders").expect_err("ambiguous");
        assert!(err.contains("pg.public.orders"), "{err}");
        assert!(err.contains("warehouse.public.orders"), "{err}");
        assert!(err.contains("Qualify it"), "{err}");
    }

    /// A CTE is a name the statement binds for itself. Qualifying one would rewrite a reference
    /// to the query's own result into a scan of a server.
    #[test]
    fn a_cte_name_is_left_alone() {
        let ctx = session(&[("pg", &["orders"])]);
        let sql = "WITH orders AS (SELECT 1 AS n) SELECT * FROM orders";
        assert_eq!(resolved(&ctx, sql).expect("resolves"), sql);
    }

    /// A create target is never rewritten, and the body of the same statement is — the whole
    /// read/write split in one statement.
    #[test]
    fn a_create_target_stays_the_workspaces_while_its_body_resolves() {
        let ctx = session(&[("pg", &["orders"])]);
        assert_eq!(
            resolved(&ctx, "CREATE TABLE orders AS SELECT * FROM orders").expect("resolves"),
            r#"CREATE TABLE orders AS SELECT * FROM "pg"."public"."orders""#
        );
    }

    /// **A write target resolves exactly as a read does**, so `INSERT INTO orders`
    /// dispatches to the relation `SELECT * FROM orders` reads. What is refused about it — a
    /// read-only connection — is the arm's, reached with the qualified name this produced.
    #[test]
    fn a_write_target_resolves_like_a_read() {
        let ctx = session(&[("pg", &["orders"])]);
        assert_eq!(
            resolved(&ctx, "INSERT INTO orders VALUES (1)").expect("resolves"),
            r#"INSERT INTO "pg"."public"."orders" VALUES (1)"#
        );
    }

    /// `WITH … INSERT` parses as a *query* whose body is the insert, so the write target sits
    /// inside a read position — and is resolved there too, which is the same rule seen from the
    /// awkward side rather than an exception to it.
    #[test]
    fn a_write_target_under_a_with_resolves_too() {
        let ctx = session(&[("pg", &["orders"])]);
        let sql = "WITH n AS (SELECT 1 AS id) INSERT INTO orders SELECT id FROM n";
        assert_eq!(
            resolved(&ctx, sql).expect("resolves"),
            r#"WITH n AS (SELECT 1 AS id) INSERT INTO "pg"."public"."orders" SELECT id FROM n"#
        );
    }

    /// A write whose target two servers have is still refused by name: resolving a write like a
    /// read never means picking one of them.
    #[test]
    fn an_ambiguous_write_target_is_still_refused() {
        let ctx = session(&[("pg", &["orders"]), ("warehouse", &["orders"])]);
        let err = resolved(&ctx, "INSERT INTO orders VALUES (1)").expect_err("ambiguous");
        assert!(err.contains("pg.public.orders"), "{err}");
        assert!(err.contains("warehouse.public.orders"), "{err}");
    }

    /// The `__snap_` namespace is the **workspace catalog's**, so a live snapshot is still the
    /// workspace's by this pass and still reserved by the router.
    #[tokio::test]
    async fn a_live_snapshot_is_not_resolved_into_a_database() {
        let eng = Engine::builder().build();
        eng.ws(WsId(1))
            .query(RunTag(1), "SELECT 1 AS n".into(), 10)
            .await
            .expect("run");
        fake_source(&eng.ctx, "pg", &["__snap_1"]);
        assert_eq!(
            resolved(&eng.ctx, "SELECT * FROM __snap_1").expect("resolves"),
            "SELECT * FROM __snap_1"
        );
    }

    /// And the other half of that rule: the prefix reserves nothing inside a database
    /// connection, so a relation a server happens to call `__snap_9` is an ordinary read.
    #[test]
    fn the_snapshot_prefix_reserves_nothing_on_a_server() {
        let ctx = session(&[("pg", &["__snap_9"])]);
        assert_eq!(
            resolved(&ctx, "SELECT * FROM __snap_9").expect("resolves"),
            r#"SELECT * FROM "pg"."public"."__snap_9""#
        );
    }

    /// A project with no database connection pays nothing and reads exactly as it did.
    #[test]
    fn a_project_with_no_connection_is_untouched() {
        let ctx = session(&[]);
        assert_eq!(
            resolved(&ctx, "SELECT * FROM orders").expect("parses"),
            "SELECT * FROM orders"
        );
    }
}
