//! The window's **query layer** (state-arch §6): freya-query capabilities over the
//! engine facade. Owned by the results element — no runs store, no query state on the
//! session.
//!
//! Consumed by the results pane's `use_query` wiring (P2-02) — dead-code-allowed until
//! that lands.
#![allow(dead_code)]
#![allow(unused_imports)]

mod run_query;

pub use run_query::{
    FetchSnapshotPage, PageSpec, QueryMode, QueryOutcome, QueryPage, QuerySpec, RunId, RunQuery,
    SnapshotPage,
};
