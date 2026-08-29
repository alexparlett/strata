//! Where a settled result's bytes live — [`SnapshotStore`], the seam, and the two stores that
//! ship with it.
//!
//! # The contract
//!
//! A store is asked for three things and nothing else: open a write pass
//! ([`begin`](SnapshotStore::begin) → [`write`](SnapshotSink::write) →
//! [`settle`](SnapshotSink::settle)), hand back a provider that reads what settled
//! ([`open`](SnapshotStore::open)), and discard bytes it is holding
//! ([`retire`](SnapshotStore::retire), [`purge_orphans`](SnapshotStore::purge_orphans)).
//! Whatever it does in between — a file, an object store, a table in RAM — is its own, and so is
//! the format it does it in.
//!
//! What every store owes its readers, whatever it is made of:
//!
//! - **Immutable once settled.** A re-run mints a new [`SnapshotId`]; nothing rewrites an old
//!   one. Immutability is what makes every read of it safely cacheable by its arguments
//!   (`docs/SNAPSHOT_SPEC.md` §1).
//! - **Typed fidelity.** A result round-trips as itself — a union included. This is the whole
//!   reason the shipped default is Arrow IPC and not parquet, whose type system is narrower than
//!   Arrow's (it cannot write a union at all) and which therefore coerced results on the way in.
//! - **The ordinal, written when minted.** A store numbers the rows it is handed, in the order it
//!   is handed them ([`ordinal`]), because a snapshot read has no order of its own
//!   (`docs/SNAPSHOT_SPEC.md` §9).
//! - **Exact null counts from the write pass.** [`SnapshotStats`] is what a settle answers with;
//!   a partitioned export reads it instead of scanning (`export::partition_null_refusal`).
//! - **[`open`](SnapshotStore::open) serves an immutable read**, never a re-list of something
//!   that might have moved.
//!
//! What a store is **not** asked about is lifecycle. Pins, retire-on-dispatch, liveness and the
//! per-engine claim on wherever the bytes go are the engine's own bookkeeping (`Lifecycle`): the
//! store moves bytes, and is told when they stop being wanted.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::array::Array;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::TableProvider;
use datafusion::prelude::SessionContext;
use strata_model::SnapshotId;

pub mod local_ipc;
pub mod mem;
mod name;
pub mod ordinal;

#[cfg(test)]
mod conformance;

pub use local_ipc::LocalIpcSnapshotStore;
pub use mem::MemSnapshotStore;
pub(crate) use name::is_snapshot_ref;
pub use name::{is_snapshot_name, snapshot_name};

/// Where an engine's settled results live ([`EngineBuilder::with_snapshot_store`](crate::EngineBuilder::with_snapshot_store)).
///
/// The module docs are the contract. [`LocalIpcSnapshotStore`] is the default and
/// [`MemSnapshotStore`] the session-scoped alternative; an embedder that wants results somewhere
/// else — an object store, a database — implements this.
#[async_trait]
pub trait SnapshotStore: Send + Sync + 'static {
    /// Open the write pass for `id`.
    ///
    /// `schema` is the shape of the batches about to arrive — the result's own, without the
    /// ordinal. `ord`, when the caller asks for one, is the ordinal column's name: the sink
    /// appends it to every batch it is handed ([`ordinal::with_ordinal`]) and reports it back in
    /// [`SnapshotStats::ord`]. `None` is a snapshot that genuinely has no order to declare, and
    /// its reads are unordered (`docs/SNAPSHOT_SPEC.md` §9 names the two shapes).
    ///
    /// Called when the first batch arrives, not when the query is dispatched: a result that
    /// produces no batches at all settles no snapshot, and a store is never asked to hold one.
    fn begin(
        &self,
        id: SnapshotId,
        schema: SchemaRef,
        ord: Option<String>,
    ) -> Result<Box<dyn SnapshotSink>, String>;

    /// The provider that reads what `id` settled.
    ///
    /// `ctx` is the session the provider will be scanned by — what a store needs to resolve its
    /// own storage against the runtime's object stores and read options. Called once per
    /// snapshot, by the engine, which registers the answer under [`snapshot_name`].
    async fn open(
        &self,
        ctx: &SessionContext,
        id: SnapshotId,
    ) -> Result<Arc<dyn TableProvider>, String>;

    /// Discard whatever is held for `id`.
    ///
    /// Best effort, and safe on a snapshot that never settled: this is also how a failed or
    /// cancelled run's partial is cleaned up. Deregistration is the engine's, not this.
    fn retire(&self, id: SnapshotId);

    /// Discard everything held for a snapshot outside `live`.
    ///
    /// The engine is the only thing that knows which snapshots are still live, so the sweep is a
    /// call rather than a rule a store could apply on its own. An engine on its way out passes an
    /// empty set, which means "all of it" — including, for a store that claimed somewhere to put
    /// them, the claim.
    fn purge_orphans(&self, live: &HashSet<SnapshotId>);

    /// Return the filesystem roots this store keeps its bytes under, if any.
    ///
    /// Provided, defaulting to none: a store in RAM or over an object store owns no directory a
    /// local write could land in. What asks is the write fence — an export or a `COPY` landing
    /// under one of these would be read back as a *result* by the next scan of it, so
    /// [`export::refuse_owned_target`](crate::export::refuse_owned_target) refuses every path
    /// beneath what is answered here.
    ///
    /// Answer the root a reader would have to look under, not the individual snapshot files, and
    /// not a parent that also holds the user's own work.
    fn owned_storage(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// One snapshot's write pass, from [`SnapshotStore::begin`].
///
/// Dropping one without [`settle`](Self::settle) abandons the write; what it leaves behind is
/// reclaimed by [`SnapshotStore::retire`], which the run's own error path calls.
pub trait SnapshotSink: Send {
    /// Spool one batch of the result, in order.
    fn write(&mut self, batch: &RecordBatch) -> Result<(), String>;

    /// Finish the snapshot and answer with what the pass observed.
    fn settle(self: Box<Self>) -> Result<SnapshotStats, String>;
}

/// What the write pass **observed**, so no later reader has to scan for it again.
///
/// Parquet's footer carried per-column statistics and `export::partition_columns_have_no_nulls`
/// read the null counts from it. Arrow IPC carries none — `ArrowFormat::infer_stats` is
/// `Statistics::new_unknown` — but nothing was ever gained by asking the *file*: a write pass
/// streams every batch already, and `Array::null_count` is a stored field on the null buffer, so
/// the exact count is a running sum over data the sink is holding anyway. Free at write time, and
/// a map lookup instead of a scan at export time.
///
/// Not persisted, and deliberately so: a snapshot never outlives its process, so this has exactly
/// the snapshot's lifetime and lives beside the rest of its bookkeeping in `Lifecycle`. A footer
/// or a sidecar file would be a second thing to keep in step for no gain.
#[derive(Debug, Clone, Default)]
pub struct SnapshotStats {
    /// Exact null count per column, in `QueryOutput::columns` order.
    pub nulls: Vec<u64>,
    /// The name of this snapshot's **ordinal column** (`docs/SNAPSHOT_SPEC.md` §9) — the
    /// written result order every ordered read sorts by and every reader projects away.
    /// Usually `__strata_ord`; escalated by prefix when the result itself has a column of
    /// that name. `None` means the snapshot genuinely has no ordinal — an `EXPLAIN` result or
    /// one with duplicate column names — and readers then read unordered, exactly as every
    /// snapshot did before ordinals existed.
    pub ord: Option<String>,
}

impl SnapshotStats {
    /// The pass's opening state: a zero per column of `schema`, and the ordinal it was asked for.
    pub fn new(schema: &SchemaRef, ord: Option<String>) -> Self {
        Self {
            nulls: vec![0; schema.fields().len()],
            ord,
        }
    }

    /// Fold one spooled batch in. Takes the batch **before** the ordinal is appended: the counts
    /// are about the result's own columns, which is what every reader of them indexes by.
    pub fn observe(&mut self, batch: &RecordBatch) {
        let width = batch.num_columns().min(self.nulls.len());
        for (i, col) in batch.columns().iter().take(width).enumerate() {
            self.nulls[i] += col.null_count() as u64;
        }
    }
}
