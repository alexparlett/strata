//! Where a **Strata-owned table's** bytes live — [`InternalTableStore`], the seam, and the two
//! stores that ship with it.
//!
//! # The contract
//!
//! A store is asked for four things and nothing else: publish a new table's rows
//! ([`create`](InternalTableStore::create)), add one statement's rows to an existing one
//! ([`append`](InternalTableStore::append)), hand back the provider that reads what it holds
//! ([`provider`](InternalTableStore::provider)), and destroy what it holds
//! ([`discard`](InternalTableStore::discard)). Whatever it does in between — files under the
//! project, a table in RAM, an object store — is its own, and so is the format it does it in.
//!
//! What every store owes, whatever it is made of:
//!
//! - **A create publishes atomically.** Nothing that reads the store can observe a half-written
//!   table: the rows are visible entire under the slug, or not at all. The shipped default spools
//!   aside and renames in.
//! - **An append is one unit per statement, and there is no compaction.** A table inserted into a
//!   thousand times holds a thousand units; `DROP TABLE` plus a `CREATE TABLE AS SELECT * FROM t`
//!   is the compaction story until a task owns one.
//! - **The provider re-lists per scan.** A scan through a provider handed out at registration
//!   sees every unit appended since, which is what lets an `INSERT`'s fold re-read the table's
//!   facts without re-registering it — the whole reason the views above it survive a write
//!   untouched.
//! - **A discard is interruption-safe.** An interruption — a killed process, a failure partway
//!   down — leaves sweepable residue, never a half-destroyed table still answering under a live
//!   slug.
//!
//! # What is deliberately not here
//!
//! **A store never names anything.** It is keyed by the **slug** the engine mints from the
//! table's name; the def, `TableOrigin`, `Catalog::is_internal`, the store-first drop order and
//! the `__snap_` fences are all untouched, and the name machinery reads the catalog registration
//! (the store's provider, registered under the def's folded name) plus the def — no hook into
//! qualify or the dialect rewrite. The engine registers what the store answers with; the store
//! moves bytes.
//!
//! And the store's provider is **read through, never written through**: the local `INSERT` arm
//! drives [`append`](InternalTableStore::append) directly, so the arm remains the only writer
//! and the gate in front of it (`Catalog::is_internal`, off the *parsed* target) remains the
//! only gate.

use std::sync::Arc;

use async_trait::async_trait;
use datafusion::catalog::TableProvider;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::prelude::SessionContext;

pub mod local_ipc;
pub mod mem;

#[cfg(test)]
mod conformance;

pub use local_ipc::LocalIpcTableStore;
pub use mem::MemTableStore;

/// Where an engine's internal tables live ([`EngineBuilder::with_table_store`](crate::EngineBuilder::with_table_store)).
///
/// The module docs are the contract. [`LocalIpcTableStore`] is the default and
/// [`MemTableStore`] the ephemeral, tests-first alternative; an embedder that wants Strata-owned
/// tables somewhere else — an object store, a database — implements this.
#[async_trait]
pub trait InternalTableStore: Send + Sync + 'static {
    /// Publish `rows` as the whole of the table `slug`, replacing anything held under it, and
    /// answer with how many rows landed — the write pass is the one thing that counted them.
    ///
    /// The publish is **atomic** (module docs). The stream's own schema is the table's, which is
    /// what makes a stream with no batches a table all the same: a bare
    /// `CREATE TABLE t (a INT)` arrives here as an empty stream carrying that schema, and what
    /// is published still describes its columns on every later read.
    ///
    /// Whether the slug may be created, replaced or must be refused is the **arm's** question,
    /// answered against the registered namespace before this is called; the store only moves
    /// bytes.
    async fn create(&self, slug: &str, rows: SendableRecordBatchStream) -> Result<u64, String>;

    /// Add `rows` to the table `slug` as **one unit**, and answer with how many landed.
    ///
    /// The unit is a statement's: an `INSERT` appends exactly one, no compaction runs, and a
    /// scan through an already-registered provider sees it (module docs). The stream arrives
    /// already coerced to the table's shape — DataFusion's `INSERT` planner casts and renames
    /// onto the registered schema, so the schema check is the planner's and a source that cannot
    /// be coerced never reaches the store.
    async fn append(&self, slug: &str, rows: SendableRecordBatchStream) -> Result<u64, String>;

    /// The provider that reads what is held under `slug` — what registration puts in the catalog
    /// under the def's folded name.
    ///
    /// `ctx` is the session the provider will be scanned by, for the same reason
    /// [`SnapshotStore::open`](crate::snapshots::SnapshotStore::open) takes one: session config,
    /// schema inference, and the per-file statistics cache a hand-built listing has to be handed
    /// by name.
    ///
    /// `Ok(None)` means the store holds **nothing** under the slug — a def replayed against a
    /// store whose data is gone — and registration then falls back to the def's own resolved
    /// paths, which is what turns it into the honest failed row rather than a fault. `Err` means
    /// the store holds something it could not serve, in the underlying reader's own words.
    async fn provider(
        &self,
        ctx: &SessionContext,
        slug: &str,
    ) -> Result<Option<Arc<dyn TableProvider>>, String>;

    /// Destroy what is held under `slug`. Interruption-safe (module docs), and safe on a slug
    /// holding nothing — a def whose data never reached this machine still drops cleanly.
    ///
    /// Deregistration is the engine's, not this; so is the order (deregister first), the confirm
    /// in front of a drop, and the report behind it.
    async fn discard(&self, slug: &str) -> Result<(), String>;
}
