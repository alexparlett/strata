//! Writing a storage store: where results live, and where engine-owned tables live.
//!
//! There are two seams, and they are separate because they hold different things for different
//! lengths of time.
//!
//! | | [`SnapshotStore`](crate::snapshots::SnapshotStore) | [`InternalTableStore`](crate::tables::InternalTableStore) |
//! |---|---|---|
//! | Holds | a settled query result | a table `CREATE TABLE` made |
//! | Lives | until the run is superseded or the engine drops | until the table is dropped |
//! | Keyed by | [`SnapshotId`](strata_model::SnapshotId), unique per engine | the table's slug |
//! | Written | once, then immutable | once, then appended to |
//! | Set with | [`with_snapshot_store`](crate::EngineBuilder::with_snapshot_store) | [`with_table_store`](crate::EngineBuilder::with_table_store) |
//!
//! Each ships two implementations — a durable default and an in-RAM one — and both run the same
//! contract-conformance module, which is where to look for what an implementation has to satisfy.
//! The `Mem` impls are the short ones and the better read.
//!
//! # `SnapshotStore`: where a result's bytes live
//!
//! A store is asked for three things and nothing else: open a write pass, hand back a provider
//! that reads what settled, and discard bytes it is holding. Whatever it does in between — a
//! file, an object store, a table in RAM — is its own, **and so is the format it does it in**.
//!
//! Five things it owes its readers whatever it is made of:
//!
//! - **Immutable once settled.** A re-run mints a new id; nothing rewrites an old one.
//!   Immutability is what makes every read of a snapshot safely cacheable by its arguments — the
//!   embedder contract in [`embedding`](super::embedding) rests on this.
//! - **Typed fidelity.** A result round-trips as itself, a union included. This is why the
//!   shipped default is Arrow IPC and not parquet: parquet's type system is narrower than
//!   Arrow's — it cannot write a union at all — so it coerced results on the way in. If you are
//!   choosing a format for your own store, this is the constraint that decides it, and
//!   [`json`](super::json) is why unions turn up at all.
//! - **The ordinal, written when minted.** A snapshot read has no order of its own, so the store
//!   numbers the rows in the order it is handed them and reports the column's name back. Every
//!   ordered read sorts by it and every reader projects it away.
//! - **Exact null counts from the write pass.** `Array::null_count` is a stored field, so the
//!   count is a running sum over data the sink is already holding — free at write time, and a map
//!   lookup instead of a scan when a partitioned export needs it.
//! - **`open` serves an immutable read**, never a re-list of something that might have moved.
//!
//! **Lifecycle is not yours.** Pins, retire-on-dispatch, liveness and the per-engine claim on
//! wherever the bytes go are the engine's own bookkeeping. Your store moves bytes and is told when
//! they stop being wanted.
//!
//! # `InternalTableStore`: where an engine-owned table's bytes live
//!
//! Four things: publish a new table's rows, add one statement's rows to an existing one, hand
//! back the provider that reads what it holds, destroy what it holds.
//!
//! - **A create publishes atomically.** Nothing that reads the store can observe a half-written
//!   table: the rows are visible entire under the slug, or not at all. The default spools aside
//!   and renames in.
//! - **An append is one unit per statement, and there is no compaction.** A table inserted into a
//!   thousand times holds a thousand units. Rewriting them smaller is the user's own `DROP TABLE`
//!   plus a `CREATE TABLE AS SELECT`.
//! - **The provider re-lists per scan.** A scan through a provider handed out at registration sees
//!   every unit appended since. That is what lets an `INSERT` re-read the table's facts without
//!   re-registering it — and re-registering is what strands the `Arc` a view captured, so this
//!   property is why views above a table survive a write untouched.
//! - **A discard is interruption-safe.** A killed process or a failure partway down leaves
//!   sweepable residue, never a half-destroyed table still answering under a live slug. The
//!   default renames the directory to a `.tmp-…` sibling first and walks it afterwards: the rename
//!   is the operation, the removal is housekeeping.
//!
//! **A store never names anything.** It is keyed by the slug the engine mints; the def,
//! `TableOrigin`, the write gate and the reserved-name fences are all the engine's. And its
//! provider is read through, never written through — the `INSERT` path drives `append` directly,
//! so the store remains the only writer and the gate in front of it remains the only gate.
//!
//! # `owned_storage`: the one method both seams share
//!
//! Both traits carry a **provided** `owned_storage` defaulting to none. It answers the filesystem
//! roots your store keeps bytes under, and what asks is the write fence: an export or a
//! `COPY … TO` landing under one of them would be read back as *result rows* or as *table rows*
//! by the next scan of that directory.
//!
//! Provided rather than required so a store in RAM or over an object store says nothing, which is
//! the honest answer — it owns no directory a local write could land in. Answer the root a reader
//! would have to look under: not the individual files, and not a parent that also holds the user's
//! own work.
//!
//! # The durability caveat, and where to state it
//!
//! [`MemSnapshotStore`](crate::snapshots::MemSnapshotStore) is unremarkable: a snapshot never
//! outlives its process anyway, so holding one in RAM loses nothing.
//!
//! [`MemTableStore`](crate::tables::MemTableStore) is not, and the difference is worth
//! generalising. An internal table's **def** is written into `project.json` and outlives the
//! process, while everything the store holds dies with it — so a restart replays defs against
//! data that is gone. The store does not paper over that: each such registration fails by naming
//! the missing data, which is the honest row.
//!
//! The pattern to copy is **where the caveat is written**: on the impl's own module docs, at the
//! point an embedder chooses it, not in a general note somewhere they may not read. A store whose
//! durability is narrower than the def's promise has to say so where it is offered.
