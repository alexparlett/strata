//! The in-RAM store: one DataFusion `MemTable` per snapshot, and nothing on disk.
//!
//! Fully honest rather than a test double — a snapshot is session-scoped by construction here,
//! which is exactly what the model already says one is (`docs/SNAPSHOT_SPEC.md` §2: ids are
//! per-engine unique for the life of the process, and a snapshot never outlives its engine).
//! What it trades is the one thing the spool buys: RAM holds one page under
//! [`LocalIpcSnapshotStore`](super::LocalIpcSnapshotStore) and the whole result here, so an
//! engine that pages results larger than memory wants the default.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::TableProvider;
use datafusion::common::Column;
use datafusion::datasource::MemTable;
use datafusion::prelude::{col, SessionContext};
use strata_model::SnapshotId;

use crate::snapshots::ordinal::{ordinal_schema, with_ordinal};
use crate::snapshots::{SnapshotSink, SnapshotStats, SnapshotStore};

/// Settled results, held in RAM for the store's lifetime.
#[derive(Default)]
pub struct MemSnapshotStore {
    settled: Arc<Mutex<HashMap<SnapshotId, Arc<MemTable>>>>,
}

impl MemSnapshotStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SnapshotStore for MemSnapshotStore {
    fn begin(
        &self,
        id: SnapshotId,
        schema: SchemaRef,
        ord: Option<String>,
    ) -> Result<Box<dyn SnapshotSink>, String> {
        let ord_schema = ord.as_deref().map(|ord| ordinal_schema(&schema, ord));
        let stats = SnapshotStats::new(&schema, ord);
        Ok(Box::new(MemSink {
            id,
            schema: ord_schema.clone().unwrap_or(schema),
            ord_schema,
            batches: Vec::new(),
            rows: 0,
            stats,
            settled: Arc::clone(&self.settled),
        }))
    }

    async fn open(
        &self,
        _ctx: &SessionContext,
        id: SnapshotId,
    ) -> Result<Arc<dyn TableProvider>, String> {
        self.settled
            .lock()
            .unwrap()
            .get(&id)
            .map(|table| Arc::clone(table) as Arc<dyn TableProvider>)
            .ok_or_else(|| format!("snapshot {id} has not settled"))
    }

    fn retire(&self, id: SnapshotId) {
        self.settled.lock().unwrap().remove(&id);
    }

    fn purge_orphans(&self, live: &HashSet<SnapshotId>) {
        self.settled
            .lock()
            .unwrap()
            .retain(|id, _| live.contains(id));
    }
}

/// One snapshot's write pass, accumulating batches.
struct MemSink {
    id: SnapshotId,
    /// The settled table's schema — the result's own, with the ordinal appended when there is one.
    schema: SchemaRef,
    /// The same schema when there is an ordinal, which is what [`with_ordinal`] builds against.
    ord_schema: Option<SchemaRef>,
    batches: Vec<RecordBatch>,
    /// How many rows are already held, which is what the next batch's ordinal counts from.
    rows: u64,
    stats: SnapshotStats,
    settled: Arc<Mutex<HashMap<SnapshotId, Arc<MemTable>>>>,
}

impl SnapshotSink for MemSink {
    fn write(&mut self, batch: &RecordBatch) -> Result<(), String> {
        let held = match &self.ord_schema {
            Some(schema) => with_ordinal(batch, schema, self.rows)?,
            None => batch.clone(),
        };
        self.stats.observe(batch);
        self.rows += batch.num_rows() as u64;
        self.batches.push(held);
        Ok(())
    }

    /// One partition, because the batches were handed over in result order and a second
    /// partition is a second stream the reader would have to merge — the very thing the ordinal
    /// exists to make unnecessary.
    fn settle(self: Box<Self>) -> Result<SnapshotStats, String> {
        let mut table = MemTable::try_new(Arc::clone(&self.schema), vec![self.batches])
            .map_err(|e| e.to_string())?;
        if let Some(ord) = &self.stats.ord {
            table = table.with_sort_order(vec![vec![
                col(Column::from_name(ord.clone())).sort(true, false)
            ]]);
        }
        self.settled
            .lock()
            .unwrap()
            .insert(self.id, Arc::new(table));
        Ok(self.stats)
    }
}
