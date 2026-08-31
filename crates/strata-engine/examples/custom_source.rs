//! A data source of your own: written, registered, run through the conformance ring, and queried.
//!
//! ```text
//! cargo run -p strata-engine --features testing --example custom_source
//! ```
//!
//! It speaks no SQL at all — the relations are read through a provider it builds itself — which is
//! the harder half of the seam to believe until you see it. Everything a statement would need
//! refuses through the trait's own defaults, and the engine still gives it bare-name resolution,
//! `SHOW TABLES`, completion and the catalog tree. The guide is
//! `strata_engine::guide::data_source`.

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::array::{Int32Array, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::{MemTable, TableProvider};

use strata_engine::secrets::SecretProvider;
use strata_engine::sources::source::{
    DataSource, Field as SettingField, Listing, Located, Relation, SourceCatalog, SourceKind,
    SourceMode, SourceSetting, Sourced,
};
use strata_engine::testing::conforms;
use strata_engine::{Engine, RunOutcome, RunTag, WsId};
use strata_model::SourceDef;

/// The shape every relation this source holds has.
fn ledger_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("account", DataType::Utf8, false),
        Field::new("balance", DataType::Int32, true),
    ]))
}

/// A source with no server behind it: one relation, held as batches.
#[derive(Debug, Default)]
struct Ledger;

impl SourceKind for Ledger {
    const NAME: &'static str = "ledger";
    const LABEL: &'static str = "Ledger";
    const BADGE: &'static str = "LDG";
    const MODE: SourceMode = SourceMode::Catalog;
}

/// One setting, drawn by the connection editor as a row. What a value may *be* is
/// [`Ledger::connect`]'s to judge; the declaration only says what to draw.
const LEDGER_SETTINGS: &[SourceSetting] = &[SourceSetting {
    key: "address",
    label: "ADDRESS",
    field: SettingField::Text,
    group: None,
    required: true,
    default: None,
    when: None,
    hint: Some("Where the ledger lives"),
    placeholder: None,
}];

#[async_trait]
impl DataSource for Ledger {
    fn settings(&self) -> &'static [SourceSetting] {
        LEDGER_SETTINGS
    }

    /// Opens the handle, and **probes it**: a description that is well-formed and wrong has to
    /// fail here, or every table under it fails later in somebody else's words.
    async fn connect(
        &self,
        def: &SourceDef,
        _secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, String> {
        match def.setting("address").trim() {
            "books" => Ok(Sourced::Catalog(Arc::new(LedgerBooks))),
            other => Err(format!("There is no ledger at '{other}'.")),
        }
    }
}

/// A connected [`Ledger`].
#[derive(Debug)]
struct LedgerBooks;

#[async_trait]
impl SourceCatalog for LedgerBooks {
    fn kind(&self) -> &'static str {
        Ledger::NAME
    }

    async fn enumerate(&self) -> Result<Listing, String> {
        Ok(Listing::of([(
            "public".to_string(),
            Relation {
                name: "accounts".to_string(),
                view: false,
            },
        )]))
    }

    async fn table_provider(
        self: Arc<Self>,
        at: &Located,
    ) -> Result<Arc<dyn TableProvider>, String> {
        if at.relation.table() != "accounts" {
            return Err(format!("no relation '{}'", at.relation));
        }
        let batch = RecordBatch::try_new(
            ledger_schema(),
            vec![
                Arc::new(StringArray::from(vec!["cash", "receivables"])),
                Arc::new(Int32Array::from(vec![1200, 340])),
            ],
        )
        .map_err(|e| e.to_string())?;
        let table =
            MemTable::try_new(ledger_schema(), vec![vec![batch]]).map_err(|e| e.to_string())?;
        Ok(Arc::new(table))
    }
}

/// A def naming this source, as a project file would hold one.
fn ledger_def() -> SourceDef {
    SourceDef {
        config: BTreeMap::from([("address".to_string(), "books".to_string())]),
        kind: Ledger::NAME.to_string(),
        name: "acme".to_string(),
        schemas: vec!["public".to_string()],
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // The generic ring, run against your own registrant exactly as it is run against the shipped
    // ones. It panics on the first thing the source does not keep.
    futures::executor::block_on(conforms(Ledger, &ledger_def()));
    println!("'{}' keeps the contract\n", Ledger::NAME);

    // Registered like any shipped source; a def naming its kind connects through the registry.
    let engine = Engine::builder().with_source(Ledger).build();
    futures::executor::block_on(engine.sources().connect(ledger_def()))?;

    let outcome = futures::executor::block_on(engine.ws(WsId(1)).run(
        RunTag(1),
        // Three-part, because the source registered a catalog under its own name. A bare
        // `accounts` resolves here too — the engine searches the connected databases for a name
        // the workspace does not hold.
        "SELECT account, balance FROM acme.public.accounts ORDER BY balance DESC".into(),
        100,
    ))?;
    let RunOutcome::Rows(run) = outcome else {
        return Err("a SELECT settles rows".into());
    };
    for row in &run.output.rows {
        let cells: Vec<&str> = row.iter().map(|cell| cell.text.as_str()).collect();
        println!("  {}", cells.join("  "));
    }

    Ok(())
}
