//! The window's **query layer** (state-arch §6): freya-query capabilities over the
//! engine facade. Owned by the results element — no runs store, no query state on the
//! session.
//!
//! The page-read side (`FetchSnapshotPage` and friends) is consumed by the grid's
//! paging/sort (P2-03) — dead-code/unused-import-allowed until that lands.
#![allow(dead_code)]
#![allow(unused_imports)]

mod run_query;

pub use run_query::{
    FetchSnapshotPage, PageSpec, QueryMode, QueryOutcome, QueryPage, QuerySpec, RunId,
    RunQuery, SnapshotPage, DEFAULT_PAGE_SIZE,
};
