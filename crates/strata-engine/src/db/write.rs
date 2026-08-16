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
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::SessionContext;
use datafusion::sql::TableReference;
use datafusion_table_providers_common::sql::arrow_sql_gen::statement::CreateTableBuilder;
use datafusion_table_providers_postgres::arrow_sql_gen::statement_ext::CreateTableBuilderPostgresExt;
use datafusion_table_providers_postgres::pool::PostgresConnectionPool;
use datafusion_table_providers_postgres::write::PostgresTableWriter;
use datafusion_table_providers_postgres::Postgres;

use crate::sink::append_rows;
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

    /// The address as a **plan** renders it: every part bare, dots between, which is what
    /// `TableReference::to_string` gives and therefore the spelling `PlanDeps::remote` holds.
    /// Never [`address`](Self::address), which quotes a part that needs it and so matches nothing
    /// the moment a schema or relation is not a plain lowercase word.
    pub fn dotted(&self) -> String {
        format!("{}.{}.{}", self.catalog, self.schema, self.table)
    }

    /// The address as the **server** knows it, `schema.relation` — what a report about a
    /// statement the server ran names, the catalog being Strata's word for the connection and
    /// already in that report's other half ("on 'pg'").
    pub fn server_address(&self) -> String {
        qualified([self.schema.as_str(), self.table.as_str()])
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
///
/// The rule is about identifiers **Strata composes**: a dispatched statement carries the parts
/// the user typed exactly as typed, for the server to judge, and only what the splice renders
/// itself comes through here.
pub(crate) fn server_ident(name: &str) -> String {
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
/// The writer is the only part of this the remote arm owns; the drive is
/// [`append_rows`](crate::sink::append_rows), shared with the workspace arm.
pub(super) async fn append(
    ctx: &SessionContext,
    pool: &Arc<PostgresConnectionPool>,
    at: &RemoteTarget,
    provider: Arc<dyn TableProvider>,
    schema: SchemaRef,
    input: &LogicalPlan,
) -> Result<u64, String> {
    let target = Postgres::new(
        at.relation(),
        Arc::clone(pool),
        schema,
        Constraints::default(),
    );
    append_rows(
        ctx,
        PostgresTableWriter::create(provider, target, None),
        input,
    )
    .await
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
