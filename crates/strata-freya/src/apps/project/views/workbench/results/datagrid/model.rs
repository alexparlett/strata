//! Datagrid data model — the [`GridData`] page the grid renders (the run's real result schema +
//! the engine's formatted cells), the resolved-page [`PageRead`], the [`Kind`] → theme-colour
//! mapping ([`KindColors`]), and the cell-padding [`Density`].

use std::rc::Rc;
use std::sync::Arc;

use freya::prelude::*;
use strata_core::engine::RecordBatch;
use strata_model::{Cell, ColumnInfo, Kind, QueryOutput};

use super::DataGridTheme;

/// Theme-colour mapping for a column's [`Kind`] (the model's schema vocabulary) — drives the
/// header dtype-label colour and the cell text colour (matches the Dioxus `Kind` →
/// `text_class()` / `cell_class()`).
pub trait KindColors {
    /// The header dtype-label colour (Dioxus `.ct .t-*`).
    fn type_color(self, t: &DataGridTheme) -> Color;
    /// The cell text colour (Dioxus `.cell.num` / `.cell.ts` / `.cell.bool`; everything else default).
    fn cell_color(self, t: &DataGridTheme) -> Color;
}

impl KindColors for Kind {
    fn type_color(self, t: &DataGridTheme) -> Color {
        match self {
            Kind::Str => t.type_str_color,
            Kind::Num => t.type_num_color,
            Kind::Bool => t.type_bool_color,
            Kind::Ts => t.type_ts_color,
            Kind::Struct => t.type_struct_color,
            Kind::List => t.type_list_color,
            Kind::Map => t.type_map_color,
        }
    }

    fn cell_color(self, t: &DataGridTheme) -> Color {
        match self {
            Kind::Num => t.cell_num_color,
            Kind::Ts => t.cell_ts_color,
            Kind::Bool => t.type_bool_color,
            _ => t.color,
        }
    }
}

/// The grid's input: one page of a run — the result schema plus that page's formatted cells,
/// and the Arrow batch those cells were formatted from.
pub struct GridData {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Cell>>,
    /// The page's typed source — `cell_pretty_json` for the nested-cell view (P2-12); Copy /
    /// Export later. A find-filtered page keeps the **unfiltered** page batch: map a filtered
    /// row index back through `row_nums` (see `cell_view::page_batch_row`).
    pub batch: RecordBatch,
}

impl PartialEq for GridData {
    fn eq(&self, other: &Self) -> bool {
        // The batch compares by *identity* (the same underlying Arrow arrays — clones of one
        // batch share them), not by content: the display rows already deep-compare, and a
        // content compare of the arrays would double the diffing cost for nothing.
        self.columns == other.columns
            && self.rows == other.rows
            && self.batch.columns().len() == other.batch.columns().len()
            && self
                .batch
                .columns()
                .iter()
                .zip(other.batch.columns())
                .all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

impl GridData {
    /// Page 1, riding in the Run's own [`QueryOutput`] — no page fetch on first paint. The
    /// batch is the Run's page-1 batch (`QueryPage::batch`), cheap to clone (`Arc`'d arrays).
    pub fn from_run(output: &QueryOutput, batch: &RecordBatch) -> Self {
        Self { columns: output.columns.clone(), rows: output.rows.clone(), batch: batch.clone() }
    }

    /// A later page read from the immutable snapshot; the schema is the Run's (a page fetch
    /// carries only rows + their batch).
    pub fn from_page(columns: Vec<ColumnInfo>, rows: Vec<Vec<Cell>>, batch: RecordBatch) -> Self {
        Self { columns, rows, batch }
    }
}

/// The resolved read of the snapshot page the results pane currently shows. `ResultsBody` owns
/// the resolution — page 1 straight from the Run's own output while the page size still matches
/// the Run's, anything else through the cached `FetchSnapshotPage` — and threads the result as a
/// prop to *both* consumers: the grid renders it, the status bar aggregates the selection over
/// it. One subscription, one place the "page 1 rides in the Run" rule lives.
#[derive(Clone, PartialEq)]
pub enum PageRead {
    /// The page's rows are in hand.
    Ready(Rc<GridData>),
    /// The snapshot read is in flight.
    Loading,
    /// The snapshot read settled `Err`.
    Failed(String),
}

impl PageRead {
    /// The page data, when the read has settled `Ok`.
    pub fn ready(&self) -> Option<&Rc<GridData>> {
        match self {
            PageRead::Ready(data) => Some(data),
            _ => None,
        }
    }
}

/// Cell padding density — the vertical breathing room around cell text (the horizontal inset is
/// fixed). Defaults to [`Comfortable`](Density::Comfortable); [`Compact`](Density::Compact) halves the
/// vertical padding for denser tables. Wire to a user setting later (the Dioxus grid has a compact toggle).
#[derive(Clone, Copy, PartialEq)]
pub enum Density {
    Comfortable,
    Compact,
}

impl Density {
    /// This density's cell padding, read from the `datagrid` theme (`comfortable_cell_padding` /
    /// `compact_cell_padding`) — the two formats live in the theme file, not in code. The horizontal
    /// sides inset the text; the vertical sides set the row height (`CELL_LINE_H + padding.vertical()`).
    pub fn padding(self, t: &DataGridTheme) -> Gaps {
        match self {
            Density::Comfortable => t.comfortable_cell_padding,
            Density::Compact => t.compact_cell_padding,
        }
    }
}
