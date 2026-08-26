//! **Writing into a `PostgreSQL` connection** — the two statements DataFusion can plan against a
//! remote relation (`INSERT INTO pg.public.events SELECT …` and `CREATE TABLE pg.public.report AS
//! SELECT …`), and the rollback that makes the second one safe.
//!
//! **The write provider is resolved by the arm and lives nowhere.** `PostgresTableWriter` *wraps*
//! the federated read provider, so the node a plan sees is the writer rather than the
//! `FederatedTableProviderAdaptor` the federation rule's downcast walk looks for. Serving writers
//! from the schema provider would forfeit pushdown on **every read** — the failure the sources
//! layer's own-provider decision exists to prevent — so the catalog goes on serving read providers
//! and a writer is built here, used once and dropped.
//!
//! **The schema the sink validates against is the caller's, because the two statements know
//! different things.** An `INSERT` reaches this with an input DataFusion has already coerced to
//! the *target's* schema, so the server's own is the right one to check. A CTAS created the table
//! from its input's schema a moment ago, so that is what its batches must match; the values reach
//! the server as literals (`InsertBuilder`), which it coerces into the columns it made.

use std::sync::Arc;

use datafusion::arrow::datatypes::SchemaRef;
use datafusion::common::Constraints;
use datafusion_table_providers_common::sql::arrow_sql_gen::statement::CreateTableBuilder;
use datafusion_table_providers_postgres::arrow_sql_gen::statement_ext::CreateTableBuilderPostgresExt;
use datafusion_table_providers_postgres::pool::PostgresConnectionPool;
use datafusion_table_providers_postgres::Postgres;

use crate::sources::source::SourceCatalog;
use crate::statements::Remote;

use super::PgCatalog;

/// Whether one relation is there, by the same catalog the enumeration reads — `pg_class`, so a
/// view or a partitioned table answers as truthfully as an ordinary one.
const RELATION_EXISTS: &str = "\
SELECT EXISTS ( \
  SELECT 1 FROM pg_catalog.pg_class c \
  JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
  WHERE n.nspname = $1 AND c.relname = $2 \
)";

/// `at` as the **server** addresses it: `"schema"."table"`, both parts quoted through the source's
/// own [`server_ident`](SourceCatalog::server_ident).
fn server_relation(catalog: &PgCatalog, at: &Remote) -> String {
    format!(
        "{}.{}",
        catalog.server_ident(at.schema()),
        catalog.server_ident(at.table())
    )
}

/// The sink `PostgresTableWriter` drives — `at` as the server knows it (`schema.relation`, never
/// the catalog, which is Strata's own prefix for the connection and means nothing to Postgres),
/// over `schema`.
pub(super) fn target(
    pool: &Arc<PostgresConnectionPool>,
    at: &Remote,
    schema: SchemaRef,
) -> Postgres {
    Postgres::new(
        at.relation(),
        Arc::clone(pool),
        schema,
        Constraints::default(),
    )
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
    catalog: &PgCatalog,
    at: &Remote,
    schema: SchemaRef,
) -> Result<bool, String> {
    let pool = &catalog.pool;
    let mut statements = vec![format!(
        "SET LOCAL search_path TO {}",
        catalog.server_ident(at.schema())
    )];
    statements.extend(CreateTableBuilder::new(schema, at.table()).build_postgres());

    let mut conn = pool.connect_direct().await.map_err(|e| refused(at, &e))?;
    let tx = conn.conn.transaction().await.map_err(|e| refused(at, &e))?;
    let held: bool = tx
        .query_one(RELATION_EXISTS, &[&at.schema(), &at.table()])
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
/// unconditional drop safe.
pub(super) async fn discard(catalog: &PgCatalog, at: &Remote) -> Result<(), String> {
    let sql = format!("DROP TABLE IF EXISTS {}", server_relation(catalog, at));
    match catalog.pool.connect_direct().await {
        Ok(conn) => conn
            .conn
            .batch_execute(&sql)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// A round trip that did not happen, named by the relation it was about.
fn refused(at: &Remote, e: &impl std::fmt::Display) -> String {
    format!("Cannot write to '{}': {e}", at.address())
}

#[cfg(test)]
mod tests {

    /// **The server's rule, not sqlparser's.** `sql::quote_verbatim` asks `needs_quoting`, whose
    /// reserved set is the words *DataFusion's* parser cannot read as a name; a `PostgreSQL` server
    /// reserves far more (`user`, `table`, `default`, `check`, `primary`, `column`, `constraint`,
    /// `references`, `unique`, `grant`, none of them in either sqlparser list), so a relation
    /// called `user` would go out bare and the statement would be a syntax error — which, for
    /// [`discard`], means the rollback silently fails and leaves the husk it exists to remove.
    #[test]
    fn every_identifier_this_source_composes_is_quoted() {
        assert_eq!(
            quoted("user"),
            "\"user\"",
            "a reserved word the local renderer would have written bare"
        );
        assert_eq!(quoted("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    /// The trait's own rule, which this source does not override — asked of it directly, because
    /// building a handle needs a server.
    fn quoted(name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}
