//! The in-RAM store: one set of batches per table, served through DataFusion `MemTable`s built
//! fresh per scan, and nothing on disk.
//!
//! **Tests and ephemeral workspaces only, and the caveat is durability**: an internal table's
//! *def* is written into `project.json` and outlives the process, while everything this store
//! holds dies with it — so a restart replays defs against data that is gone, and each lands as
//! an honest `Failed` row naming the missing data rather than a fault. The shipped default
//! ([`LocalIpcTableStore`](super::LocalIpcTableStore)) is what makes the def's promise true
//! across restarts. (A def whose data an *earlier* engine spooled to disk still replays from
//! those files: this store answers nothing for the slug, and registration falls back to the
//! def's own resolved paths.)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::MemTable;
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::execution::SendableRecordBatchStream;
use datafusion::logical_expr::{Expr, TableType};
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::SessionContext;
use futures::StreamExt;

use crate::tables::InternalTableStore;

/// Tables held in RAM for the store's lifetime, keyed by slug.
#[derive(Default)]
pub struct MemTableStore {
    held: Arc<Mutex<HashMap<String, Held>>>,
}

/// One table's rows: the schema its batches conform to, and one inner vec per
/// [`append`](InternalTableStore::append)ed unit — the create's own rows are the first.
#[derive(Debug)]
struct Held {
    schema: SchemaRef,
    units: Vec<Vec<RecordBatch>>,
}

impl MemTableStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl InternalTableStore for MemTableStore {
    /// Atomic by construction: the rows are drained first and the map write publishes them in
    /// one step, so a reader observes the previous table or the new one and never a half.
    async fn create(&self, slug: &str, rows: SendableRecordBatchStream) -> Result<u64, String> {
        let schema = rows.schema();
        let (unit, count) = drained(rows).await?;
        self.held.lock().unwrap().insert(
            slug.to_string(),
            Held {
                schema,
                units: vec![unit],
            },
        );
        Ok(count)
    }

    async fn append(&self, slug: &str, rows: SendableRecordBatchStream) -> Result<u64, String> {
        let (unit, count) = drained(rows).await?;
        match self.held.lock().unwrap().get_mut(slug) {
            Some(held) => held.units.push(unit),
            None => return Err(format!("no table data to append to under '{slug}'")),
        }
        Ok(count)
    }

    /// The provider reads the slot **per scan** rather than snapshotting it — the
    /// append-visibility rule the module contract states, and the reason this is not the
    /// `MemTable` itself, which captures its batches when it is built.
    async fn provider(
        &self,
        _ctx: &SessionContext,
        slug: &str,
    ) -> Result<Option<Arc<dyn TableProvider>>, String> {
        let held = self.held.lock().unwrap();
        Ok(held.get(slug).map(|table| {
            Arc::new(MemProvider {
                slug: slug.to_string(),
                seeded: table.schema.clone(),
                held: Arc::clone(&self.held),
            }) as Arc<dyn TableProvider>
        }))
    }

    async fn discard(&self, slug: &str) -> Result<(), String> {
        self.held.lock().unwrap().remove(slug);
        Ok(())
    }
}

/// Collect a stream into one unit, counting as it goes.
async fn drained(mut rows: SendableRecordBatchStream) -> Result<(Vec<RecordBatch>, u64), String> {
    let mut unit = Vec::new();
    let mut count = 0u64;
    while let Some(batch) = rows.next().await {
        let batch = batch.map_err(|e| e.to_string())?;
        count += batch.num_rows() as u64;
        unit.push(batch);
    }
    Ok((unit, count))
}

/// A read of whatever the slot holds **now**: each scan builds a fresh `MemTable` over the
/// current units and delegates to it, so a provider handed out at registration sees every unit
/// appended since. Read-only by construction — `insert_into` is the trait's own refusal — which
/// is the module contract's rule that the arm remains the only writer.
#[derive(Debug)]
struct MemProvider {
    slug: String,
    /// The schema at the moment the provider was built — what [`schema`](TableProvider::schema)
    /// falls back to once the slot is discarded, that call being infallible.
    seeded: SchemaRef,
    held: Arc<Mutex<HashMap<String, Held>>>,
}

#[async_trait]
impl TableProvider for MemProvider {
    fn schema(&self) -> SchemaRef {
        self.held
            .lock()
            .unwrap()
            .get(&self.slug)
            .map(|held| held.schema.clone())
            .unwrap_or_else(|| self.seeded.clone())
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
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        let table = {
            let held = self.held.lock().unwrap();
            let table = held.get(&self.slug).ok_or_else(|| {
                DataFusionError::Plan(format!(
                    "internal table '{}' is gone from its store",
                    self.slug
                ))
            })?;
            MemTable::try_new(table.schema.clone(), table.units.clone())?
        };
        table.scan(state, projection, filters, limit).await
    }
}
