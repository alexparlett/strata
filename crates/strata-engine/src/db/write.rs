//! **Writing into a database connection** (DB-10) — the two statements DataFusion can plan
//! against a remote relation: `INSERT INTO pg.public.events SELECT …` and
//! `CREATE TABLE pg.public.report AS SELECT …`.
//!
//! **The write provider is resolved by the arm and lives nowhere.**
//! `PostgresTableWriter` *wraps* the federated read provider, so the node a plan sees is the
//! writer rather than the `FederatedTableProviderAdaptor` the federation rule's downcast walk
//! looks for. Serving writers from [`DbSchemaProvider`](super::DbSchemaProvider) would forfeit
//! pushdown on **every read** — the failure the workstream's own-provider decision exists to
//! prevent — so the catalog goes on serving read providers and a writer is built here, used once
//! and dropped.
//!
//! **The schema the sink validates against is the caller's, because the two statements know
//! different things.** An `INSERT` reaches this with an input DataFusion has already coerced to
//! the *target's* schema, so the server's own is the right one to check. A CTAS created the table
//! from its input's schema a moment ago, so that is what its batches must match; the values reach
//! the server as literals (`InsertBuilder`), which it coerces into the columns it made.

use std::sync::Arc;

use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::TableProvider;
use datafusion::common::Constraints;
use datafusion::logical_expr::dml::InsertOp;
use datafusion::logical_expr::LogicalPlan;
use datafusion::optimizer::optimize_projections::OptimizeProjections;
use datafusion::optimizer::{Optimizer, OptimizerContext};
use datafusion::physical_plan::coalesce_partitions::CoalescePartitionsExec;
use datafusion::physical_plan::{collect, ExecutionPlan, ExecutionPlanProperties};
use datafusion::prelude::SessionContext;
use datafusion::sql::TableReference;
use datafusion_table_providers_common::sql::arrow_sql_gen::statement::CreateTableBuilder;
use datafusion_table_providers_postgres::arrow_sql_gen::statement_ext::CreateTableBuilderPostgresExt;
use datafusion_table_providers_postgres::pool::PostgresConnectionPool;
use datafusion_table_providers_postgres::write::PostgresTableWriter;
use datafusion_table_providers_postgres::Postgres;

use crate::export::copy_row_count;
use crate::sql::qualified;

/// One relation inside a database connection, as a **write target**: the connection's catalog in
/// its registered spelling, and the schema and relation as the statement named them.
///
/// Minted by [`ddl::remote_target`](crate::ddl) in front of the arms, so the question "is this
/// name the workspace's" is asked in one place and answered once.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteTarget {
    pub catalog: String,
    pub schema: String,
    pub table: String,
}

impl RemoteTarget {
    /// The address a message prints — `qualified`, because these three parts are a server's
    /// spelling and quoting them whole would name one relation with dots in it.
    pub fn address(&self) -> String {
        qualified([
            self.catalog.as_str(),
            self.schema.as_str(),
            self.table.as_str(),
        ])
    }

    /// How the **server** is addressed: `schema.table`, never the catalog, which is Strata's own
    /// prefix for the connection and means nothing to Postgres. `InsertBuilder` and
    /// `Postgres::table_exists` both read this reference, so a full one would render
    /// `"pg"."public"."orders"` into a statement the server then refuses.
    fn relation(&self) -> TableReference {
        TableReference::partial(self.schema.clone(), self.table.clone())
    }
}

/// Whether one relation is there, by the same catalog the enumeration reads — `pg_class`, so a
/// view or a partitioned table answers as truthfully as an ordinary one.
const RELATION_EXISTS: &str = "\
SELECT EXISTS ( \
  SELECT 1 FROM pg_catalog.pg_class c \
  JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
  WHERE n.nspname = $1 AND c.relname = $2 \
)";

/// An identifier as a statement **the server** parses may say it — quoted unconditionally.
///
/// **Not [`quote_verbatim`], which is a third audience's rule.** That one asks
/// `sql::needs_quoting`, whose reserved set is sqlparser's `RESERVED_FOR_TABLE_ALIAS` and
/// `RESERVED_FOR_COLUMN_ALIAS` — the words *DataFusion's* parser cannot read as a name. A
/// `PostgreSQL` server reserves far more (`user`, `table`, `default`, `check`, `primary`, `column`, `constraint`,
/// `references`, `unique`, `grant`, none of them in either sqlparser list), so a relation called
/// `user` would go out bare and the statement would be a syntax error — which, for
/// [`discard`], means the rollback silently fails and leaves the husk it exists to remove.
/// The server treats `"loaded"` and `loaded` as one name, so quoting always costs nothing, and it
/// is what `CreateTableBuilder` already does for the other half of the same operation.
fn server_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `at` as the **server** addresses it: `"schema"."table"`, both parts quoted.
fn server_relation(at: &RemoteTarget) -> String {
    format!("{}.{}", server_ident(&at.schema), server_ident(&at.table))
}

/// Append `input`'s rows to the relation `provider` reads, and report how many landed.
///
/// `schema` is what the sink validates each batch against — see the module docs for why it is the
/// caller's answer rather than one derived here.
///
/// **The input is coalesced first.** `DataSinkExec` reads partition 0 and nothing else, and says
/// so: a plan built outside the physical optimizer has to arrive single-partition, or a
/// repartitioned scan would write a fraction of its rows and report the fraction as the whole.
pub(super) async fn append(
    ctx: &SessionContext,
    pool: &Arc<PostgresConnectionPool>,
    at: &RemoteTarget,
    provider: Arc<dyn TableProvider>,
    schema: SchemaRef,
    input: &LogicalPlan,
) -> Result<u64, String> {
    let state = ctx.state();
    let planned = state
        .create_physical_plan(&collapse_projections(input)?)
        .await
        .map_err(|e| e.to_string())?;
    let single: Arc<dyn ExecutionPlan> = match planned.output_partitioning().partition_count() {
        1 => planned,
        _ => Arc::new(CoalescePartitionsExec::new(planned)),
    };

    let target = Postgres::new(
        at.relation(),
        Arc::clone(pool),
        schema,
        Constraints::default(),
    );
    let writer = PostgresTableWriter::create(provider, target, None);
    let sink = writer
        .insert_into(&state, single, InsertOp::Append)
        .await
        .map_err(|e| e.to_string())?;
    let batches = collect(sink, ctx.task_ctx())
        .await
        .map_err(|e| e.to_string())?;
    Ok(copy_row_count(&batches) as u64)
}

/// Collapse the redundant projection DataFusion's `INSERT` planner leaves, **before** the
/// federation analyzer wraps the plan.
///
/// `INSERT INTO t SELECT a, b FROM u` plans as a renaming projection over the query's own
/// projection, and DataFusion's unparser renders `Projection -> Projection -> TableScan` as a
/// derived table (`… FROM (SELECT …) AS "derived_projection"`) while leaving the **outer** column
/// references carrying the scan's qualifier — so a remote source comes back from the server as
/// `missing FROM-clause entry for table "customers"`. No statement a user can *write* produces
/// that shape (a subquery carries an alias the outer refs then use); only a planner-built plan
/// does, which is why it surfaced here first.
///
/// It has to be done here rather than through the executor's `logical_optimizer` hook, which is
/// otherwise exactly the seam for it: by the time that hook runs the plan is already inside the
/// federation crate's extension node, so a rule walking it rewrites nothing.
///
/// One rule rather than the default optimizer — the rest of that pass is about *execution*, and
/// this is about what can be written down. `create_physical_plan` still runs the full analyzer and
/// optimizer over the result.
fn collapse_projections(input: &LogicalPlan) -> Result<LogicalPlan, String> {
    Optimizer::with_rules(vec![Arc::new(OptimizeProjections::new())])
        .optimize(input.clone(), &OptimizerContext::new(), |_, _| {})
        .map_err(|e| e.to_string())
}

/// Create the relation `at` from `schema` — the server table a CTAS then fills. `false` means the
/// server already held it and nothing was made.
///
/// **The existence check is inside this transaction, and that is the whole point of it being
/// here.** `CreateTableBuilder` hardcodes `IF NOT EXISTS`, so a relation that appeared since the
/// arm last looked would be silently adopted — and then dropped by [`discard`] if the fill failed,
/// destroying a table this statement never created. Asked and created together, the answer cannot
/// go stale between the two, so a `true` is a promise that the rollback only ever removes our own
/// work.
///
/// **Under a `search_path` of exactly the target schema.** `CreateTableBuilder` renders an
/// unqualified name (`sea_query`'s `Alias` is one identifier, and there is no schema on the
/// builder), so without the `SET LOCAL` a `CREATE TABLE pg.warehouse.report` would land wherever
/// the role's search path pointed. The composite types a struct column needs are rendered by the
/// same builder and go into the same schema for the same reason.
///
/// The transaction is a real one rather than a `BEGIN` in a batch, because its `Drop` is the
/// rollback: a statement failing halfway through a batched `BEGIN` would hand a connection back
/// to the pool inside an aborted transaction.
pub(super) async fn create(
    pool: &Arc<PostgresConnectionPool>,
    at: &RemoteTarget,
    schema: SchemaRef,
) -> Result<bool, String> {
    let mut statements = vec![format!(
        "SET LOCAL search_path TO {}",
        server_ident(&at.schema)
    )];
    statements.extend(CreateTableBuilder::new(schema, &at.table).build_postgres());

    let mut conn = pool.connect_direct().await.map_err(|e| refused(at, &e))?;
    let tx = conn.conn.transaction().await.map_err(|e| refused(at, &e))?;
    let held: bool = tx
        .query_one(RELATION_EXISTS, &[&at.schema, &at.table])
        .await
        .map_err(|e| refused(at, &e))?
        .get(0);
    if held {
        return Ok(false);
    }
    tx.batch_execute(&statements.join(";\n"))
        .await
        .map_err(|e| refused(at, &e))?;
    tx.commit().await.map_err(|e| refused(at, &e))?;
    Ok(true)
}

/// Take a just-created relation back off the server — the CTAS rollback, so a failed insert
/// leaves no schema-only husk under a name the user thinks holds data.
///
/// Only ever reached for a relation [`create`] reported making, which is what makes an
/// unconditional drop safe. Best effort and logged rather than reported: the error the user is
/// owed is the insert's, and a drop that also failed would replace it with a sentence about
/// cleanup.
pub(super) async fn discard(pool: &Arc<PostgresConnectionPool>, at: &RemoteTarget) {
    let sql = format!("DROP TABLE IF EXISTS {}", server_relation(at));
    let dropped = match pool.connect_direct().await {
        Ok(conn) => conn
            .conn
            .batch_execute(&sql)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = dropped {
        tracing::warn!(
            "could not remove '{}' after its CREATE TABLE AS failed ({e}); it is empty",
            at.address()
        );
    }
}

/// The relation a CTAS created, removed on **every** way out that is not a settled fill — an
/// error, and a **cancel**.
///
/// The cancel is why this is a guard rather than an `if filled.is_err()`: a CTAS is registered as
/// the workspace's in-flight call, so `Engine::cancel` and a re-press both abort the task, and an
/// aborted task's future is *dropped* at its next await — no error path runs. Without this, every
/// cancelled remote CTAS would leave an empty table on the server under the name the user chose,
/// and the retry would then refuse it as already existing. `ddl::tables::Staging` is the same
/// guard for the local half, for the same reason.
///
/// The drop is **async**, so it is spawned rather than performed: the future is being dropped on
/// the engine runtime, which is where `Handle::current` resolves. Best effort, exactly as the
/// local guard's `remove_dir_all` is — a runtime already shutting down may never run it.
pub(super) struct Created {
    pool: Arc<PostgresConnectionPool>,
    at: RemoteTarget,
    armed: bool,
}

impl Created {
    pub(super) fn open(pool: Arc<PostgresConnectionPool>, at: RemoteTarget) -> Self {
        Created {
            pool,
            at,
            armed: true,
        }
    }

    /// The awaits that could be cancelled are behind us, so the caller's own paths own the
    /// relation from here — including its deterministic rollback.
    pub(super) fn settled(&mut self) {
        self.armed = false;
    }
}

impl Drop for Created {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (pool, at) = (Arc::clone(&self.pool), self.at.clone());
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move { discard(&pool, &at).await });
            }
            Err(e) => tracing::warn!(
                "could not remove '{}' after its CREATE TABLE AS was cancelled ({e}); it is empty",
                self.at.address()
            ),
        }
    }
}

/// A round trip that did not happen, named by the relation it was about.
fn refused(at: &RemoteTarget, e: &impl std::fmt::Display) -> String {
    format!("Cannot write to '{}': {e}", at.address())
}

/// **The shape [`collapse_projections`] exists for, pinned on both sides** — that DataFusion's
/// `INSERT` planner still produces it, and that one `OptimizeProjections` pass still removes it.
///
/// Its own test rather than only the integration test's insert-from-a-remote-source, because that
/// one fails twelve minutes away in another binary and says only that a server rejected some SQL.
/// What can actually move under this is DataFusion: a planner that stops nesting the projections
/// makes the first assertion fail (the collapse becomes dead weight), and an `OptimizeProjections`
/// that stops merging them makes the second fail (the unparse breaks again). Neither needs a
/// database to notice, so neither should wait for one.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::datasource::empty::EmptyTable;

    use super::*;

    /// A session with a source and a target whose columns differ in name — which is what makes
    /// the planner add its renaming projection, and is the real statement's shape
    /// (`INSERT INTO pg.public.loaded SELECT name, id FROM pg.public.customers`).
    fn session() -> SessionContext {
        let ctx = crate::build_context(&BTreeMap::new());
        for (name, columns) in [("source", ["name", "id"]), ("target", ["tier", "total"])] {
            let schema = Arc::new(Schema::new(vec![
                Field::new(columns[0], DataType::Utf8, true),
                Field::new(columns[1], DataType::Int32, true),
            ]));
            ctx.register_table(name, Arc::new(EmptyTable::new(schema)))
                .expect("table");
        }
        ctx
    }

    /// How many `Projection` nodes `plan` holds, root included.
    fn projections(plan: &LogicalPlan) -> usize {
        let here = usize::from(matches!(plan, LogicalPlan::Projection(_)));
        plan.inputs().iter().map(|i| projections(i)).sum::<usize>() + here
    }

    #[tokio::test]
    async fn an_inserts_input_arrives_as_nested_projections_and_leaves_as_one() {
        let ctx = session();
        let plan = ctx
            .state()
            .create_logical_plan("INSERT INTO target SELECT name, id FROM source")
            .await
            .expect("planned");
        let LogicalPlan::Dml(dml) = &plan else {
            panic!("{plan:?}");
        };

        assert_eq!(
            projections(&dml.input),
            2,
            "the planner still stacks its renaming projection on the query's own: {}",
            dml.input.display_indent()
        );

        let collapsed = collapse_projections(&dml.input).expect("collapsed");
        assert_eq!(
            projections(&collapsed),
            1,
            "and the pair the unparser cannot render is gone: {}",
            collapsed.display_indent()
        );
        assert_eq!(
            collapsed.schema(),
            dml.input.schema(),
            "without moving what the sink is handed"
        );
    }
}
