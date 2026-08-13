//! Statement **execution** — the `Verdict::Intercept` half of the router (ED-02,
//! `docs/STATEMENTS_SPEC.md` §4 + §7).
//!
//! [`Engine::run`](crate::engine::Engine::run) classifies once, in front of dispatch; a
//! statement the editor implements itself lands here as its [`StmtKind`], and comes back as a
//! [`StatementReport`] — what to say, how many rows it moved, and the [`StoreEffect`] the app
//! folds into `ProjectState`. Nothing here returns rows and nothing here touches the snapshot
//! lifecycle: DDL never retires a snapshot (`docs/SNAPSHOT_SPEC.md` §4), so a tab that creates a
//! table can still page the result it had.
//!
//! **The store learns from the returned value, never by introspection.** That is the whole
//! reason lifecycle is intercepted rather than left to DataFusion's provider traits (spec §3):
//! `SchemaProvider::register_table` cannot say who called it or await anything, so an accreted
//! native-DDL state would have to be *read back* — the `FetchCatalog` refetch the catalog
//! invariant forbids — or pushed out through a channel, which is the message-passing
//! architecture the direct-call facade deleted.
//!
//! **Every arm is one call into a funnel that already exists.** Typed `CREATE VIEW` runs
//! [`views::create`] — the body [`Engine::create_view`](crate::engine::Engine::create_view) runs
//! for ⌘S; typed `CREATE EXTERNAL TABLE` and a CTAS's spooled output are both
//! `catalog::register_external`. ED-02 shipped the dispatch and the vocabulary and each arm was
//! filled by the task that owned its capability; ED-10 was the last of them, so there is no stub
//! refusal left and the `match` below is exhaustive on `StmtKind` with every arm real.

mod copy;
mod external;
mod functions;
mod session;
mod tables;
mod views;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use datafusion::catalog::TableProvider;
use datafusion::logical_expr::TableType;
use datafusion::prelude::SessionContext;
use datafusion::sql::parser::Statement as DFStatement;
use datafusion::sql::TableReference;

use crate::engine::catalog::{TableMeta, ViewMeta};
use crate::engine::functions::Functions;
use crate::engine::providers::in_workspace;
use crate::engine::sql::StmtKind;
use crate::engine::{fold_ident, Connections, InternalTables, CATALOG, SCHEMA};
use crate::util::plural;
use strata_model::{TableDef, ViewDef};

/// The statement family's completion vocabulary (ED-11) — the format words `STORED AS`
/// takes and the per-format `OPTIONS` key tables, owned by the module whose arms they
/// mirror and read by `sql::complete` so the offer and the arm set are one table.
pub(crate) use external::{option_keys_for, OptionKind, STORED_AS_FORMATS};
/// DataFusion's own seam for `CREATE FUNCTION` (ED-09) — installed on every engine by
/// `build_context`, which is what makes the statement dispatchable at all.
pub(super) use functions::StrataFunctionFactory;
/// The `SET` overlay's key fence (ED-08) — also the `SET` key pool's filter (ED-11), so
/// what completion offers and what dispatch accepts cannot drift.
pub(crate) use session::refuse_reserved_key;
/// The session state a statement can move (ED-08) — held by the engine, reached by the arms.
pub use session::SessionScope;
/// The per-row type probe behind that panel's free-text type field — see
/// [`tables::column_type`].
pub(super) use tables::column_type;
/// A table drop's own words — see [`tables::drop_intent`]. Re-exported here because the
/// catalog pane says them too, and `ddl` is the vocabulary module the app already reads.
pub use tables::drop_intent;
pub(super) use tables::drop_table;
/// A repeated column name's refusal — see [`tables::duplicate_column`]. Re-exported for the
/// same reason `drop_intent` is: the empty-table panel (IT-01) refuses one as it is typed, and
/// it must say what the create arm would have said.
pub use tables::duplicate_column;
pub(super) use views::{create as create_view, drop as drop_view};

/// What one intercepted statement did — the `RunOutcome::Statement` the results pane renders
/// as a status row and the app folds into its stores.
#[derive(Clone, Debug, PartialEq)]
pub struct StatementReport {
    /// Which statement ran. The results pane's label and the log's subject come off
    /// [`StmtKind::label`], so the kind travels rather than a second spelling of it.
    pub kind: StmtKind,
    /// The sentence the user reads, in the app's IDE register — and the one place a
    /// session-scoped outcome says so ("for this session"), since `SET`, prepared statements
    /// and created functions die with the engine (spec §8).
    pub message: String,
    /// Rows created / inserted / exported, where the statement moved any. `None` is *not
    /// applicable* — a `DROP` or a `SET` counts nothing, which is a different fact from
    /// counting zero.
    pub count: Option<u64>,
    pub elapsed_ms: u128,
    /// What the app folds into `ProjectState`. `None` where the statement changed nothing the
    /// catalog holds; deliberately not a `StoreEffect::None` variant beside it, which would be
    /// a second way to say the same thing and a second arm every fold has to remember.
    pub effect: Option<StoreEffect>,
}

/// What an arm answers with — [`StatementReport`] minus the two fields `execute` owns. An arm
/// therefore cannot mislabel itself or forget to stamp the clock.
pub struct StatementOutcome {
    pub message: String,
    pub count: Option<u64>,
    pub effect: Option<StoreEffect>,
}

/// The catalog mutation a statement leaves behind, as a **value the app applies** — the
/// `save_view` fold generalized (spec §7): store upsert on the matching `ProjChan` → the
/// persist funnel → `catalog_settled` → the event log.
///
/// The store stays the catalog authority, so nothing here is a request to go and look: an
/// effect carries the def *and* what registration learned about it, exactly as the load-time
/// pass hands both to the same row.
#[derive(Clone, Debug, PartialEq)]
pub enum StoreEffect {
    /// A table def arrived or was rewritten, already registered — an internal table's CTAS
    /// output (ED-04) or a typed `CREATE EXTERNAL TABLE` (ED-10). The def is the durable,
    /// shareable half; the meta is the answer that lands on its row.
    TableUpserted { def: TableDef, meta: TableMeta },
    /// A table def is gone and its provider deregistered (ED-05). `dependents` are the views
    /// left reading it — **named, never cascaded**: a `ViewTable`'s inlined plan goes on
    /// executing until reload, and the epoch bump makes diagnostics re-derive immediately,
    /// which is the surface that matters. They go `Reg::Failed` honestly on the next pass.
    TableRemoved {
        name: String,
        dependents: Vec<String>,
    },
    /// A view def arrived or was rewritten, already created (ED-06) — the same pair ⌘S folds.
    ViewUpserted { def: ViewDef, meta: ViewMeta },
    /// A view def is gone and the view dropped (ED-06).
    ViewRemoved { name: String },
    /// The table's *data* moved but its def did not — an `INSERT` appending a file (ED-05).
    /// A re-scan is what refreshes `TableMeta.rows`, because a row count is something the
    /// scan driver reads, never something the store adds up for itself.
    RescanTable { name: String },
    /// The session's function catalog moved (ED-09). Nothing persists — functions are
    /// session-scoped (spec §8) — but names that did not resolve a moment ago now do, so the
    /// catalog epoch has to move with them.
    FunctionsChanged,
    /// The session's prepared statements moved (ED-08) — a `PREPARE` or a `DEALLOCATE`. Nothing
    /// persists either, and for the same reason it is still an effect: `EXECUTE p` resolves now
    /// and did not a moment ago, so both the language service's snapshot and every tab's
    /// diagnostics have to be re-derived against the session the engine now holds.
    PreparedChanged,
}

/// Where an intercepted statement may write, and what it may write **relative to**.
///
/// The **project folder**, not `.strata/tables` — because a statement that creates an internal
/// table produces two things from it: an absolute path to spool into, and the project-relative
/// source path the def stores, which is what makes the def portable
/// ([`internal_source`](crate::project::internal_source)). Handing down only the data directory
/// would leave the def naming an absolute path on the machine that ran the statement.
///
/// `None` is an engine with no project behind it — the agent's headless workspaces before a
/// project is opened, and every test fixture. Nothing that only reads notices; the arms that
/// write refuse politely.
pub type DataRoot = Option<PathBuf>;

/// What an intercepted statement can reach **of the engine**, gathered once in
/// [`Engine::run`](crate::engine::Engine::run).
///
/// Every member is a copy — a handle where the state is shared, a clone where it is a value — for
/// one reason: the arms run inside the task `Engine::bookkeep` spawned, and that task must not
/// hold the engine, because the engine's `Drop` is what aborts it. `internal`, `scope` and
/// `functions` hold values only, so they outlive an engine harmlessly; `root` and `baseline` are
/// snapshots taken at dispatch, which is the moment they are true for.
///
/// One value rather than a parameter list because it is one thing — the engine, minus everything
/// an arm may not touch — and it gains a member for each capability this workstream lifts.
pub struct Dispatch {
    /// Where an internal table's data may be written (ED-04).
    pub root: DataRoot,
    /// Which registered tables Strata owns the data of (ED-04/05).
    pub internal: InternalTables,
    /// Which object stores this project has a connection to (ED-10) — what a typed
    /// `CREATE EXTERNAL TABLE`'s `LOCATION` may name.
    pub connections: Connections,
    /// The `SET` overlay and the prepared-statement mirror (ED-08).
    pub scope: SessionScope,
    /// The function catalog and the names this session created (ED-09).
    pub functions: Functions,
    /// The engine's `datafusion.*` overrides — what a `RESET` puts a key back to
    /// (`session::reset`), which is the Settings baseline rather than DataFusion's default.
    pub baseline: BTreeMap<String, String>,
}

/// Execute one intercepted statement and report what it did.
///
/// The timer and the kind are stamped here rather than in the arms, so a report can never
/// disagree with the statement that produced it.
pub async fn execute(
    ctx: &SessionContext,
    kind: StmtKind,
    stmt: DFStatement,
    engine: Dispatch,
) -> Result<StatementReport, String> {
    let Dispatch {
        root,
        internal,
        connections,
        scope,
        functions: registry,
        baseline,
    } = engine;
    let start = Instant::now();
    let outcome: StatementOutcome = match kind {
        StmtKind::CreateTable | StmtKind::Ctas => tables::create(ctx, kind, stmt, root).await,
        StmtKind::Insert => tables::insert(ctx, stmt, &internal).await,
        StmtKind::DropTable => tables::drop_statement(ctx, &root, &internal, stmt).await,
        StmtKind::CreateView => views::create_statement(ctx, stmt).await,
        StmtKind::DropView => views::drop_statement(ctx, stmt).await,
        StmtKind::Copy => copy::copy_to(ctx, stmt, &root).await,
        StmtKind::Set => session::set(ctx, stmt, &scope).await,
        StmtKind::Reset => session::reset(ctx, stmt, &scope, &baseline).await,
        StmtKind::Prepare => session::prepare(ctx, stmt, &scope).await,
        StmtKind::Deallocate => session::deallocate(ctx, stmt, &scope).await,
        StmtKind::CreateFunction => functions::create(ctx, stmt, &registry).await,
        StmtKind::DropFunction => functions::drop(ctx, stmt, &registry).await,
        StmtKind::CreateExternalTable => {
            external::create(ctx, stmt, &root, &internal, &connections).await
        }
    }?;
    Ok(StatementReport {
        kind,
        message: outcome.message,
        count: outcome.count,
        elapsed_ms: start.elapsed().as_millis(),
        effect: outcome.effect,
    })
}

/// What `name` resolves to in the engine's one schema, and what kind it is — `None` when the
/// name is free. The one existence question every arm asks, because tables and views share that
/// namespace and a create has to know which of them it is standing on.
///
/// Through `table_provider`, not `table`: the latter builds a `DataFrame`, which for a view means
/// planning its whole body just to ask whether the name is taken. Addressed as a **bare, folded**
/// reference for the reason [`Engine::create_view`](crate::engine::Engine::create_view) gives —
/// `impl Into<TableReference> for &str` parses, and a name that needed quoting does not survive a
/// parse, so it would be looked up under a name nothing ever registered.
pub(super) async fn existing(ctx: &SessionContext, name: &str) -> Option<TableType> {
    let provider: Arc<dyn TableProvider> = ctx
        .table_provider(TableReference::bare(fold_ident(name)))
        .await
        .ok()?;
    Some(provider.table_type())
}

/// The bare name a statement targets, and `what` those statements create — `"Tables"`,
/// `"Views"`.
///
/// **The one choke point in front of every arm.** The *workspace* catalog has exactly one schema
/// (`engine::providers`), so a qualified name is one of three things: a longer spelling of the
/// same place, a relation inside a database connection's catalog, or nowhere at all —
/// and registration takes a bare name, so an unrecognised qualifier would otherwise be silently
/// dropped and the object created somewhere the user did not ask for.
///
/// The middle case is the DB workstream's, and the reason this refusal is worded here rather than
/// per arm: **every** intercepted statement that resolves a target comes through this function, so
/// one sentence covers `DROP TABLE pg.public.orders`, `INSERT INTO pg.…`, `CREATE VIEW pg.…` and
/// the rest — and an arm that grew its own copy of the check would be the drift this prevents.
/// The catalog list is asked rather than a list of connections, because it is what *resolves* the
/// name: a catalog is registered exactly while its connection is live
/// ([`StrataCatalogList`](crate::engine::providers::StrataCatalogList)), which is also the window
/// in which the user can address it.
pub(super) fn bare_name(
    ctx: &SessionContext,
    name: &TableReference,
    what: &str,
) -> Result<String, String> {
    if in_workspace(name) {
        return Ok(name.table().to_string());
    }
    Err(match database_catalog(ctx, name) {
        Some(catalog) => in_database(name, &catalog),
        None => elsewhere(what),
    })
}

/// The database connection's catalog `name` sits in, in the spelling it was registered under —
/// `None` for the workspace catalog, and for a qualifier that resolves to nothing.
///
/// The registered spelling rather than the folded key, because that is what the connection is
/// called everywhere else the user meets it: the catalog list keeps both for exactly this
/// (see its `catalogs` field).
///
/// **Folded on both sides, the workspace's own name included.** The catalog list resolves by
/// [`fold_ident`], so a quoted `"STRATA"` names the workspace catalog — and compared raw it
/// would slip past the guard below and then *match* the workspace's own entry in the search,
/// telling the user their project's catalog is a database connection. No real connection can
/// produce that (`PgStore::check_catalog` refuses `strata` case-insensitively), so the sentence
/// would name a connection that cannot exist.
fn database_catalog(ctx: &SessionContext, name: &TableReference) -> Option<String> {
    let TableReference::Full { catalog, .. } = name else {
        return None;
    };
    let folded = fold_ident(catalog);
    if folded == CATALOG {
        return None;
    }
    ctx.catalog_names()
        .into_iter()
        .find(|registered| fold_ident(registered) == folded)
}

/// The wording for a name inside a database connection's catalog — read-only in v1, and the
/// sentence says which of the two halves is true rather than naming a surface to go and use:
/// there is no Strata surface that creates a remote table, and the server's own client is not
/// something this app can point at.
///
/// Not parameterised by `what`: the answer is the same for a table and for a view, because it is
/// about the *catalog* and not about the kind of thing being made in it.
pub(super) fn in_database(name: &TableReference, catalog: &str) -> String {
    format!(
        "'{name}' is in the database connection '{catalog}'. Strata reads remote tables; it does \
         not create, drop or write them"
    )
}

/// The wording for a name that points outside Strata's single schema — held apart from
/// [`bare_name`] because a caller that parses the name itself has to be able to refuse the forms
/// a `TableReference` cannot even represent, in the same words (`views::definition`).
pub(super) fn elsewhere(what: &str) -> String {
    format!("Strata has one schema, '{SCHEMA}'. {what} cannot be created elsewhere")
}

/// What a drop leaves behind, appended to its report — empty when it leaves nothing.
///
/// One wording for both drops, because "left invalid" is one fact: a dependent's plan was inlined
/// when it was created and goes on executing until reload, so nothing is stale yet and nothing is
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

/// **The statement policy over a database connection's catalog** (DB-03) — one test module for
/// one rule, because the rule is cross-arm: `bare_name` is the single choke point in front of
/// every intercepted statement that resolves a target, so what is under test is that *no arm
/// gets there another way*.
///
/// The fourteen [`StmtKind`]s divide into four answers and each is pinned below: a target inside
/// a remote catalog is refused by name (the seven kinds that name a target), a **read** of one is
/// not (`COPY`'s source, `PREPARE`'s body, and every plain query), a function name cannot be
/// qualified at all (DataFusion refuses it while planning, which is one refusal in one place
/// rather than a second of ours), and the four session statements name no relation.
///
/// Against a fake catalog rather than a server: see `providers::fake_database` for what that does
/// and does not stand in for.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::{env, process};

    use crate::engine::providers::fake_database;
    use crate::engine::{Engine, RunOutcome, RunTag, WsId};
    use crate::project::{save_defs, ProjectDefs};

    use super::*;

    /// An engine with a project folder, a workspace table, and a live database connection's
    /// catalog called `pg` holding `pg.public.orders`.
    ///
    /// The workspace table is called `orders` **too**, on purpose: every refusal below has to
    /// be about the name the user wrote and not about the bare component it ends with, and a
    /// fixture where the two could not collide would prove neither.
    async fn engine(tag: &str) -> (PathBuf, Engine) {
        let root = scratch(tag);
        let eng = Engine::new(BTreeMap::new());
        eng.set_data_dir(&root);
        run(&eng, "CREATE TABLE orders AS SELECT 1 AS id, 2 AS total")
            .await
            .expect("workspace table");
        fake_database(&eng.ctx, "pg", &["orders"]);
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
        eng.run(WsId(1), RunTag(1), sql.into(), 10).await
    }

    /// The refusal `sql` came back with, or a failure naming what it did instead.
    async fn refusal(eng: &Engine, sql: &str) -> String {
        match run(eng, sql).await {
            Err(why) => why,
            Ok(_) => panic!("'{sql}' was not refused"),
        }
    }

    /// Every statement that names a target: refused, naming the connection, whatever the arm.
    ///
    /// One test over the list rather than seven, because the point *is* that they answer
    /// identically — seven tests asserting one sentence would let six of them keep passing while
    /// an arm quietly grew a wording of its own.
    ///
    /// The `INSERT` takes its source from the target's own columns, so the target's *schema*
    /// cannot be what refuses it: this is about the target's catalog and nothing else.
    #[tokio::test]
    async fn every_statement_that_names_a_target_refuses_a_remote_one() {
        let (_root, eng) = engine("targets").await;
        let expected = in_database(&TableReference::parse_str("pg.public.orders"), "pg");
        for sql in [
            "CREATE TABLE pg.public.orders (id INT)",
            "CREATE TABLE pg.public.orders AS SELECT 1 AS id",
            "INSERT INTO pg.public.orders SELECT id, total FROM pg.public.orders",
            "DROP TABLE pg.public.orders",
            "DROP VIEW pg.public.orders",
            "CREATE VIEW pg.public.orders AS SELECT 1 AS id",
            "CREATE EXTERNAL TABLE pg.public.orders STORED AS PARQUET LOCATION 'data/'",
        ] {
            assert_eq!(refusal(&eng, sql).await, expected, "'{sql}'");
        }
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
    /// `database_catalog` folds before it compares, and this is what says so: the catalog list
    /// resolves by `fold_ident`, so a quoted `"STRATA"` names the workspace — and an unfolded
    /// guard let that spelling past, whereupon the search *matched the workspace's own entry*
    /// and told the user their project's catalog was a database connection. No real connection
    /// can produce that sentence: `PgStore::check_catalog` refuses the name `strata`
    /// case-insensitively, so it would have named a connection that cannot exist.
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
