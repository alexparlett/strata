//! One virtualized **body row** — the gutter cell + a [`Cell`] per *windowed* column (fixed
//! spacers stand in for the off-window ones, keeping x-positions and the row's width exact) +
//! the trailing filler, on the zebra background with the row rule. Built by the grid's
//! `VirtualScrollView` builder for the ~viewport rows only; everything reactive (selection,
//! widths, the column window) is read *inside* — the builder closure is memoized, so state
//! must be read here, not snapshotted outside (the `VirtualScrollView` rule, AGENTS.md §3).
//!
//! The row also owns its cells' interaction handlers: gutter double-click → record view
//! (P2-10), nested-cell double-click → value modal snapshotted at press time (P2-12), and
//! right-click → the copy menu over the selection, retargeting it first when the pressed
//! cell sits outside it (P2-11).

use std::rc::Rc;

use freya::prelude::*;

use strata_model::Kind;

use super::cell::Cell;
use super::model::KindColors;
use super::{ColWindow, DataGridTheme, GridData, GUTTER_W, TRAIL_W};
use crate::apps::project::views::workbench::results::cell_view::{page_batch_row, CellValue};
use crate::apps::project::views::workbench::results::copy;
use crate::apps::project::views::workbench::results::selection::{CellRole, SelCtl};
use crate::apps::project::views::workbench::results::value_tree::TreeModel;
use crate::components::divider::Divider;
use crate::components::type_palette::type_palette;

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
    /// The grid's starting column width — see [`HeaderRow::seed_w`](super::header::HeaderRow).
    pub seed_w: f32,
    /// The grid's resolved column window (`DataGrid::render` derives it from the horizontal
    /// scroll, spacer extents included) — read reactively, so a pan rebuilds exactly the
    /// rows' windowed cells.
    pub col_window: State<ColWindow>,
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
    /// `Settings::zebra` — off means every row takes the plain row background.
    pub zebra: bool,
    pub theme: DataGridTheme,
    /// Diff identity (the page row index): lets a scroll step *move* this row instead of
    /// rebuilding it in another position's scope — set via `.key()` in the grid's builder.
    pub key: DiffKey,
}

impl KeyExt for Row {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Row {
    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }

    fn render(&self) -> impl IntoElement {
        let index = self.index;
        let row_base = self.row_base;
        let sel_ctl = self.sel;
        let record_view = self.record_view;
        let theme = &self.theme;
        let palette = type_palette();

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

        let win = *self.col_window.read();
        let widths = self.widths.read();
        let col_w = |ci: usize| widths.get(ci).copied().unwrap_or(self.seed_w);

        let mut cells = rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .direction(Direction::Horizontal)
            .content(Content::Flex)
            .child(Cell {
                width: Size::px(GUTTER_W),
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
                on_open: Some(EventHandler::new(move |_: Event<PointerEventData>| {
                    let mut record_view = record_view;
                    record_view.set(Some(index));
                })),
                on_secondary: on_menu_row,
                key: DiffKey::None,
            });

        cells = cells.child(rect().width(Size::px(win.lead)).height(Size::fill()));
        for (ci, col) in self
            .data
            .columns
            .iter()
            .enumerate()
            .take(win.end)
            .skip(win.start)
        {
            let w = col_w(ci);
            let cell = &self.data.rows[index][ci];
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
                    cell_view.set(Some(CellValue {
                        name: name.clone(),
                        dtype: dtype.clone(),
                        tree: TreeModel::new(data.batch.clone(), ci, row),
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
            cells = cells.child(
                Cell {
                    width: Size::px(w),
                    text: cell.text.clone(),
                    color: if cell.null {
                        theme.gutter_color
                    } else {
                        col.kind.cell_color(theme, &palette)
                    },
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
                    key: DiffKey::None,
                }
                .key(ci),
            );
        }
        cells = cells.child(rect().width(Size::px(win.tail)).height(Size::fill()));
        cells = cells.child(
            rect()
                .width(Size::flex(1.))
                .min_width(Size::px(TRAIL_W))
                .height(Size::fill()),
        );

        rect()
            .width(Size::fill())
            .height(Size::px(self.row_h))
            .background(if self.zebra && index % 2 == 1 {
                theme.zebra_row_background
            } else {
                theme.row_background
            })
            .content(Content::Flex)
            .child(cells)
            .child(Divider::horizontal().color(theme.divider_fill))
    }
}
