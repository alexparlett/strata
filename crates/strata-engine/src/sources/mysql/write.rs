//! **Writing into a `MySQL` data source** — the two statements DataFusion can plan against a
//! remote relation (`INSERT INTO my.shop.events SELECT …` and `CREATE TABLE my.shop.report AS
//! SELECT …`), and the rollback that makes the second one safe.
//!
//! **The existence answer is the server's own name collision, not a transaction.** This is the one
//! place the `PostgreSQL` arm and this one settle the same question differently, and it is worth
//! reading once. Postgres asks whether the relation is there *inside the create's transaction*,
//! because the statement it builds hardcodes `IF NOT EXISTS` and a two-round-trip check could go
//! stale between the asking and the creating — which would let a failed fill drop a table this
//! statement never made. `MySQL` cannot copy that: its DDL commits implicitly, so there is no
//! transaction to ask inside. It does not need to. A `CREATE TABLE` **without** `IF NOT EXISTS`
//! either creates the relation or fails with errno 1050, and that is a test-and-set the server
//! performs atomically on its own namespace. So a `true` from [`create`] is the same promise it is
//! on Postgres — the rollback only ever removes our own work — reached in one round trip instead
//! of three. The statement is therefore built here rather than by the provider crate's
//! `CreateTableBuilder`, which hardcodes both the `IF NOT EXISTS` and a bare table name.
//!
//! **Both of those are upstream constraints, and neither is in `UPSTREAM_REPORTS.md`** — which is
//! a statement about that file rather than about the crate. It holds *correctness* bugs worked
//! around here and fixed there, each with a reproducer; these are a public API that fixes its
//! collision policy and its name shape for every caller. The asymmetry is the crate's own —
//! `InsertBuilder::new` takes a `TableReference` a screen away from `CreateTableBuilder::new`
//! taking a `&str` — and it is what makes the `PostgreSQL` arm reach for `SET LOCAL search_path`.
//! This arm cannot (`USE` is not transactional and would outlive the write on a pooled
//! connection), so it composes the statement instead, which answers the bare name and lets it drop
//! the `IF NOT EXISTS` in one move. Nothing here needs upstream to change: twenty lines is less to
//! carry than two new upstream APIs and the pin that would bring them.
//!
//! Errno 1050 itself is not a workaround for anything. It is the server's documented answer to a
//! create it cannot perform, used as intended.
//!
//! **Every statement here names the relation in full**, which is the other thing the provider
//! crate cannot do for us: its `MySQL` write helper holds a bare table name and leans on the
//! connection's default database. A Strata source is a whole **server**, so there is no default
//! database to lean on — `USE` would have to be run on a pooled connection and would outlive the
//! write. `InsertBuilder` takes a `TableReference`, and a `Partial` one renders ``  `db`.`table` ``,
//! so the sink below is the crate's own shape with the name it could not carry.
//!
//! **The write provider is resolved by the arm and lives nowhere.** [`writer`] *wraps* the
//! federated read provider, so the node a plan sees is the writer rather than the
//! `FederatedTableProviderAdaptor` the federation rule's downcast walk looks for. Serving writers
//! from the schema provider would forfeit pushdown on **every read**.
//!
//! **The schema the sink validates against is the caller's, because the two statements know
//! different things.** An `INSERT` reaches this with an input DataFusion has already coerced to the
//! *target's* schema, so the server's own is the right one to check. A CTAS created the table from
//! its input's schema a moment ago, so that is what its batches must match; the values reach the
//! server as literals, which it coerces into the columns it made.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::{DataFusionError, Result as DfResult};
use datafusion::datasource::sink::{DataSink, DataSinkExec};
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::dml::InsertOp;
use datafusion::logical_expr::{Expr, TableType};
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion::physical_plan::{DisplayAs, DisplayFormatType, ExecutionPlan};
use datafusion::sql::TableReference;
use datafusion_table_providers_common::sql::arrow_sql_gen::statement::{
    map_data_type_to_column_type, table_reference_to_sea_table_ref, InsertBuilder,
};
use datafusion_table_providers_mysql::pool::MySQLConnectionPool;
use futures::StreamExt;
use mysql_async::prelude::Queryable;
use mysql_async::TxOpts;
use sea_query::{Alias, ColumnDef, ColumnType, MysqlQueryBuilder, Table};

use super::MyCatalog;
use crate::sources::source::SourceCatalog;
use crate::statements::Remote;

/// `ER_TABLE_EXISTS_ERROR` — the server's answer to a `CREATE TABLE` for a name it already holds,
/// and therefore [`create`]'s "no, and nothing was made".
///
/// The **code**, never the prose: the wording is the server's to change and is localized.
const TABLE_EXISTS: u16 = 1050;

/// `at` as the **server** addresses it: `` `db`.`table` ``, both parts quoted through the source's
/// own [`server_ident`](SourceCatalog::server_ident).
fn server_relation(catalog: &MyCatalog, at: &Remote) -> String {
    format!(
        "{}.{}",
        catalog.server_ident(at.schema()),
        catalog.server_ident(at.table())
    )
}

/// Create the relation `at` from `schema` — the server table a CTAS then fills. `false` means the
/// server already held it and nothing was made.
///
/// See the module docs for why there is no transaction and no `IF NOT EXISTS`.
pub(super) async fn create(
    catalog: &MyCatalog,
    at: &Remote,
    schema: SchemaRef,
) -> Result<bool, String> {
    let sql = create_statement(at, &schema);
    let conn = catalog
        .pool
        .connect_direct()
        .await
        .map_err(|e| refused(at, &e))?;
    let mut held = conn.conn.lock().await;
    let made = held.query_drop(&sql).await;
    drop(held);
    match made {
        Ok(()) => Ok(true),
        Err(e) if already_held(&e) => Ok(false),
        Err(e) => Err(refused(at, &e)),
    }
}

/// Whether the server refused a create because it already holds that name.
fn already_held(e: &mysql_async::Error) -> bool {
    matches!(e, mysql_async::Error::Server(server) if server.code == TABLE_EXISTS)
}

/// The `CREATE TABLE` itself: the target named in full, the columns mapped by the provider crate's
/// own Arrow → `MySQL` table, and **no `IF NOT EXISTS`**.
///
/// A nested Arrow type becomes a `JSON` column, which is the rule the crate's own `build_mysql`
/// applies and the only faithful one available: `MySQL` has no array or struct column.
fn create_statement(at: &Remote, schema: &SchemaRef) -> String {
    let mut create = Table::create();
    create.table(table_reference_to_sea_table_ref(&at.relation()));
    for field in schema.fields() {
        let kind = match field.data_type().is_nested() {
            true => ColumnType::JsonBinary,
            false => map_data_type_to_column_type(field.data_type()),
        };
        let mut column = ColumnDef::new_with_type(Alias::new(field.name()), kind);
        if !field.is_nullable() {
            column.not_null();
        }
        create.col(&mut column);
    }
    create.to_string(MysqlQueryBuilder)
}

/// Take a just-created relation back off the server — the CTAS rollback, so a failed insert leaves
/// no schema-only husk under a name the user thinks holds data.
///
/// Only ever reached for a relation [`create`] reported making, which is what makes an
/// unconditional drop safe.
pub(super) async fn discard(catalog: &MyCatalog, at: &Remote) -> Result<(), String> {
    let sql = format!("DROP TABLE IF EXISTS {}", server_relation(catalog, at));
    let conn = catalog
        .pool
        .connect_direct()
        .await
        .map_err(|e| e.to_string())?;
    let mut held = conn.conn.lock().await;
    let dropped = held.query_drop(&sql).await;
    drop(held);
    dropped.map_err(|e| e.to_string())
}

/// The provider a write statement is planned against: `read`'s scans, and a sink of our own.
pub(super) fn writer(
    catalog: &MyCatalog,
    read: Arc<dyn TableProvider>,
    at: &Remote,
    schema: SchemaRef,
) -> Arc<dyn TableProvider> {
    Arc::new(MyWriter {
        read,
        pool: Arc::clone(&catalog.pool),
        target: at.relation(),
        address: at.address(),
        schema,
    })
}

/// A read provider with somewhere to put rows.
#[derive(Debug)]
struct MyWriter {
    read: Arc<dyn TableProvider>,
    pool: Arc<MySQLConnectionPool>,
    /// The relation as the server addresses it, `db.table`.
    target: TableReference,
    /// The relation as a **message** names it, `source.db.table`.
    address: String,
    /// What the caller says the rows will look like — see the module docs.
    schema: SchemaRef,
}

#[async_trait]
impl TableProvider for MyWriter {
    fn schema(&self) -> SchemaRef {
        self.read.schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> DfResult<Arc<dyn ExecutionPlan>> {
        self.read.scan(state, projection, filters, limit).await
    }

    /// Appending is the only thing this takes.
    ///
    /// `INSERT OVERWRITE` is refused by the statement router long before a provider is built, so
    /// this arm is unreachable through the app; it is written out because a `TableProvider` is a
    /// public seam and "the router happens not to send one" is not a property of this type.
    async fn insert_into(
        &self,
        _state: &dyn Session,
        input: Arc<dyn ExecutionPlan>,
        op: InsertOp,
    ) -> DfResult<Arc<dyn ExecutionPlan>> {
        if op != InsertOp::Append {
            return Err(DataFusionError::Execution(format!(
                "'{}' can only have rows appended to it.",
                self.address
            )));
        }
        Ok(Arc::new(DataSinkExec::new(
            input,
            Arc::new(MySink {
                pool: Arc::clone(&self.pool),
                target: self.target.clone(),
                address: self.address.clone(),
                schema: Arc::clone(&self.schema),
            }),
            None,
        )))
    }
}

/// The rows, on their way to the server: one statement per batch, all of them in one transaction.
struct MySink {
    pool: Arc<MySQLConnectionPool>,
    target: TableReference,
    address: String,
    schema: SchemaRef,
}

#[async_trait]
impl DataSink for MySink {
    fn metrics(&self) -> Option<MetricsSet> {
        None
    }

    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// **One transaction for the whole write**, so a statement that fails halfway leaves the
    /// relation as it was rather than partly filled — which is what lets a CTAS's rollback be the
    /// only cleanup there is.
    ///
    /// An empty batch is skipped rather than sent: `InsertBuilder` refuses to build an `INSERT`
    /// with no rows, and that refusal would be reported as a failed write of nothing.
    async fn write_all(
        &self,
        mut data: SendableRecordBatchStream,
        _context: &Arc<TaskContext>,
    ) -> DfResult<u64> {
        let conn = self
            .pool
            .connect_direct()
            .await
            .map_err(|e| self.failed(&e))?;
        let mut held = conn.conn.lock().await;
        let mut tx = held
            .start_transaction(TxOpts::default())
            .await
            .map_err(|e| self.failed(&e))?;

        let mut rows = 0u64;
        while let Some(batch) = data.next().await {
            let batch = batch?;
            if batch.num_rows() == 0 {
                continue;
            }
            rows += batch.num_rows() as u64;
            let batches = vec![batch];
            let sql = InsertBuilder::new(&self.target, &batches)
                .build_mysql(None)
                .map_err(|e| self.failed(&e))?;
            tx.exec_drop(&sql, ()).await.map_err(|e| self.failed(&e))?;
        }
        tx.commit().await.map_err(|e| self.failed(&e))?;
        Ok(rows)
    }
}

impl MySink {
    /// A write that did not happen, named by the relation it was about.
    fn failed(&self, e: &impl fmt::Display) -> DataFusionError {
        DataFusionError::Execution(format!("Cannot write to '{}': {e}", self.address))
    }
}

impl fmt::Debug for MySink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MySink")
            .field("target", &self.address)
            .finish()
    }
}

impl DisplayAs for MySink {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MySink target={}", self.address)
    }
}

/// A round trip that did not happen, named by the relation it was about.
fn refused(at: &Remote, e: &impl fmt::Display) -> String {
    format!("Cannot write to '{}': {e}", at.address())
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field, Schema};

    use super::*;

    fn at() -> Remote {
        Remote {
            source: "my".into(),
            reference: TableReference::full("my", "shop", "report"),
        }
    }

    /// **The create names the relation in full and does not say `IF NOT EXISTS`** — the two
    /// things the provider crate's builder cannot do, and the second is what makes errno 1050 the
    /// existence answer.
    #[test]
    fn a_create_is_qualified_and_collides_on_purpose() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("note", DataType::Utf8, true),
        ]));
        let sql = create_statement(&at(), &schema);
        assert!(
            sql.starts_with("CREATE TABLE `shop`.`report` ("),
            "the target is named in full, and the catalog is Strata's own word: {sql}"
        );
        assert!(
            !sql.contains("IF NOT EXISTS"),
            "a create that cannot collide cannot answer whether it made anything: {sql}"
        );
        assert!(sql.contains("`id` int NOT NULL"), "{sql}");
        assert!(
            sql.contains("`note` text") && !sql.contains("`note` text NOT NULL"),
            "a nullable column is left nullable: {sql}"
        );
    }

    /// A nested Arrow type has no `MySQL` column, so it becomes `JSON` — the rule the provider
    /// crate applies to its own builder, restated here because this statement is ours.
    #[test]
    fn a_nested_column_becomes_json() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        )]));
        assert!(
            create_statement(&at(), &schema).contains("`tags` json"),
            "{}",
            create_statement(&at(), &schema)
        );
    }

    /// The existence answer is read off the **code**, so a server that rewords or localizes
    /// `ER_TABLE_EXISTS_ERROR` still answers "it was already there" rather than failing the
    /// statement.
    #[test]
    fn the_existence_answer_is_the_errno() {
        let server = |code: u16| {
            mysql_async::Error::Server(mysql_async::ServerError {
                code,
                message: "Table 'report' already exists".into(),
                state: "42S01".into(),
            })
        };
        assert!(already_held(&server(TABLE_EXISTS)));
        assert!(!already_held(&server(1142)), "a denied grant is not a name");
    }
}
