//! Query-results vocabulary: a formatted [`Cell`], a page of [`QueryOutput`], the
//! [`PageQuery`] naming a later page of it, and the [`SnapshotId`] identifying the immutable
//! result snapshot a Run materialized.

use super::ColumnInfo;

/// One display cell: the formatted text plus a null flag (the grid dims nulls, so the
/// flag stays even though the text is the configured NULL rendering).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The cell as it is displayed.
    pub text: String,
    /// Whether the underlying value is NULL.
    pub null: bool,
}

/// The identity of one materialized result snapshot (`docs/SNAPSHOT_SPEC.md` §2): unique per engine
/// for the life of the process, so a re-run of the same SQL is a *different* snapshot and every
/// read keyed by this id targets a fixed, immutable result set.
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
    /// The result schema.
    pub columns: Vec<ColumnInfo>,
    /// This page's rows.
    pub rows: Vec<Vec<Cell>>,
    /// How many rows the whole result holds.
    pub total: usize,
    /// Which page this is, 1-based.
    pub page: usize,
    /// How many rows a page holds.
    pub page_size: usize,
    /// Wall-clock the run took.
    pub elapsed_ms: u128,
}

/// Which page of a settled snapshot to read, and how to order it.
///
/// `sort` is applied as an `ORDER BY` over the *whole* snapshot before the page window, never as
/// a rewrite of it; `None` reads in snapshot order.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PageQuery {
    /// 1-based.
    pub page: usize,
    /// How many rows a page holds.
    pub page_size: usize,
    /// `(column name, ascending)`.
    pub sort: Option<(String, bool)>,
}
