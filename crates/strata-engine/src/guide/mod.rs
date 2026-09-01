//! Guides for embedding this engine, and for extending it.
//!
//! [`embedding`] is the place to start: build an engine, give it a catalog, run a statement,
//! read the result. The rest are read when a particular question comes up.
//!
//! - [`embedding`] — getting started, the capability model, what the engine asks of its caller.
//! - [`json`] — reading JSON that stock DataFusion refuses, and using that reader on its own.
//! - [`data_source`] — writing a [`DataSource`](crate::sources::source::DataSource): an object
//!   store, or a server with a catalog of its own.
//! - [`storage`] — writing a [`SnapshotStore`](crate::snapshots::SnapshotStore) or an
//!   [`InternalTableStore`](crate::tables::InternalTableStore).
//!
//! Beside them is [`testing`](crate::testing), which is not prose: it is the six conformance
//! rings, one per seam, that hold your implementation to the law the shipped ones keep.
//!
//! Nothing here is compiled into a binary: these modules hold documentation and no items.

pub mod data_source;
pub mod embedding;
pub mod json;
pub mod storage;
