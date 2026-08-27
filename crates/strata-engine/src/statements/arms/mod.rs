//! Statement **execution** — what the pipeline admits as a statement rather than a query
//! (`docs/STATEMENTS_SPEC.md` §4 + §7).
//!
//! [`Workspace::run`](crate::Workspace::run) classifies once, in front of dispatch; a
//! statement the editor implements itself lands here as its [`StmtKind`], and comes back as a
//! [`StatementReport`] — what to say, how many rows it moved, and the
//! [`StoreEffect`](crate::StoreEffect) the app
//! folds into `ProjectState`. Nothing here returns rows and nothing here touches the snapshot
//! lifecycle: DDL never retires a snapshot (`docs/SNAPSHOT_SPEC.md` §4), so a tab that creates a
//! table can still page the result it had.
//!
//! **One contract, for every arm.** `async fn(&StmtCtx, &Principal, &Qualified)
//! -> Result<StatementOutcome, String>` — the engine minus everything an arm may not touch, who
//! is asking, and the admitted statement. An arm that grows a need reaches for a member of
//! [`StmtCtx`] rather than for a signature of its own, so [`execute`]'s match stays one line per
//! kind and a new kind cannot arrive with a call shape nobody else has.
//!
//! **The store learns from the returned value, never by introspection.** That is the whole
//! reason lifecycle is intercepted rather than left to DataFusion's provider traits (spec §3):
//! `SchemaProvider::register_table` cannot say who called it or await anything, so an accreted
//! native-DDL state would have to be *read back* — the `FetchCatalog` refetch the catalog
//! invariant forbids — or pushed out through a channel, which is the message-passing
//! architecture the direct-call facade deleted.
//!
//! **Every arm is one call into a funnel that already exists.** Typed `CREATE VIEW` runs
//! `views::create` — the body [`Catalog::create_view`](crate::Catalog::create_view) runs
//! for ⌘S; typed `CREATE EXTERNAL TABLE` and a CTAS's spooled output are both
//! `catalog::register_external`. Every kind has a real arm, so the `match` below is exhaustive on
//! `StmtKind` with no stub refusal in it.

pub(crate) mod copy;
pub(crate) mod external;
pub(crate) mod functions;
pub(crate) mod remote;
pub(crate) mod session;
pub(crate) mod tables;
pub(crate) mod views;

use std::sync::Arc;
use std::time::Instant;

use datafusion::catalog::TableProvider;
use datafusion::logical_expr::TableType;
use datafusion::prelude::SessionContext;
use datafusion::sql::TableReference;

use crate::fold_ident;
use crate::policy::Principal;
use crate::statements::ctx::StmtCtx;
use crate::statements::pipeline::Qualified;
use crate::statements::report::{StatementOutcome, StatementReport};
use crate::statements::StmtKind;
use strata_core::util::plural;

pub(crate) use functions::StrataFunctionFactory;
pub(crate) use remote::dispatched;
pub(crate) use session::refuse_reserved_key;
pub use session::SessionScope;
pub(crate) use tables::column_type;
pub use tables::drop_intent;
pub(crate) use tables::drop_table;
pub use tables::duplicate_column;
pub(crate) use views::{create as create_view, drop as drop_view};

/// Execute one intercepted statement and report what it did.
///
/// The timer and the kind are stamped here rather than in the arms, so a report can never
/// disagree with the statement that produced it — and the same stamping serves the direct
/// catalog gestures, which reach an arm's body without a statement to classify (`stamped`).
///
/// Wildcard-free on [`StmtKind`], so a kind the engine gains is a compile error here rather than
/// a statement the router admits and nothing performs.
pub async fn execute(
    kind: StmtKind,
    stmt: Qualified,
    who: &Principal,
    cx: StmtCtx,
) -> Result<StatementReport, String> {
    let start = Instant::now();
    let outcome = match kind {
        StmtKind::CreateTable => tables::create_table(&cx, who, &stmt).await,
        StmtKind::Ctas => tables::create_as(&cx, who, &stmt).await,
        StmtKind::Insert => tables::insert(&cx, who, &stmt).await,
        StmtKind::DropTable => tables::drop_statement(&cx, who, &stmt).await,
        StmtKind::CreateView => views::create_statement(&cx, who, &stmt).await,
        StmtKind::DropView => views::drop_statement(&cx, who, &stmt).await,
        StmtKind::Update => remote::update(&cx, who, &stmt).await,
        StmtKind::Delete => remote::delete(&cx, who, &stmt).await,
        StmtKind::Copy => copy::copy_to(&cx, who, &stmt).await,
        StmtKind::Set => session::set(&cx, who, &stmt).await,
        StmtKind::Reset => session::reset(&cx, who, &stmt).await,
        StmtKind::Prepare => session::prepare(&cx, who, &stmt).await,
        StmtKind::Deallocate => session::deallocate(&cx, who, &stmt).await,
        StmtKind::CreateFunction => functions::create(&cx, who, &stmt).await,
        StmtKind::DropFunction => functions::drop(&cx, who, &stmt).await,
        StmtKind::CreateExternalTable => external::create(&cx, who, &stmt).await,
    }?;
    Ok(stamped(kind, start, outcome))
}

/// [`execute`]'s stamp, for the gestures that reach an arm's body directly — the catalog pane's
/// drop of a table or a view, which has no statement to classify but owes the app the same
/// answer shape as the typed statement beside it.
///
/// One report shape for both gestures, so a surface that folds one folds the other.
pub(crate) fn stamped(
    kind: StmtKind,
    start: Instant,
    outcome: StatementOutcome,
) -> StatementReport {
    StatementReport {
        kind,
        message: outcome.message,
        count: outcome.count,
        elapsed_ms: start.elapsed().as_millis(),
        effect: outcome.effect,
    }
}

/// What `name` resolves to in the engine's one schema, and what kind it is — `None` when the
/// name is free. The one existence question every arm asks, because tables and views share that
/// namespace and a create has to know which of them it is standing on.
///
/// Through `table_provider`, not `table`: the latter builds a `DataFrame`, which for a view means
/// planning its whole body just to ask whether the name is taken. Addressed as a **bare, folded**
/// reference for the reason [`Catalog::create_view`](crate::Catalog::create_view) gives —
/// `impl Into<TableReference> for &str` parses, and a name that needed quoting does not survive a
/// parse, so it would be looked up under a name nothing ever registered.
pub(crate) async fn existing(ctx: &SessionContext, name: &str) -> Option<TableType> {
    let provider: Arc<dyn TableProvider> = ctx
        .table_provider(TableReference::bare(fold_ident(name)))
        .await
        .ok()?;
    Some(provider.table_type())
}

/// What a drop leaves behind, appended to its report — empty when it leaves nothing.
///
/// One wording for both drops, because "left invalid" is one fact: a dependent's plan was inlined
/// when it was created and goes on executing until reload, so nothing is stale and nothing is
/// cascaded. Shared so a table drop and a view drop cannot describe the same consequence two ways.
pub(super) fn left_invalid(dependents: &[String]) -> String {
    if dependents.is_empty() {
        return String::new();
    }
    let names: Vec<String> = dependents.iter().map(|v| format!("'{v}'")).collect();
    let verb = match dependents.len() {
        1 => "is",
        _ => "are",
    };
    format!(
        ". {} {verb} left invalid: {}",
        plural(dependents.len(), "view"),
        names.join(", ")
    )
}

/// **The statement policy over a database connection's catalog** — one test module for
/// one rule, because the rule is cross-arm: [`resolve_target`](crate::statements::resolve_target)
/// is the single choke point in front of every intercepted statement that manages a target, so
/// what is under test is that *no arm gets there another way*.
///
/// The sixteen [`StmtKind`]s divide into five answers and each is pinned below: `INSERT` and
/// CTAS **write** a remote relation once the connection is opted in and are refused by the
/// read-only sentence until it is, the other five kinds that name a target are refused by
/// [`in_database`](crate::statements::target::in_database), a **read** of one is never refused (`COPY`'s source, `PREPARE`'s body, and
/// every plain query), a function name cannot be qualified at all (DataFusion refuses it while
/// planning, which is one refusal in one place rather than a second of ours), and the four session
/// statements name no relation.
///
/// Against a fake catalog rather than a server: see `sources::fake::fake_source` for what that does
/// and does not stand in for. It is registered on the session and held by no `Live`, which is
/// exactly a connection that is not opted in — so the write half is pinned here at its refusal and
/// the landing is `tests/postgres_federation.rs`'s, where a real server can take an insert.
#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::{env, process};

    use crate::providers::fake_source;
    use crate::statements::target::{elsewhere, in_database, read_only};
    use crate::statements::Remote;
    use crate::{Engine, RunOutcome, RunTag, WsId};
    use strata_core::project::{save_defs, ProjectDefs};

    use super::*;

    /// An engine with a project folder, a workspace table, and a live database connection's
    /// catalog called `pg` holding `pg.public.orders`.
    ///
    /// The workspace table is called `orders` **too**, on purpose: every refusal below has to
    /// be about the name the user wrote and not about the bare component it ends with, and a
    /// fixture where the two could not collide would prove neither.
    async fn engine(tag: &str) -> (PathBuf, Arc<Engine>) {
        let root = scratch(tag);
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        run(&eng, "CREATE TABLE orders AS SELECT 1 AS id, 2 AS total")
            .await
            .expect("workspace table");
        fake_source(&eng.ctx, "pg", &["orders"]);
        (root, eng)
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_ddl_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        save_defs(&dir, &ProjectDefs::default()).unwrap();
        dir
    }

    async fn run(eng: &Engine, sql: &str) -> Result<RunOutcome, String> {
        eng.ws(WsId(1)).run(RunTag(1), sql.into(), 10).await
    }

    /// The refusal `sql` came back with, or a failure naming what it did instead.
    async fn refusal(eng: &Engine, sql: &str) -> String {
        match run(eng, sql).await {
            Err(why) => why,
            Ok(_) => panic!("'{sql}' was not refused"),
        }
    }

    /// The one statement left that will not touch a relation inside a database connection —
    /// registering a table externally, which declares files and a format for something the server
    /// already describes.
    #[tokio::test]
    async fn registering_a_remote_relation_externally_refuses() {
        let (_root, eng) = engine("targets").await;
        assert_eq!(
            refusal(
                &eng,
                "CREATE EXTERNAL TABLE pg.public.orders STORED AS PARQUET LOCATION 'data/'"
            )
            .await,
            in_database("pg.public.orders", "pg")
        );
    }

    /// **A write against a connection nobody opted in names the toggle**, whichever statement it
    /// is — one sentence, minted once, because the fix is one setting.
    ///
    /// The `INSERT` takes its source from the target's own columns, so the target's *schema*
    /// cannot be what refuses it: this is about the connection and nothing else. And the refusal
    /// is reached **before** `Catalog::is_internal`, which is not a question to ask about a
    /// relation whose data Strata could never own.
    ///
    /// The last five are the statements the **server** would have run, so the gate is the same one
    /// standing in front of a different mechanism.
    #[tokio::test]
    async fn a_write_into_a_read_only_connection_names_the_toggle() {
        let (_root, eng) = engine("read_only").await;
        let expected = read_only(&Remote {
            connection: "pg".into(),
            reference: TableReference::full("pg", "public", "orders"),
        });
        for sql in [
            "INSERT INTO pg.public.orders SELECT id, total FROM pg.public.orders",
            "CREATE TABLE pg.public.orders AS SELECT 1 AS id",
            "CREATE TABLE pg.public.orders (id INT)",
            "CREATE VIEW pg.public.orders AS SELECT id FROM pg.public.orders",
            "DROP TABLE pg.public.orders",
            "DROP VIEW pg.public.orders",
            "DELETE FROM pg.public.orders WHERE id = 1",
        ] {
            assert_eq!(refusal(&eng, sql).await, expected, "'{sql}'");
        }
        assert!(
            expected.contains("Read only"),
            "the sentence names the setting: {expected}"
        );
    }

    /// **A bare write target resolves like a bare read**. Before it, `sql::qualify`
    /// refused one that only a connection had, because "not found" was the wrong answer about a
    /// relation the same session would happily read. Now it rewrites, and the refusal that lands
    /// is the arm's own — which is the point: one funnel, whether or not the qualifier was typed.
    #[tokio::test]
    async fn a_bare_write_target_reaches_the_arm_the_qualified_one_does() {
        let (_root, eng) = engine("bare_write").await;
        fake_source(&eng.ctx, "warehouse", &["shipments"]);

        let expected = read_only(&Remote {
            connection: "warehouse".into(),
            reference: TableReference::full("warehouse", "public", "shipments"),
        });
        assert_eq!(
            refusal(&eng, "INSERT INTO shipments VALUES (1, 2)").await,
            expected
        );
    }

    /// And the workspace table of the same bare name is untouched by all of it — the collision
    /// the qualified refusal exists to keep apart, asserted from the other side.
    #[tokio::test]
    async fn the_workspace_table_of_the_same_name_still_drops() {
        let (_root, eng) = engine("collision").await;
        let _ = refusal(&eng, "DROP TABLE pg.public.orders").await;
        let RunOutcome::Statement(report) = run(&eng, "DROP TABLE orders").await.expect("dropped")
        else {
            panic!("DROP TABLE ran as a query");
        };
        assert_eq!(report.message, "Table 'orders' and its data were deleted");
    }

    /// A qualifier that names **nothing** keeps the older wording, which is a different fact
    /// and has to stay a different sentence: there is no connection to name.
    #[tokio::test]
    async fn an_unknown_catalog_is_still_nowhere() {
        let (_root, eng) = engine("unknown").await;
        assert_eq!(
            refusal(&eng, "DROP TABLE nosuch.public.orders").await,
            elsewhere("Tables")
        );
        assert_eq!(
            refusal(&eng, "CREATE VIEW nosuch.public.v AS SELECT 1").await,
            elsewhere("Views")
        );
    }

    /// **The workspace catalog is never a database connection**, however it is spelled.
    ///
    /// `source_catalog` folds before it compares, and this is what says so: the catalog list
    /// resolves by `fold_ident`, so a quoted `"STRATA"` names the workspace — and an unfolded
    /// guard let that spelling past, whereupon the search *matched the workspace's own entry*
    /// and told the user their project's catalog was a connection. No real connection can
    /// produce that sentence: `check_catalog` refuses the name `strata` case-insensitively, so it
    /// would have named a connection that cannot exist.
    ///
    /// And what it answers instead is the *right* thing: the name resolves to the workspace
    /// catalog, so the statement simply acts on the workspace table, exactly as the unquoted
    /// `strata.public.orders` does.
    #[tokio::test]
    async fn the_workspace_catalog_is_never_named_as_a_connection() {
        let (_root, eng) = engine("workspace_spelling").await;
        let RunOutcome::Statement(report) =
            run(&eng, "CREATE VIEW \"STRATA\".public.v AS SELECT 1")
                .await
                .expect("a quoted spelling of the workspace catalog is the workspace")
        else {
            panic!("CREATE VIEW ran as a query");
        };
        assert_eq!(report.message, "View 'v' created");

        let RunOutcome::Statement(report) = run(&eng, "DROP TABLE \"STRATA\".public.orders")
            .await
            .expect("and so is this one")
        else {
            panic!("DROP TABLE ran as a query");
        };
        assert!(
            report
                .message
                .starts_with("Table 'orders' and its data were deleted"),
            "{}",
            report.message
        );
    }

    /// **Reading is not managing.** The three ways a statement the router touches can read a
    /// remote relation all work: a query, a `COPY`'s source, and a `PREPARE`d body — which is
    /// the whole point of the connection and the thing an over-broad gate would break.
    #[tokio::test]
    async fn reading_a_remote_relation_is_never_refused() {
        let (root, eng) = engine("reads").await;
        let out = root.join("out.parquet");
        run(&eng, "SELECT id FROM pg.public.orders")
            .await
            .expect("a plain query reads the connection");
        run(
            &eng,
            &format!(
                "COPY (SELECT id FROM pg.public.orders) TO '{}'",
                out.display()
            ),
        )
        .await
        .expect("and a COPY may take its source from one");
        run(&eng, "PREPARE p AS SELECT id FROM pg.public.orders")
            .await
            .expect("and a PREPARE may hold one");
        run(&eng, "EXECUTE p").await.expect("and EXECUTE run it");
    }

    /// A cross-source read is the load-bearing one: the workspace table and the remote relation
    /// of the same bare name, joined, both resolving to what their qualifier says.
    #[tokio::test]
    async fn a_cross_source_query_resolves_both_sides() {
        let (_root, eng) = engine("cross").await;
        run(
            &eng,
            "SELECT o.total FROM orders o JOIN pg.public.orders r ON o.id = r.id",
        )
        .await
        .expect("cross-source join");
    }

    /// **`CREATE`/`DROP FUNCTION` cannot take a qualified name at all**, and the refusal is
    /// DataFusion's own (`datafusion-sql-54.0.0/src/statement.rs:1390` and `:1484`, both
    /// `not_impl_err!("Qualified functions are not supported")`). Pinned rather than
    /// re-implemented: a second fence of ours would be a second sentence for one fact, and this
    /// one already names the thing that is wrong.
    #[tokio::test]
    async fn a_function_name_cannot_be_qualified() {
        let (_root, eng) = engine("functions").await;
        for sql in [
            "CREATE FUNCTION pg.public.f(x INT) RETURNS INT RETURN x + 1",
            "DROP FUNCTION pg.public.f",
        ] {
            assert!(
                refusal(&eng, sql).await.contains("Qualified functions"),
                "'{sql}'"
            );
        }
    }

    /// The session statements name no relation, so a remote catalog cannot reach them — pinned
    /// because "not applicable" is an answer the checklist has to state rather than skip.
    #[tokio::test]
    async fn the_session_statements_are_unaffected() {
        let (_root, eng) = engine("session").await;
        run(&eng, "SET datafusion.execution.batch_size = 4096")
            .await
            .expect("SET");
        run(&eng, "RESET datafusion.execution.batch_size")
            .await
            .expect("RESET");
        run(&eng, "PREPARE q AS SELECT 1").await.expect("PREPARE");
        run(&eng, "DEALLOCATE q").await.expect("DEALLOCATE");
    }

    /// A `postgres://` URL typed into a `LOCATION` is **not** a way into a connection: it splits
    /// like any other remote location and lands on the membership refusal, naming a connection
    /// the project does not have. Pinned because the alternative — a bare planner error, or a
    /// panic on a URL whose path is a database name — is what `url()`-carries-a-path makes
    /// possible.
    #[tokio::test]
    async fn a_database_url_in_a_location_is_refused_as_a_connection() {
        let (_root, eng) = engine("location").await;
        let why = refusal(
            &eng,
            "CREATE EXTERNAL TABLE t STORED AS PARQUET LOCATION 'postgres://host:5432/db/x'",
        )
        .await;
        assert!(
            why.contains("postgres://host:5432") && why.contains("connection"),
            "{why}"
        );
    }
}
