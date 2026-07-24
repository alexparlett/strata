//! One virtualized **body row** — the gutter cell + a [`Cell`] per column + the trailing
//! filler, on the zebra background with the row rule. Built by the grid's
//! `VirtualScrollView` builder for the ~viewport rows only; everything reactive (selection,
//! widths) is read *inside* — the builder closure is memoized, so state must be read here,
//! not snapshotted outside (the `VirtualScrollView` rule in CLAUDE.md).
//!
//! The row also owns its cells' interaction handlers: gutter double-click → record view
//! (P2-10), nested-cell double-click → value modal snapshotted at press time (P2-12), and
//! right-click → the copy menu over the selection, retargeting it first when the pressed
//! cell sits outside it (P2-11).

use std::rc::Rc;

use freya::prelude::*;

use strata_core::engine::serialize::cell_pretty_json;
use strata_model::Kind;

use super::cell::Cell;
use super::model::KindColors;
use super::{DataGridTheme, GridData, DEFAULT_COL_W, GUTTER_W, TRAIL_W};
use crate::apps::project::views::workbench::results::cell_view::{page_batch_row, CellValue};
use crate::apps::project::views::workbench::results::copy;
use crate::apps::project::views::workbench::results::selection::{CellRole, SelCtl};
use crate::components::divider::Divider;

/// One body row of the results grid (page row `index`, display order).
#[derive(PartialEq)]
pub struct Row {
    pub index: usize,
    /// The resolved (possibly find-filtered) page.
    pub data: Rc<GridData>,
    /// The find filter's absolute gutter numbers when the page is filtered (see `DataGrid`).
    pub row_nums: Option<Rc<Vec<usize>>>,
    /// Absolute index of the page's first row (0-based) — gutter numbering + batch mapping.
    pub row_base: usize,
    /// The grid's per-column widths — read reactively so a resize reflows the row.
    pub widths: State<Vec<f32>>,
    /// The shared selection controller (cells read it reactively for styling).
    pub sel: SelCtl,
    /// The nested-cell modal's open slot (P2-12) — a data-cell double-click fills it.
    pub cell_view: State<Option<CellValue>>,
    /// The record view's open slot (P2-10) — a gutter double-click points it at this row.
    pub record_view: State<Option<usize>>,
    /// Row box height (line box + the density's vertical padding) — matches `item_size`.
    pub row_h: f32,
    /// Horizontal cell padding from the density.
    pub cell_pad: Gaps,
    pub theme: DataGridTheme,
}

impl Component for Row {
    fn render(&self) -> impl IntoElement {
        let index = self.index;
        let row_base = self.row_base;
        let sel_ctl = self.sel;
        let record_view = self.record_view;
        let theme = &self.theme;

        // Right-click → the copy context menu over the selection (P2-11). A press on a
        // cell *outside* the current selection retargets it first (Excel semantics: the
        // gutter takes the whole row, a body cell a single-cell rectangle — both focus
        // the grid via SelCtl); the menu's actions then read the live selection.
        let open_copy_menu = {
            let data = self.data.clone();
            let row_nums = self.row_nums.clone();
            move || {
                ContextMenu::open(copy::copy_menu(
                    data.clone(),
                    row_nums.clone(),
                    row_base,
                    sel_ctl.sel,
                ));
            }
        };
        let on_menu_row = Some(EventHandler::new({
            let open_copy_menu = open_copy_menu.clone();
            move |_: Event<PointerEventData>| {
                if !sel_ctl.sel.peek().rows().contains(&index) {
                    sel_ctl.row(index);
                }
                open_copy_menu();
            }
        }));

        let mut cells = rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .direction(Direction::Horizontal)
            .content(Content::Flex)
            .child(Cell {
                width: Size::px(GUTTER_W),
                // A filtered page numbers by the survivors' original positions; otherwise
                // by position from the page base.
                text: self
                    .row_nums
                    .as_ref()
                    .and_then(|nums| nums.get(index).copied())
                    .unwrap_or(row_base + index + 1)
                    .to_string(),
                color: theme.gutter_color,
                mono: false,
                cross: Alignment::Center,
                pad: Gaps::default(),
                hover_bg: theme.gutter_active_background,
                divider: theme.column_divider_fill,
                role: CellRole::Row(index),
                sel: sel_ctl,
                sel_border: theme.selection_border_fill,
                active_color: Some(theme.gutter_active_color),
                active_background: Some(theme.gutter_active_background),
                // Double-click on the gutter opens the whole row in the record view
                // (P2-10) — a live page-row pointer, so no snapshot is taken here.
                on_open: Some(EventHandler::new(move |_: Event<PointerEventData>| {
                    let mut record_view = record_view;
                    record_view.set(Some(index));
                })),
                on_secondary: on_menu_row,
            });

        for (ci, col) in self.data.columns.iter().enumerate() {
            let w = self.widths.read().get(ci).copied().unwrap_or(DEFAULT_COL_W);
            let cell = &self.data.rows[index][ci];
            // Nested non-null value → double-click opens the cell view (P2-12). The
            // handler snapshots the pretty JSON **at press time** (the canvas semantics —
            // a later filter/page shift can't retarget an open modal), reading the typed
            // value from the page batch (a filtered page maps back through `row_nums`).
            let nested = matches!(col.kind, Kind::Struct | Kind::List | Kind::Map) && !cell.null;
            let on_nested = nested.then(|| {
                let data = self.data.clone();
                let row_nums = self.row_nums.clone();
                let name = col.name.clone();
                let dtype = col.dtype.clone();
                let mut cell_view = self.cell_view;
                EventHandler::new(move |_: Event<PointerEventData>| {
                    let row =
                        page_batch_row(row_nums.as_ref().map(|n| n.as_slice()), row_base, index);
                    let json = cell_pretty_json(&data.batch, ci, row)
                        .unwrap_or_else(|| data.rows[index][ci].text.clone());
                    cell_view.set(Some(CellValue {
                        name: name.clone(),
                        dtype: dtype.clone(),
                        json,
                    }));
                })
            });
            let on_menu_cell = Some(EventHandler::new({
                let open_copy_menu = open_copy_menu.clone();
                move |_: Event<PointerEventData>| {
                    if !sel_ctl.sel.peek().contains(index, ci) {
                        sel_ctl.cell_down(index, ci);
                        sel_ctl.end_drag();
                    }
                    open_copy_menu();
                }
            }));
            cells = cells.child(Cell {
                width: Size::px(w),
                text: cell.text.clone(),
                // Nulls render dimmed (the model keeps the flag exactly for this), in the
                // gutter's muted tone; everything else takes its type colour.
                color: if cell.null { theme.gutter_color } else { col.kind.cell_color(theme) },
                mono: true,
                cross: Alignment::Start,
                pad: Gaps::new(0., self.cell_pad.right(), 0., self.cell_pad.left()),
                hover_bg: theme.cell_hover_background,
                divider: theme.column_divider_fill,
                role: CellRole::Data(index, ci),
                sel: sel_ctl,
                sel_border: theme.selection_border_fill,
                active_color: None,
                active_background: None,
                on_open: on_nested,
                on_secondary: on_menu_cell,
            });
        }
        // Trailing dead space (matches the header) so the row extends past the last column.
        cells = cells
            .child(rect().width(Size::flex(1.)).min_width(Size::px(TRAIL_W)).height(Size::fill()));

        rect()
            .width(Size::fill())
            .height(Size::px(self.row_h))
            .background(if index % 2 == 1 {
                theme.zebra_row_background
            } else {
                theme.row_background
            })
            .content(Content::Flex)
            .child(cells)
            .child(Divider::horizontal().color(theme.divider_fill))
    }
}
