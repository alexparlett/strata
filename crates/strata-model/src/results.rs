//! Query-results vocabulary: a formatted [`Cell`] and a page of [`QueryOutput`].
//! Produced by the engine, stored by [`crate::runs`], rendered by the grid.

use super::ColumnInfo;

/// One display cell: the formatted text plus a null flag (the grid dims nulls, so the
/// flag stays even though the text is the configured NULL rendering).
#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub null: bool,
}

/// The current page of a query, plus the snapshot's true total.
#[derive(Clone, Debug, Default)]
pub struct QueryOutput {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Cell>>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub elapsed_ms: u128,
}
