//! **Views** — the body every gesture that creates or drops one runs, and the two typed
//! statements that are a second gesture into it. `docs/STATEMENTS_SPEC.md` §6.3.
//!
//! A view is Save's artifact: ⌘S wraps the tab's plain query in `CREATE OR REPLACE VIEW` and
//! folds the answer into the store. Typed view DDL is the **same funnel entered a second way** —
//! [`create`] is what [`Engine::create_view`](crate::Engine::create_view) spawns, so a
//! view is indistinguishable by origin: one store row, one `project.json` entry, one set of deps,
//! and either gesture edits the row the other made.
//!
//! **The statement never runs natively**, for two disqualifying reasons. DataFusion's
//! `CREATE OR REPLACE VIEW` over a *table* name silently replaces the table — it deregisters
//! whatever is there and registers a `ViewTable` without asking `table_type` — so a typo would turn
//! a registered parquet table into a view while its def went on naming files nothing reads;
//! [`create_statement`] refuses a name that resolves to a base table. And the store write-back
//! needs a [`ViewMeta`], which introspecting for after the fact is the refetch the catalog
//! invariant forbids; [`create`] reads it off the freshly-registered view's own `DataFrame`.
//!
//! **The def stores `ViewDef { name, sql }` and nothing else**, so a typed statement has to arrive
//! at exactly that pair: the folded target name, and the definition **query's** canonical rendering
//! rather than the statement around it. That is what makes the row round-trip, and it is why every
//! clause `CREATE VIEW` can carry is refused by name ([`definition`]) — the statement is rebuilt
//! around the query, so a clause we did not read is a clause silently dropped.

use datafusion::logical_expr::{DdlStatement, LogicalPlan, TableType};
use datafusion::prelude::SessionContext;
use datafusion::sql::parser::Statement as DFStatement;
use datafusion::sql::sqlparser::ast::{CreateTableOptions, CreateView, Statement as SqlStatement};
use datafusion::sql::TableReference;

use strata_arrow::column_info;

use crate::catalog::{dependents_of_view, plan_deps, view_error, ViewMeta};
use crate::query::is_snapshot_name;
use crate::sources::Live;
use crate::statements::pipeline::resolved_one;
use crate::statements::{Fault, StmtKind};
use crate::{fold_ident, quote_ident};
use strata_model::ViewDef;

use super::{bare_name, elsewhere, existing, left_invalid, remote, StatementOutcome, StoreEffect};

/// What [`bare_name`] calls the objects these statements create.
const WHAT: &str = "Views";

/// Create (or redefine) the SQL view `name` over `sql`, returning its columns and what it reads
/// — the body behind both gestures.
///
/// `name` is whatever the user typed (it rides in `.strata/project.json`, a shared, committed
/// file), so it goes through [`quote_ident`] rather than straight into the statement — which is
/// the only reason a name like `Sales 2024` can be a view at all. The view's identity is then
/// [`fold_ident(name)`](fold_ident), which is what the lookup below asks for.
///
/// **Parsed and resolved rather than handed to `ctx.sql`** ([`resolved_one`]): a view's body
/// is a read like any other, and resolving it is what makes [`plan_deps`] record a body reading a
/// connection's `orders` as the *remote* dependency it is. The def still stores the SQL the user
/// wrote.
///
/// A failure comes back through [`view_error`], the table funnel's `register_error` from the
/// other side: one diagnosis — a relation a database connection no longer has — in front of the
/// same unwrapping a refused *table* gets. A view's failure lands in the same Problems list, one
/// row below its cause, so a view carrying DataFusion's wrapper stack beside a table that has had
/// it peeled would read as two faults worded by two apps. Both halves are no-ops on a message
/// they do not recognise, which is most of them.
pub async fn create(ctx: &SessionContext, name: &str, sql: &str) -> Result<ViewMeta, String> {
    if is_snapshot_name(name) {
        return Err(Fault::ReservedName.message());
    }
    let stmt = format!("CREATE OR REPLACE VIEW {} AS {sql}", quote_ident(name));
    let stmt = resolved_one(ctx, &stmt).map_err(|e| view_error(ctx, &e))?;
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| view_error(ctx, &e.to_string()))?;
    let df = ctx
        .execute_logical_plan(plan)
        .await
        .map_err(|e| view_error(ctx, &e.to_string()))?;
    let _ = df.collect().await;
    let t = ctx
        .table(TableReference::bare(fold_ident(name)))
        .await
        .map_err(|e| view_error(ctx, &e.to_string()))?;
    let deps = plan_deps(t.logical_plan());
    let columns = t.schema().fields().iter().map(|f| column_info(f)).collect();
    Ok(ViewMeta {
        columns,
        tables: deps.tables,
        remote: deps.remote,
        aliases: deps.aliases,
    })
}

/// Drop the SQL view `name` (idempotent — `IF EXISTS`). Quoted the same way [`create`] quoted it,
/// so the drop names the same view.
pub async fn drop(ctx: &SessionContext, name: &str) -> Result<(), String> {
    ctx.sql(&format!("DROP VIEW IF EXISTS {}", quote_ident(name)))
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The view a typed `CREATE VIEW` names, created — the statement half of [`create`].
///
/// Read off the **parsed statement** rather than a plan, because the def stores the definition
/// query's text and only the AST still carries it: `statement_to_plan` hands back a resolved
/// `LogicalPlan` and a `definition` string that is the whole `CREATE VIEW` rebuilt, neither of
/// which is the query on its own.
///
/// A name inside a database connection is [`remote`]'s: the view is the server's, so the
/// statement is dispatched rather than reduced to a def, and `MATERIALIZED` is accepted there and
/// nowhere else.
pub async fn create_statement(
    ctx: &SessionContext,
    stmt: DFStatement,
    sources: &Live,
    source: &str,
) -> Result<StatementOutcome, String> {
    let DFStatement::Statement(s) = &stmt else {
        return Err(not_a_view(StmtKind::CreateView));
    };
    let SqlStatement::CreateView(view) = s.as_ref() else {
        return Err(not_a_view(StmtKind::CreateView));
    };
    if let Some(at) = remote::target(ctx, StmtKind::CreateView, &stmt) {
        return remote::create_view(ctx, sources, &at, view.materialized, &stmt, source).await;
    }
    let (name, sql) = definition(ctx, view)?;

    let replacing = match existing(ctx, &name).await {
        Some(TableType::View) if !view.or_replace => {
            return Err(format!(
                "View '{name}' already exists. Use CREATE OR REPLACE VIEW"
            ))
        }
        Some(TableType::View) => true,
        Some(_) => return Err(format!("'{name}' is a table")),
        None => false,
    };

    let meta = create(ctx, &name, &sql).await?;
    let verb = match replacing {
        true => "replaced",
        false => "created",
    };
    Ok(StatementOutcome {
        message: format!("View '{name}' {verb}"),
        count: None,
        effect: Some(StoreEffect::ViewUpserted {
            def: ViewDef {
                name: name.clone(),
                sql,
            },
            meta,
        }),
    })
}

/// The view a typed `DROP VIEW` names, dropped — the statement half of [`drop`].
///
/// Planned rather than read off the AST, unlike [`create_statement`]: a drop carries no query to
/// preserve, and planning is what resolves the name and refuses the forms DataFusion does not
/// implement (a list of several objects) in its own words. Planning a `DROP` executes nothing —
/// the existence test lives in `execute_logical_plan`, which is the half we are replacing.
///
/// The remote branch is taken before that, off the AST, because a plan of a name in a database
/// connection tells this arm nothing it does not already have.
pub async fn drop_statement(
    ctx: &SessionContext,
    stmt: DFStatement,
    sources: &Live,
    source: &str,
) -> Result<StatementOutcome, String> {
    if let Some(at) = remote::target(ctx, StmtKind::DropView, &stmt) {
        return remote::drop_relation(ctx, sources, &at, true, &stmt, source).await;
    }
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Ddl(DdlStatement::DropView(dropping)) = plan else {
        return Err(format!(
            "{} did not plan as a view drop",
            StmtKind::DropView.label()
        ));
    };
    let name = bare_name(ctx, &dropping.name, WHAT)?;
    match existing(ctx, &name).await {
        Some(TableType::View) => {}
        Some(_) => return Err(format!("'{name}' is a table. Use DROP TABLE")),
        None if dropping.if_exists => {
            return Ok(StatementOutcome {
                message: format!("View '{name}' does not exist"),
                count: None,
                effect: None,
            })
        }
        None => return Err(format!("View '{name}' does not exist")),
    }
    let dependents = dependents_of_view(ctx, &name).await;

    drop(ctx, &name).await?;

    Ok(StatementOutcome {
        message: format!("View '{name}' dropped{}", left_invalid(&dependents)),
        count: None,
        effect: Some(StoreEffect::ViewRemoved { name }),
    })
}

/// The `(name, definition query)` pair a `CREATE VIEW` reduces to — everything `ViewDef` holds,
/// and the whole of what the statement is allowed to carry.
///
/// **Exhaustive over the parsed statement, with no `..`.** The statement is rebuilt around the
/// query in [`create`], so DataFusion's own clause gate never sees the user's spelling of it and
/// a clause read by nobody is a clause silently dropped — `CREATE TEMPORARY VIEW` would create a
/// permanent one. A clause sqlparser learns later is therefore a compile error here rather than a
/// promise Strata quietly breaks, which is the rule the router's wildcard-free match keeps from
/// the other end.
fn definition(ctx: &SessionContext, view: &CreateView) -> Result<(String, String), String> {
    let CreateView {
        or_alter,
        or_replace: _,
        materialized,
        secure,
        name,
        name_before_not_exists: _,
        columns,
        query,
        options,
        cluster_by,
        comment,
        with_no_schema_binding,
        if_not_exists,
        temporary,
        copy_grants,
        to,
        params,
    } = view;

    if *if_not_exists {
        return Err(
            "CREATE VIEW IF NOT EXISTS is not supported. Use CREATE OR REPLACE VIEW".into(),
        );
    }
    if !columns.is_empty() {
        return Err("A view's column list is not supported. Alias the columns in the query".into());
    }
    for (present, clause) in [
        (*or_alter, "OR ALTER"),
        (*materialized, "MATERIALIZED"),
        (*secure, "SECURE"),
        (*temporary, "TEMPORARY"),
        (*copy_grants, "COPY GRANTS"),
        (*with_no_schema_binding, "WITH NO SCHEMA BINDING"),
        (!cluster_by.is_empty(), "CLUSTER BY"),
        (comment.is_some(), "COMMENT"),
        (to.is_some(), "TO"),
        (params.is_some(), "ALGORITHM, DEFINER and SQL SECURITY"),
        (!matches!(options, CreateTableOptions::None), "view options"),
    ] {
        if present {
            return Err(format!("CREATE VIEW does not support {clause}"));
        }
    }

    if name.0.len() > 3 {
        return Err(elsewhere(WHAT));
    }
    let name = bare_name(ctx, &TableReference::parse_str(&name.to_string()), WHAT)?;
    Ok((name, query.to_string()))
}

/// The router said this was view DDL and sqlparser parses it as `CreateView`. Anything else is
/// the two disagreeing.
fn not_a_view(kind: StmtKind) -> String {
    format!("{} did not parse as a view", kind.label())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::{env, process};

    use crate::register::{register_project, RegOutcome};
    use crate::statements::Fault;
    use crate::{Engine, RunOutcome, RunTag, StatementReport, WsId};
    use strata_core::project::{save_defs, ProjectDefs};

    use super::*;

    /// Run one statement and take its report — anything else is a test that asked the wrong
    /// question.
    async fn statement(eng: &Engine, sql: &str) -> Result<StatementReport, String> {
        match eng.run(WsId(1), RunTag(1), sql.into(), 10).await? {
            RunOutcome::Statement(report) => Ok(report),
            RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
        }
    }

    /// The values a query returns, as text — a view has to be readable the way anything else is.
    async fn read(eng: &Engine, sql: &str) -> Vec<Vec<String>> {
        let RunOutcome::Rows(output, _) = eng
            .run(WsId(2), RunTag(2), sql.into(), 100)
            .await
            .expect("query")
        else {
            panic!("{sql} did not return rows");
        };
        output
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(|cell| cell.text).collect())
            .collect()
    }

    /// The `ViewUpserted` pair a report carries, or a failure naming what it carried instead.
    fn upserted(report: &StatementReport) -> (ViewDef, ViewMeta) {
        match &report.effect {
            Some(StoreEffect::ViewUpserted { def, meta }) => (def.clone(), meta.clone()),
            other => panic!("{other:?}"),
        }
    }

    /// **One funnel, two gestures.** A typed `CREATE OR REPLACE VIEW` lands exactly the pair ⌘S
    /// lands for the same query: the same def (name + the *plain* query, so the row round-trips
    /// through Save) and the same meta, columns and deps included. If these two ever came apart,
    /// a view would be editable by the gesture that made it and by nothing else.
    ///
    /// The def's SQL is asserted against a **rendering**, not the text typed: the statement is
    /// rebuilt around the parsed query node, so what lands is sqlparser's canonical form of it.
    /// The lower-case input is what makes that visible.
    #[tokio::test]
    async fn a_typed_create_lands_what_save_as_view_lands() {
        let root = scratch("create");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        statement(
            &eng,
            "CREATE TABLE t AS SELECT * FROM (VALUES (1), (2)) AS v(n)",
        )
        .await
        .expect("created");

        let report = statement(
            &eng,
            "create or replace view v as select n from t where n>0",
        )
        .await
        .expect("created");
        assert_eq!(report.message, "View 'v' created");
        assert_eq!(report.count, None, "creating a view moves no rows");
        let (def, meta) = upserted(&report);
        assert_eq!(def.name, "v");
        assert_eq!(
            def.sql, "SELECT n FROM t WHERE n > 0",
            "the definition query alone, canonically rendered, so Save can rewrite it"
        );
        assert_eq!(
            meta.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["n"]
        );
        assert_eq!(meta.tables, vec!["t".to_string()], "and what it reads");
        assert_eq!(read(&eng, "SELECT n FROM v ORDER BY n").await.len(), 2);

        let saved = eng
            .create_view(def.name.clone(), def.sql.clone())
            .await
            .expect("saved");
        assert_eq!(
            saved, meta,
            "⌘S over the typed def is the same registration"
        );

        let replaced = statement(&eng, "CREATE OR REPLACE VIEW v AS SELECT 1 AS other")
            .await
            .expect("replaced");
        assert_eq!(replaced.message, "View 'v' replaced");
        let (def, meta) = upserted(&replaced);
        assert_eq!(def.sql, "SELECT 1 AS other");
        assert_eq!(meta.columns[0].name, "other");
        let _ = fs::remove_dir_all(&root);
    }

    /// The name semantics on the one namespace tables and views share.
    ///
    /// **The table fence is the point.** DataFusion's own `CREATE OR REPLACE VIEW` over a table
    /// name deregisters the table and registers a view in its place without ever asking what was
    /// there — so this drives the statement through `Engine::run` and then reads the table back,
    /// which is the only assertion that would have failed before the fence existed.
    #[tokio::test]
    async fn a_plain_create_points_at_or_replace_and_a_table_name_is_refused() {
        let root = scratch("names");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        statement(&eng, "CREATE TABLE sales AS SELECT 1 AS n")
            .await
            .expect("created");
        statement(&eng, "CREATE VIEW v AS SELECT 1 AS n")
            .await
            .expect("created");

        assert_eq!(
            statement(&eng, "CREATE VIEW v AS SELECT 2 AS n")
                .await
                .expect_err("taken"),
            "View 'v' already exists. Use CREATE OR REPLACE VIEW"
        );
        assert_eq!(
            read(&eng, "SELECT n FROM v").await,
            vec![vec!["1"]],
            "and the refusal left it alone"
        );

        for sql in [
            "CREATE VIEW sales AS SELECT 2 AS n",
            "CREATE OR REPLACE VIEW sales AS SELECT 2 AS n",
        ] {
            assert_eq!(
                statement(&eng, sql)
                    .await
                    .expect_err("a table is not a view"),
                "'sales' is a table"
            );
        }
        assert_eq!(
            read(&eng, "SELECT n FROM sales").await,
            vec![vec!["1"]],
            "the table DataFusion would have replaced is still a table"
        );
        assert!(eng.is_internal("sales"), "…and still a write target");
        let _ = fs::remove_dir_all(&root);
    }

    /// A drop removes the row, names what it leaves invalid **without cascading**, honours
    /// `IF EXISTS`, and sends a table name to the statement that drops tables.
    #[tokio::test]
    async fn a_drop_reports_its_readers_and_honours_if_exists() {
        let root = scratch("drop");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        statement(&eng, "CREATE TABLE t AS SELECT 1 AS n")
            .await
            .expect("created");
        statement(&eng, "CREATE VIEW inner_v AS SELECT n FROM t")
            .await
            .expect("created");
        statement(&eng, "CREATE VIEW outer_v AS SELECT n FROM inner_v")
            .await
            .expect("created");

        let report = statement(&eng, "DROP VIEW inner_v").await.expect("dropped");
        assert_eq!(
            report.message,
            "View 'inner_v' dropped. 1 view is left invalid: 'outer_v'"
        );
        assert_eq!(report.count, None, "a drop moves no rows");
        assert_eq!(
            report.effect,
            Some(StoreEffect::ViewRemoved {
                name: "inner_v".into()
            })
        );
        assert!(
            eng.run(WsId(2), RunTag(2), "SELECT n FROM inner_v".into(), 10)
                .await
                .is_err(),
            "the dropped view no longer resolves"
        );
        assert_eq!(
            read(&eng, "SELECT n FROM outer_v").await,
            vec![vec!["1"]],
            "left invalid, not cascaded: its plan was inlined when it was created"
        );

        let missing = statement(&eng, "DROP VIEW IF EXISTS inner_v")
            .await
            .expect("reported");
        assert_eq!(missing.message, "View 'inner_v' does not exist");
        assert_eq!(missing.effect, None, "nothing for the store to fold");
        assert_eq!(
            statement(&eng, "DROP VIEW inner_v")
                .await
                .expect_err("gone"),
            "View 'inner_v' does not exist"
        );
        assert_eq!(
            statement(&eng, "DROP VIEW t").await.expect_err("a table"),
            "'t' is a table. Use DROP TABLE"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **The clauses a rebuilt statement would have dropped.** `create` renders
    /// `CREATE OR REPLACE VIEW <name> AS <query>` around the parsed query, so DataFusion's own
    /// clause gate never sees what the user wrote — anything not refused here would be accepted
    /// and then silently ignored, and `TEMPORARY` is the one that matters: it would create a
    /// permanent view out of a statement asking for a session-scoped one.
    #[tokio::test]
    async fn unsupported_clauses_refuse_before_the_view_exists() {
        let eng = Engine::builder().build();

        for (sql, message) in [
            (
                "CREATE TEMPORARY VIEW v AS SELECT 1 AS n",
                "CREATE VIEW does not support TEMPORARY",
            ),
            (
                "CREATE MATERIALIZED VIEW v AS SELECT 1 AS n",
                "CREATE VIEW does not support MATERIALIZED",
            ),
            (
                "CREATE VIEW IF NOT EXISTS v AS SELECT 1 AS n",
                "CREATE VIEW IF NOT EXISTS is not supported. Use CREATE OR REPLACE VIEW",
            ),
            (
                "CREATE VIEW v (a) AS SELECT 1 AS n",
                "A view's column list is not supported. Alias the columns in the query",
            ),
        ] {
            assert_eq!(statement(&eng, sql).await.expect_err("refused"), message);
        }
        assert!(
            read(
                &eng,
                "SELECT table_name FROM information_schema.tables WHERE table_name = 'v'"
            )
            .await
            .is_empty(),
            "a refusal creates nothing"
        );
    }

    /// **The reader set is raw, and deliberately biased to over-report.** `PlanDeps::aliases`
    /// cannot tell an inlined view from a local table alias or a CTE of the same name, so a view
    /// that merely writes `FROM t AS v` is named by a drop of the view `v`. That is the safe
    /// direction and the *same* answer the catalog pane's confirm gives: the store's own filter
    /// (`ProjectState::view_registered`) keeps an alias only where a view row of that name
    /// exists, which is always true of the name being dropped — so it cannot subtract this case,
    /// and the two surfaces still say one thing. Under-reporting is what would matter, and the
    /// first half of this test is what pins it.
    #[tokio::test]
    async fn a_drops_readers_over_report_rather_than_miss_one() {
        let root = scratch("aliases");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        statement(&eng, "CREATE TABLE t AS SELECT 1 AS n")
            .await
            .expect("created");
        statement(&eng, "CREATE VIEW v AS SELECT n FROM t")
            .await
            .expect("created");
        statement(&eng, "CREATE VIEW reader AS SELECT n FROM v")
            .await
            .expect("created");
        statement(&eng, "CREATE VIEW impostor AS SELECT n FROM t AS v")
            .await
            .expect("created");

        let report = statement(&eng, "DROP VIEW v").await.expect("dropped");
        assert!(
            report.message.contains("'reader'"),
            "the reader that matters is never missed: {}",
            report.message
        );
        assert!(
            report.message.contains("'impostor'"),
            "and the alias is named too, which is the raw set's known cost: {}",
            report.message
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **A qualified name names one place or nowhere.** Strata has one catalog and one schema, so
    /// `strata.public.v` is a longer spelling of `v` and anything else is refused rather than
    /// silently created here.
    ///
    /// The four-part case is the one that needs the test: `TableReference::parse_str` cannot
    /// represent it, and its fallback is a **bare** reference holding the whole dotted string — so
    /// without the part-count check `CREATE VIEW a.b.c.d` created a view literally named `a.b.c.d`,
    /// which `DROP VIEW a.b.c.d` then could not drop (DataFusion's planner refuses that name).
    #[tokio::test]
    async fn a_name_outside_the_one_schema_is_refused_however_it_is_spelled() {
        let eng = Engine::builder().build();
        let elsewhere = elsewhere(WHAT);

        statement(&eng, "CREATE VIEW strata.public.qualified AS SELECT 1 AS n")
            .await
            .expect("the long spelling of the one schema");
        assert_eq!(read(&eng, "SELECT n FROM qualified").await, vec![vec!["1"]]);

        for sql in [
            "CREATE VIEW other.v AS SELECT 1 AS n",
            "CREATE VIEW other.public.v AS SELECT 1 AS n",
            "CREATE VIEW a.b.c.d AS SELECT 1 AS n",
        ] {
            assert_eq!(statement(&eng, sql).await.expect_err("refused"), elsewhere);
        }
        assert!(
            read(&eng, "SELECT table_name FROM information_schema.tables")
                .await
                .iter()
                .all(|row| row[0] != "a.b.c.d"),
            "and nothing was created under the dotted name"
        );
    }

    /// The reserved namespace, from the router — a `__snap_` view name would collide with a live
    /// snapshot registration, which the provider answers "already exists" to, on a name the same
    /// prefix hides from every catalog reader.
    #[tokio::test]
    async fn a_reserved_view_name_is_refused() {
        let eng = Engine::builder().build();
        assert_eq!(
            statement(&eng, "CREATE VIEW __snap_1 AS SELECT 1 AS n")
                .await
                .expect_err("refused"),
            Fault::ReservedName.message()
        );
    }

    /// **Replay is the ordinary pass.** The defs a typed view chain produces go into
    /// `project.json` and a cold engine registers them back with no code of its own — the view
    /// over a view included, which only works because `register_pass`'s fixed point creates what
    /// it can each round. Nothing here is view-DDL-specific, which is the claim: a typed view is
    /// a `ViewDef` and nothing else.
    #[tokio::test]
    async fn a_typed_view_chain_survives_a_restart() {
        let root = scratch("replay");
        let defs = {
            let eng = Engine::builder().build();
            eng.set_data_dir(&root);
            let table = statement(&eng, "CREATE TABLE t AS SELECT 1 AS n")
                .await
                .expect("created");
            let Some(StoreEffect::TableUpserted { def, .. }) = table.effect else {
                panic!("{:?}", table.effect);
            };
            let base = statement(&eng, "CREATE VIEW base AS SELECT n FROM t")
                .await
                .expect("created");
            let over = statement(&eng, "CREATE VIEW over_base AS SELECT n FROM base")
                .await
                .expect("created");
            ProjectDefs {
                tables: vec![def],
                views: vec![upserted(&over).0, upserted(&base).0],
                ..Default::default()
            }
        };
        save_defs(&root, &defs).expect("written");

        let cold = Engine::builder().build();
        let mut out = Vec::new();
        register_project(&cold, &root, &defs, |o| out.push(o)).await;

        let failed: Vec<&RegOutcome> = out
            .iter()
            .filter(|o| match o {
                RegOutcome::Table { result, .. } => result.is_err(),
                RegOutcome::View { result, .. } => result.is_err(),
                RegOutcome::Connection { result, .. } => result.is_err(),
            })
            .collect();
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(
            read(&cold, "SELECT n FROM over_base").await,
            vec![vec!["1"]],
            "and the chain reads through both views"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A scratch project folder of our own, per test — the tag is load-bearing because these run
    /// concurrently in one process. Only the tests that create a *table* need one; a view is
    /// nothing but a def.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_views_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        save_defs(&dir, &ProjectDefs::default()).unwrap();
        dir
    }
}
