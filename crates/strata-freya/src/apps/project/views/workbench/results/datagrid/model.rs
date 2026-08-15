//! Datagrid data model — the [`GridData`] page the grid renders (the run's real result schema +
//! the engine's formatted cells), the resolved-page [`PageRead`], the [`Kind`] → theme-colour
//! mapping ([`KindColors`]), and the cell-padding [`Density`].

use std::rc::Rc;
use std::sync::Arc;

use freya::prelude::*;
use strata_engine::RecordBatch;
use strata_model::{Cell, ColumnInfo, Kind, QueryOutput};

use super::DataGridTheme;
use crate::components::type_palette::TypePaletteTheme;

/// Theme-colour mapping for a column's [`Kind`] — the **cell text** colour (Dioxus
/// `.cell.num` / `.cell.ts` / `.cell.bool`; everything else default).
///
/// The header's dtype-label colour is not here: that is the shared type palette
/// ([`crate::components::type_palette::kind_color`]), which this borrows for booleans so a `true`
/// reads in the same hue its column header does.
pub trait KindColors {
    fn cell_color(self, t: &DataGridTheme, palette: &TypePaletteTheme) -> Color;
}

impl KindColors for Kind {
    fn cell_color(self, t: &DataGridTheme, palette: &TypePaletteTheme) -> Color {
        match self {
            Kind::Num => t.cell_num_color,
            Kind::Ts => t.cell_ts_color,
            Kind::Bool => palette.bool_color,
            _ => t.color,
        }
    }
}

/// The grid's input: one page of a run — the result schema plus that page's formatted cells,
/// and the Arrow batch those cells were formatted from.
pub struct GridData {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Cell>>,
    /// The page's typed source — `cell_preview_json` for the nested-cell view (P2-12); Copy /
    /// Export later. A find-filtered page keeps the **unfiltered** page batch: map a filtered
    /// row index back through `row_nums` (see `cell_view::page_batch_row`).
    pub batch: RecordBatch,
}

impl PartialEq for GridData {
    fn eq(&self, other: &Self) -> bool {
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

/// `Eq` is what lets an `Rc<GridData>` comparison take std's pointer-identity fast path
/// (specialized on `T: Eq`) — so a props diff carrying an unchanged page is one pointer
/// compare instead of a walk of every cell (measured: a resize drag paid that walk per
/// pointer move through the virtual scroller's `builder_data`). The claim holds: the
/// manual `eq` above is reflexive — `Arc::ptr_eq` plus float-free display fields.
impl Eq for GridData {}

impl GridData {
    /// Page 1, riding in the Run's own [`QueryOutput`] — no page fetch on first paint. The
    /// batch is the Run's page-1 batch (`QueryPage::batch`), cheap to clone (`Arc`'d arrays).
    pub fn from_run(output: &QueryOutput, batch: &RecordBatch) -> Self {
        Self {
            columns: output.columns.clone(),
            rows: output.rows.clone(),
            batch: batch.clone(),
        }
    }

    /// A later page read from the immutable snapshot; the schema is the Run's (a page fetch
    /// carries only rows + their batch).
    pub fn from_page(columns: Vec<ColumnInfo>, rows: Vec<Vec<Cell>>, batch: RecordBatch) -> Self {
        Self {
            columns,
            rows,
            batch,
        }
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
/// fixed). [`Compact`](Density::Compact) halves the vertical padding for denser tables. Selected by
/// the user's `Settings::density_compact` (default [`Comfortable`](Density::Comfortable)), read
/// where the grid renders.
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
