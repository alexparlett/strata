//! Two data sources with no server behind them, and the contract every source is judged by.
//!
//! [`TestDoc`] speaks no SQL at all: it reads through a provider of its own, and everything a
//! statement would need refuses through the trait's own defaults — the generic ring, proven by
//! something that is not a database. [`TestSql`] composes [`sql`](super::sql) over an in-memory
//! session, so a plan really is unparsed into a statement and that statement really is parsed and
//! run on the other side of the executor — the SQL ring, without a container.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use datafusion::arrow::array::{Int32Array, Int64Array};
use datafusion::arrow::datatypes::{DataType, Field as ArrowField, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::{MemTable, TableProvider};
use datafusion::common::{DataFusionError, Result as DfResult};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{PhysicalExpr, SendableRecordBatchStream};
use datafusion::prelude::SessionContext;
use datafusion::sql::unparser::dialect::{DefaultDialect, Dialect};
use datafusion::sql::TableReference;
use futures::TryStreamExt;

use strata_model::{ConnectionDef, Provider, SourceDef};

use super::source::{
    ConnectionKey, DataSource, Field, Listing, Located, Relation, SourceCatalog, SourceKind,
    SourceMode, Sourced,
};
use super::sql::{federated, SQLExecutor, SqlSpec};
use crate::secrets::SecretProvider;

/// The two columns every fake relation has, so a test can join one to a workspace table on `id`
/// and still have something to project.
pub(crate) fn fake_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        ArrowField::new("id", DataType::Int32, false),
        ArrowField::new("total", DataType::Int64, true),
    ]))
}

/// A def for `catalog`, served by `S` at an address of its own.
pub(crate) fn fake_def<S: SourceKind>(catalog: &str, address: &str) -> ConnectionDef {
    ConnectionDef {
        address: address.to_string(),
        name: catalog.to_string(),
        provider: Provider::Source(SourceDef {
            kind: S::NAME.to_string(),
            schemas: vec!["public".to_string()],
            ..Default::default()
        }),
        client_config: BTreeMap::new(),
    }
}

/// What a fake connection holds: the relations it was told to have, as batches.
#[derive(Debug)]
pub(crate) struct Rows(BTreeMap<String, Vec<RecordBatch>>);

impl Rows {
    /// A connected [`TestDoc`] holding `relations`, for a fixture that registers a catalog
    /// directly rather than connecting one.
    pub(crate) fn catalog(relations: &[&str]) -> (Arc<dyn SourceCatalog>, Listing) {
        let rows = Arc::new(Rows::of(relations));
        let listing = rows.listing();
        (Arc::new(DocCatalog(rows)), listing)
    }
}

impl Rows {
    /// One relation per name, each holding two rows.
    fn of(relations: &[&str]) -> Self {
        let batch = RecordBatch::try_new(
            fake_schema(),
            vec![
                Arc::new(Int32Array::from(vec![1, 2])),
                Arc::new(Int64Array::from(vec![10, 20])),
            ],
        )
        .expect("a fixture batch");
        Rows(
            relations
                .iter()
                .map(|name| ((*name).to_string(), vec![batch.clone()]))
                .collect(),
        )
    }

    fn listing(&self) -> Listing {
        Listing::of(self.0.keys().map(|name| {
            (
                "public".to_string(),
                Relation {
                    name: name.clone(),
                    view: false,
                },
            )
        }))
    }

    fn provider(&self, relation: &str) -> Option<Arc<dyn TableProvider>> {
        let batches = self.0.get(relation)?.clone();
        let table = MemTable::try_new(fake_schema(), vec![batches]).expect("a fixture table");
        Some(Arc::new(table))
    }
}

/// A source with no SQL: its relations are read through its own provider, and everything a
/// statement would need refuses through the trait's own defaults.
#[derive(Debug, Default)]
pub(crate) struct TestDoc {
    /// What each address holds, so one registered value serves several connections.
    connections: Mutex<BTreeMap<String, Arc<Rows>>>,
}

impl TestDoc {
    /// A source that answers `address` with `relations`.
    pub(crate) fn holding(address: &str, relations: &[&str]) -> Self {
        let held = Self::default();
        held.connections
            .lock()
            .unwrap()
            .insert(address.to_string(), Arc::new(Rows::of(relations)));
        held
    }

    fn rows(&self, address: &str) -> Result<Arc<Rows>, String> {
        self.connections
            .lock()
            .unwrap()
            .get(address.trim())
            .map(Arc::clone)
            .ok_or_else(|| format!("no fake source at '{address}'"))
    }
}

impl SourceKind for TestDoc {
    const NAME: &'static str = "test-doc";
    const LABEL: &'static str = "Test document store";
    const BADGE: &'static str = "DOC";
    const MODE: SourceMode = SourceMode::Catalog;
}

/// One key, so a test has something to assert a declaration reaches a surface with.
const DOC_KEYS: &[ConnectionKey] = &[ConnectionKey {
    key: "collection_prefix",
    label: "PREFIX",
    field: Field::Text,
    required: false,
    default: None,
}];

#[async_trait]
impl DataSource for TestDoc {
    async fn connect(
        &self,
        def: &ConnectionDef,
        _secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, String> {
        Ok(Sourced::Catalog(Arc::new(DocCatalog(
            self.rows(&def.address)?,
        ))))
    }

    fn config_keys(&self) -> &'static [ConnectionKey] {
        DOC_KEYS
    }
}

/// A connected [`TestDoc`].
#[derive(Debug)]
pub(crate) struct DocCatalog(Arc<Rows>);

#[async_trait]
impl SourceCatalog for DocCatalog {
    fn kind(&self) -> &'static str {
        TestDoc::NAME
    }

    async fn enumerate(&self) -> Result<Listing, String> {
        Ok(self.0.listing())
    }

    async fn table_provider(
        self: Arc<Self>,
        at: &Located,
    ) -> Result<Arc<dyn TableProvider>, String> {
        self.0
            .provider(at.relation.table())
            .ok_or_else(|| format!("no relation '{}'", at.relation))
    }
}

/// A SQL-speaking source whose server is a `SessionContext`: a federated plan is unparsed into a
/// statement, and that statement is parsed and run here.
#[derive(Debug, Default)]
pub(crate) struct TestSql {
    connections: Mutex<BTreeMap<String, Arc<Rows>>>,
}

impl TestSql {
    pub(crate) fn holding(address: &str, relations: &[&str]) -> Self {
        let held = Self::default();
        held.connections
            .lock()
            .unwrap()
            .insert(address.to_string(), Arc::new(Rows::of(relations)));
        held
    }
}

impl SourceKind for TestSql {
    const NAME: &'static str = "test-sql";
    const LABEL: &'static str = "Test SQL server";
    const BADGE: &'static str = "SQL";
    const MODE: SourceMode = SourceMode::Catalog;
}

#[async_trait]
impl DataSource for TestSql {
    async fn connect(
        &self,
        def: &ConnectionDef,
        _secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, String> {
        let rows = self
            .connections
            .lock()
            .unwrap()
            .get(def.address.trim())
            .map(Arc::clone)
            .ok_or_else(|| format!("no fake source at '{}'", def.address))?;
        Ok(Sourced::Catalog(Arc::new(SqlCatalog(rows))))
    }
}

/// A connected [`TestSql`].
#[derive(Debug)]
pub(crate) struct SqlCatalog(Arc<Rows>);

#[async_trait]
impl SourceCatalog for SqlCatalog {
    fn kind(&self) -> &'static str {
        TestSql::NAME
    }

    async fn enumerate(&self) -> Result<Listing, String> {
        Ok(self.0.listing())
    }

    async fn table_provider(
        self: Arc<Self>,
        at: &Located,
    ) -> Result<Arc<dyn TableProvider>, String> {
        let provider = self
            .0
            .provider(at.relation.table())
            .ok_or_else(|| format!("no relation '{}'", at.relation))?;
        let server = SessionContext::new();
        server
            .register_table(at.relation.table(), Arc::clone(&provider))
            .map_err(|e| e.to_string())?;
        Ok(federated(
            self,
            SqlSpec {
                dialect: Arc::new(DefaultDialect {}),
                executor: Arc::new(MemExecutor {
                    server,
                    relation: at.relation.table().to_string(),
                }),
                provider,
            },
            at,
        ))
    }
}

/// Runs the statement federation composed, against an in-memory session holding the relation.
struct MemExecutor {
    server: SessionContext,
    relation: String,
}

impl std::fmt::Debug for MemExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemExecutor")
            .field("relation", &self.relation)
            .finish()
    }
}

#[async_trait]
impl SQLExecutor for MemExecutor {
    fn name(&self) -> &str {
        TestSql::NAME
    }

    /// `None`, so what a federated plan fuses on is whatever the assembly stamps rather than
    /// anything this fake decided.
    fn compute_context(&self) -> Option<String> {
        None
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        Arc::new(DefaultDialect {})
    }

    fn execute(
        &self,
        query: &str,
        schema: SchemaRef,
        _filters: &[Arc<dyn PhysicalExpr>],
    ) -> DfResult<SendableRecordBatchStream> {
        let server = self.server.clone();
        let sql = query.to_string();
        let batches = futures::stream::once(async move {
            let frame = server.sql(&sql).await?;
            let batches = frame.collect().await?;
            Ok::<_, DataFusionError>(futures::stream::iter(batches.into_iter().map(Ok)))
        })
        .try_flatten();
        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, batches)))
    }

    async fn table_names(&self) -> DfResult<Vec<String>> {
        Ok(vec![self.relation.clone()])
    }

    async fn get_table_schema(&self, _table_name: &str) -> DfResult<SchemaRef> {
        Ok(fake_schema())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemSecrets;
    use crate::sources::source::{unsupported, Sources};
    use crate::statements::Remote;
    use crate::{Engine, RunTag, WsId};

    fn secrets() -> Arc<dyn SecretProvider> {
        Arc::new(MemSecrets::new())
    }

    /// **The contract every source keeps**, run against each of them: connecting yields the mode
    /// the kind declared, the handle names the kind it was registered under, and what it does not
    /// implement refuses in the trait's own words rather than in an arm's.
    async fn conforms<S: DataSource + SourceKind>(source: S, def: &ConnectionDef) {
        let mode = S::MODE;
        let kind = S::NAME;
        let connected = source.connect(def, secrets()).await.expect("a fixture");
        let catalog = match (connected, mode) {
            (Sourced::Catalog(catalog), SourceMode::Catalog) => catalog,
            (Sourced::Store { .. }, SourceMode::Store) => return,
            _ => panic!("'{kind}' connected as something other than the mode it declares"),
        };
        assert_eq!(catalog.kind(), kind, "the handle names its own kind");

        let listing = catalog.enumerate().await.expect("an enumeration");
        assert!(
            !listing.schemas().is_empty(),
            "'{kind}' enumerated nothing to read"
        );
        let at = Remote {
            connection: "fixture".into(),
            reference: TableReference::full("fixture", "public", "orders"),
        };
        if let Err(why) = catalog.execute_text("SELECT 1").await {
            assert_eq!(
                why,
                unsupported(kind, "run a statement of its own"),
                "a refusal a source does not word itself is the trait's own"
            );
        }
        if let Err(why) = catalog.create_relation(&at, fake_schema()).await {
            assert_eq!(why, unsupported(kind, "have relations created in it"));
        }
        if let Err(why) = catalog.writer(
            catalog
                .clone()
                .table_provider(&Located {
                    connection: "fixture".into(),
                    identity: def.identity(),
                    relation: "public.orders".into(),
                })
                .await
                .expect("a read provider"),
            &at,
            fake_schema(),
        ) {
            assert_eq!(why, unsupported(kind, "be written to"));
        }
    }

    #[tokio::test]
    async fn every_test_source_keeps_the_contract() {
        conforms(
            TestDoc::holding("docs", &["orders"]),
            &fake_def::<TestDoc>("docs", "docs"),
        )
        .await;
        conforms(
            TestSql::holding("server", &["orders"]),
            &fake_def::<TestSql>("sales", "server"),
        )
        .await;
    }

    /// **A source with no SQL, end to end**: registered under its own name, connected through the
    /// registry, enumerated, and read through a provider it built itself.
    #[tokio::test]
    async fn a_document_source_connects_enumerates_and_reads() {
        let engine = Engine::builder()
            .with_source(TestDoc::holding("docs", &["orders", "events"]))
            .build();
        engine
            .sources()
            .connect(fake_def::<TestDoc>("docs", "docs"))
            .await
            .expect("the fixture connects");

        let rows = engine
            .ws(WsId(1))
            .query(
                RunTag(1),
                "SELECT id FROM docs.public.orders ORDER BY 1".into(),
                10,
            )
            .await
            .expect("a read through the source's own provider")
            .output;
        assert_eq!(
            rows.rows
                .iter()
                .map(|row| row[0].text.clone())
                .collect::<Vec<_>>(),
            vec!["1".to_string(), "2".to_string()]
        );
    }

    /// **A SQL source, end to end**: the same path, with the read leaving as a statement the fake
    /// server parses — which is what the assembly is for.
    #[tokio::test]
    async fn a_sql_source_reads_through_a_federated_statement() {
        let engine = Engine::builder()
            .with_source(TestSql::holding("server", &["orders"]))
            .build();
        engine
            .sources()
            .connect(fake_def::<TestSql>("sales", "server"))
            .await
            .expect("the fixture connects");

        let plan = engine
            .ws(WsId(1))
            .explain(
                RunTag(1),
                "SELECT id FROM sales.public.orders WHERE id = 1".into(),
            )
            .await
            .expect("a plan")
            .physical_text;
        assert!(
            plan.contains("VirtualExecutionPlan"),
            "the scan did not federate:\n{plan}"
        );
    }

    /// **Two connections of one kind never share plan-cache identity**, which is what stops a
    /// statement fused across them being sent to whichever executor won. The assembly stamps it,
    /// so a source that composes it cannot forget.
    #[tokio::test]
    async fn two_connections_of_one_kind_are_two_compute_contexts() {
        let mut source = TestSql::default();
        for address in ["north", "south"] {
            source
                .connections
                .get_mut()
                .unwrap()
                .insert(address.to_string(), Arc::new(Rows::of(&["orders"])));
        }
        let engine = Engine::builder().with_source(source).build();
        for (catalog, address) in [("north", "north"), ("south", "south")] {
            engine
                .sources()
                .connect(fake_def::<TestSql>(catalog, address))
                .await
                .expect("both connect");
        }

        let contexts: Vec<String> = ["north", "south"]
            .iter()
            .map(|catalog| {
                let plan = futures::executor::block_on(
                    engine
                        .ws(WsId(1))
                        .explain(RunTag(1), format!("SELECT id FROM {catalog}.public.orders")),
                )
                .expect("a plan")
                .physical_text;
                plan.lines()
                    .find_map(|line| {
                        line.split("compute_context=")
                            .nth(1)
                            .map(|rest| rest.split(' ').next().unwrap_or_default().to_string())
                    })
                    .unwrap_or_else(|| panic!("no compute_context in:\n{plan}"))
            })
            .collect();
        assert_ne!(
            contexts[0], contexts[1],
            "two connections of one kind fused into one"
        );
    }

    /// A def naming a kind nothing is registered for settles as a failure naming the fix — no
    /// panic, and no parse error either: the def is well-formed, the engine simply has nothing to
    /// serve it with.
    #[tokio::test]
    async fn an_unregistered_kind_fails_the_connection_and_names_the_fix() {
        let engine = Engine::builder().build();
        let why = engine
            .sources()
            .connect(fake_def::<TestDoc>("docs", "docs"))
            .await
            .expect_err("nothing serves 'test-doc'")
            .to_string();
        assert!(
            why.contains("'test-doc'") && why.contains("with_source"),
            "{why}"
        );
    }

    /// What a picker, a badge and a form read is one list off the registry, and a source an
    /// embedder registered is on it on the same terms as a shipped one.
    #[test]
    fn the_registrants_read_answers_for_every_registered_source() {
        let engine = Engine::builder()
            .with_source(TestDoc::holding("docs", &["orders"]))
            .build();
        let doc = engine
            .sources()
            .registrants()
            .into_iter()
            .find(|info| info.kind == TestDoc::NAME)
            .expect("the registered source is offered");
        assert_eq!(doc.label, TestDoc::LABEL);
        assert_eq!(doc.badge, TestDoc::BADGE);
        assert_eq!(doc.mode, SourceMode::Catalog);
        assert_eq!(doc.keys, DOC_KEYS, "the form draws the source's own keys");
        assert_eq!(
            engine
                .sources()
                .check_address(TestDoc::NAME, "")
                .map_err(|e| e.to_string()),
            Err("This connection has no address.".into()),
            "the default address rule is reached through the registry"
        );
    }

    /// The registry is one map: a source registered under a name another holds replaces it, which
    /// is how an embedder substitutes their own for a shipped one.
    #[test]
    fn a_second_registration_of_one_name_replaces_the_first() {
        let mut sources = Sources::default();
        sources.insert(TestDoc::holding("first", &["orders"]));
        sources.insert(TestDoc::holding("second", &["events"]));
        assert_eq!(sources.registrants().len(), 1);
    }
}
