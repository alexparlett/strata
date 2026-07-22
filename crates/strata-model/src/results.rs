//! Query-results vocabulary: a formatted [`Cell`], a page of [`QueryOutput`], and the
//! [`SnapshotId`] identifying the immutable result snapshot a Run materialized.
//! Produced by the engine, stored by [`crate::runs`], rendered by the grid.

use super::ColumnInfo;

/// One display cell: the formatted text plus a null flag (the grid dims nulls, so the
/// flag stays even though the text is the configured NULL rendering).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub null: bool,
}

/// The identity of one materialized result snapshot (`docs/SNAPSHOT_SPEC.md` §2): the
/// Run's request id, unique per engine for the life of the process — so a re-run of the
/// same SQL is a *different* snapshot, and every read keyed by this id targets a fixed,
/// immutable result set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SnapshotId(pub u64);

impl std::fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// The current page of a query, plus the snapshot handle: its id, the result schema
/// (`columns`), and the exact `total` — everything pagination / sort / export need to
/// read the materialized set.
#[derive(Clone, Debug, Default)]
pub struct QueryOutput {
    /// The materialized snapshot every later read targets. `None` ⇔ the query produced
    /// zero rows (nothing was materialized; there are no pages to read).
    pub snapshot: Option<SnapshotId>,
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Cell>>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub elapsed_ms: u128,
}
