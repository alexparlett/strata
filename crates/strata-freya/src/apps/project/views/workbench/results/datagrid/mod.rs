//! The results **datagrid** — our custom virtualized grid (distinct from Freya's built-in `Table`,
//! which renders a component per cell with no virtualization). Virtualized in **both** directions:
//! a [`VirtualScrollView`] over the rows (only the ~viewport rows are ever built) and a
//! scroll-derived **column window** over the columns (only the ~viewport columns are built; fixed
//! spacers stand in for the rest, so the scroll extent and alignment stay exact) — with hand-rolled
//! `rect` cells: a row-number gutter, per-column resizable widths, a type-labelled sticky header,
//! type-coloured cell text, zebra rows, column / row / header dividers, per-cell hover, and
//! Excel-style selection. Horizontal scroll pans header + body together for wide tables.
//!
//! Layout: this file owns the [`DataGrid`] component + its render (page resolution, scroll
//! composition, focus + keyboard wiring, the modals), the shared constants, and the `datagrid`
//! [`define_theme!`]. The pieces live in submodules — [`model`] (grid data + type→colour +
//! density), [`cell`] (the body / gutter / `#` cell), [`header`] (the sticky header row +
//! column cells + resize grips + auto-fit), [`row`] (one virtualized body row + its cells'
//! interaction handlers) — and the selection model is the sibling `super::selection`.
//!
//! Every colour is a `datagrid` component token (`define_theme!` / `get_theme!`) — no semantic sheet
//! reads. Fed by the Run's real [`GridData`]: the results pane resolves the current page (page 1
//! from the Run's own output, anything else via the cached `FetchSnapshotPage`) and hands it in as
//! a [`PageRead`] — the grid itself never touches the engine.

use std::rc::Rc;

use freya::components::{define_theme, get_theme, CircularLoader};
use freya::prelude::*;

use crate::state::{use_config, use_config_station, ConfigChan};
use strata_core::config::Command;
use strata_core::engine::serialize::TextFormat;

use super::cell_view::{CellValue, CellView};
use super::copy;
use super::error::ErrorState;
use super::find::FindState;
use super::record_view::RecordView;
use super::selection::{SelCtl, Selection};
use super::sort::SortState;
use super::toolbar::ResultsToolbar;
use crate::apps::export::ExportLaunch;
use crate::apps::project::views::workbench::results::shape::ShapeTarget;
use crate::components::divider::Divider;
use crate::keymap::on_commands;
use strata_model::TabId;

mod cell;
mod header;
#[cfg(test)]
mod interaction;
mod model;
mod row;

use header::HeaderRow;
use model::Density;
pub use model::{GridData, PageRead};
use row::Row;

const HEADER_H: f32 = 46.;
const GUTTER_W: f32 = 52.;
const TRAIL_W: f32 = 48.;
const CELL_LINE_H: f32 = 16.;
/// The grid's own starting column width, and the number `Settings::default_col_width`'s default
/// mirrors (that setting took this constant's place — core's `defaults_match_the_constants…`
/// test pins the pair). Reached only when the setting can't be honoured at all; every column
/// width the grid actually uses comes from the seed `DataGrid::render` computes from it.
const DEFAULT_COL_W: f32 = 168.;
/// The bounds a column width is held to — seeded from the setting or dragged. Defined once, in
/// core beside the setting itself, so the Settings ▸ Data display input offers exactly the
/// range the grid honours (see `strata_core::config::COL_WIDTH_MIN`).
const MIN_COL_W: f32 = strata_core::config::COL_WIDTH_MIN as f32;
const MAX_COL_W: f32 = strata_core::config::COL_WIDTH_MAX as f32;
const GRIP_W: f32 = 6.;
const EDGE_MARGIN: f32 = 36.;
const EDGE_STEP: f32 = 24.;
const SCROLL_AXIS_LOCK: f32 = 1.0;
/// How far past each horizontal viewport edge the column window extends — two default columns
/// of slack on each side. The window is derived in a side effect, which settles a task-poll
/// after the scroll event that moved the view, so a step within the overscan lands on cells
/// that are already built; a single jump larger than this (a scrollbar-thumb scrub, a hard
/// fling) shows the spacer for that one frame and back-fills when the effect settles. That is
/// the accepted cost of deriving the window off the render path.
const OVERSCAN_W: f32 = 2. * DEFAULT_COL_W;
/// How much viewport width the *seed* window covers before the first sized layout has run.
/// Wider than any real screen, so the first paint is never blank — while a thousand-column
/// result no longer builds every column for that one frame (the un-windowed tree is exactly
/// the cost this windowing exists to remove, and a Run press remounts the grid).
const SEED_COVER_W: f32 = 6000.;

/// The resolved column window: the half-open range of columns the header and body build, plus
/// the extent of the off-window columns on either side — the spacers' widths. Resolved in
/// **one** place (the side effect in [`DataGrid::render`]) and consumed as a value, so header
/// and body cannot disagree about which columns are real, and the O(cols) extent sums are paid
/// once per window move rather than per visible row.
#[derive(Clone, Copy, PartialEq)]
pub struct ColWindow {
    pub start: usize,
    pub end: usize,
    /// The summed width of the columns before `start`.
    pub lead: f32,
    /// The summed width of the columns from `end` on.
    pub tail: f32,
}

impl ColWindow {
    /// Resolve a `[start, end)` range against the live widths. `start <= end <= widths.len()`
    /// — the callers' ranges come from [`col_range`] (bounded by construction) or a union of
    /// two such ranges.
    fn resolve(widths: &[f32], start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            lead: widths[..start].iter().sum(),
            tail: widths[end..].iter().sum(),
        }
    }
}

/// The half-open range of columns whose spans intersect `[lo, hi)`, in content coordinates
/// (columns start after the gutter). `lo`/`hi` come from the horizontal scroll position ±
/// [`OVERSCAN_W`]; an empty range means the band sits entirely off the columns (in the gutter,
/// or in the trailing dead zone).
fn col_range(widths: &[f32], lo: f32, hi: f32) -> (usize, usize) {
    let mut left = GUTTER_W;
    let mut start = None;
    let mut end = widths.len();
    for (ci, w) in widths.iter().enumerate() {
        let right = left + w;
        if start.is_none() && right > lo {
            start = Some(ci);
        }
        if left >= hi {
            end = ci;
            break;
        }
        left = right;
    }
    (start.unwrap_or(end), end)
}

define_theme!(
    %[component]
    pub DataGrid {
        %[fields]
        background: Color,
        arrow_fill: Color,
        row_background: Color,
        zebra_row_background: Color,
        cell_hover_background: Color,
        selection_border_fill: Color,
        gutter_color: Color,
        gutter_active_background: Color,
        gutter_active_color: Color,
        header_color: Color,
        header_background: Color,
        header_hover_background: Color,
        header_label_color: Color,
        header_active_background: Color,
        header_active_color: Color,
        divider_fill: Color,
        column_divider_fill: Color,
        header_divider_fill: Color,
        cell_num_color: Color,
        cell_ts_color: Color,
        color: Color,
        comfortable_cell_padding: Gaps,
        compact_cell_padding: Gaps,
    }
);

/// The results grid for one settled Run. Renders the page the results pane resolved for it
/// ([`PageRead`]): the pane owns the page/page-size state and the snapshot read; the grid keeps
/// its own per-column widths (which is why the in-flight and failed page states render *inside*
/// it — swapping the component out would drop the user's resizes).
#[derive(PartialEq)]
pub struct DataGrid {
    /// The page the Run itself returned — the source of the result schema (one seeded width
    /// per column of it).
    run: Rc<GridData>,
    /// The resolved current page.
    view: PageRead,
    /// Absolute index of the page's first row (0-based) — the gutter continues across pages.
    row_base: usize,
    /// The tab this grid's Run belongs to — the toolbar's Trash clears its Run trigger.
    tab: TabId,
    /// Find-in-results (P2-09): the popover state the toolbar renders and ⌘F / Esc drive.
    find: FindState,
    /// Column sort (P2-13): the intent the header chevrons cycle; the results pane folds it
    /// into the snapshot read.
    sort: SortState,
    /// Absolute gutter numbers when the find filter reindexed the page (survivors keep
    /// their original positions, so the gutter shows gaps); `None` = number by position.
    row_nums: Option<Rc<Vec<usize>>>,
    /// The snapshot's total row count — the record view's `Row n of total` label (P2-10).
    total: usize,
    /// What the toolbar's Download would export (P4-10) — the settled run's snapshot, schema,
    /// sort and page. `None` while the run hasn't settled rows, which is exactly when there is
    /// nothing to export.
    export: Option<ExportLaunch>,
    /// What the toolbar's Shape press composes over (Chart 09). `None` on the same terms as
    /// `export`: no settled rows, nothing to group.
    shape: Option<ShapeTarget>,
    pub(crate) theme: Option<DataGridThemePartial>,
}

impl DataGrid {
    pub fn new(
        run: Rc<GridData>,
        view: PageRead,
        row_base: usize,
        tab: TabId,
        find: FindState,
        sort: SortState,
    ) -> Self {
        Self {
            run,
            view,
            row_base,
            tab,
            find,
            sort,
            row_nums: None,
            total: 0,
            export: None,
            shape: None,
            theme: None,
        }
    }

    /// The filtered page's absolute gutter numbers (see [`Self::row_nums`]).
    pub fn row_nums(mut self, row_nums: Option<Rc<Vec<usize>>>) -> Self {
        self.row_nums = row_nums;
        self
    }

    /// The snapshot's total row count (see [`Self::total`]).
    pub fn total(mut self, total: usize) -> Self {
        self.total = total;
        self
    }

    /// What Download would export (see [`Self::export`]).
    pub fn export(mut self, export: Option<ExportLaunch>) -> Self {
        self.export = export;
        self
    }

    /// What the Shape press composes over (see [`Self::shape`]).
    pub fn shape(mut self, shape: Option<ShapeTarget>) -> Self {
        self.shape = shape;
        self
    }
}

impl Component for DataGrid {
    #[allow(clippy::too_many_lines)]
    fn render(&self) -> impl IntoElement {
        let settings = use_config(ConfigChan::Settings);
        let (zebra, density, setting_w) = {
            let cfg = settings.read();
            (
                cfg.settings.zebra,
                if cfg.settings.density_compact {
                    Density::Compact
                } else {
                    Density::Comfortable
                },
                cfg.settings.default_col_width as f32,
            )
        };
        let seed_w = use_hook(move || {
            if setting_w.is_finite() {
                setting_w.clamp(MIN_COL_W, MAX_COL_W)
            } else {
                DEFAULT_COL_W
            }
        });

        let n = self.run.columns.len();
        let widths = use_state(move || vec![seed_w; n]);
        let controller = use_scroll_controller(ScrollConfig::default);
        let mut viewport = use_state(Area::default);
        let hold_w = use_state(|| 0.0f32);

        let mut col_window = use_state(move || {
            let end = n.min((SEED_COVER_W / seed_w).ceil() as usize);
            ColWindow {
                start: 0,
                end,
                lead: 0.,
                tail: (n - end) as f32 * seed_w,
            }
        });
        use_side_effect(move || {
            let (sx, _): (i32, i32) = controller.into();
            let vp_w = viewport.read().width();
            let resizing = *hold_w.read() != 0.0;
            let widths = widths.read();
            if vp_w <= 0. {
                return;
            }
            let x0 = -(sx as f32);
            let (mut start, mut end) = col_range(&widths, x0 - OVERSCAN_W, x0 + vp_w + OVERSCAN_W);
            if resizing {
                let w = *col_window.peek();
                start = start.min(w.start);
                end = end.max(w.end);
            }
            col_window.set_if_modified(ColWindow::resolve(&widths, start, end));
        });

        let sel = use_consume::<State<Selection>>();
        let config = use_config_station();
        let anchor = use_state(|| None::<usize>);
        let drag = use_state(|| false);
        let mut shift = use_state(|| false);
        let mut meta = use_state(|| false);
        let a11y = use_a11y();
        let cell_view = use_state(|| None::<CellValue>);
        let record_view = use_state(|| None::<usize>);

        let theme = get_theme!(&self.theme, DataGridThemePreference, "datagrid");
        let cell_pad = density.padding(&theme);
        let row_h = CELL_LINE_H + cell_pad.vertical();

        let data: Rc<GridData> = match &self.view {
            PageRead::Ready(data) => data.clone(),
            PageRead::Failed(err) => {
                return ErrorState::new(err.clone(), self.tab).into_element();
            }
            PageRead::Loading => {
                return rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .center()
                    .child(CircularLoader::new().size(30.))
                    .into_element();
            }
        };
        let sel_ctl = SelCtl {
            sel,
            anchor,
            drag,
            shift,
            meta,
            nrows: data.rows.len(),
            ncols: data.columns.len(),
            a11y,
        };

        let min_w = GUTTER_W + widths.read().iter().sum::<f32>() + TRAIL_W;

        let header = HeaderRow {
            data: data.clone(),
            widths,
            seed_w,
            col_window,
            controller,
            viewport,
            hold_w,
            sel: sel_ctl,
            sort: self.sort,
            theme: theme.clone(),
        };

        let len = data.rows.len();
        let row_base = self.row_base;
        let theme_b = theme.clone();
        let body_data = (data.clone(), self.row_nums.clone(), zebra, cell_pad);
        let body = VirtualScrollView::new_with_data(body_data, move |item, page| {
            let (data, row_nums, zebra, cell_pad) = page;
            Row {
                index: item.index,
                data: data.clone(),
                row_nums: row_nums.clone(),
                row_base,
                widths,
                seed_w,
                col_window,
                sel: sel_ctl,
                cell_view,
                record_view,
                row_h,
                cell_pad: *cell_pad,
                zebra: *zebra,
                theme: theme_b.clone(),
                key: DiffKey::None,
            }
            .key(item.index)
            .into_element()
        })
        .direction(Direction::Vertical)
        .item_size(row_h)
        .length(len)
        .width(Size::fill())
        .height(Size::flex(1.))
        .wheel_axis_lock(SCROLL_AXIS_LOCK);

        let scroll = ScrollView::new_controlled(controller)
            .direction(Direction::Horizontal)
            .wheel_axis_lock(SCROLL_AXIS_LOCK)
            .child(
                rect()
                    .width(Size::fill())
                    .min_width(Size::px(min_w.max(hold_w())))
                    .height(Size::fill())
                    .content(Content::Flex)
                    .background(theme.background)
                    .child(header)
                    .child(Divider::horizontal().color(theme.header_divider_fill))
                    .child(body),
            );
        rect()
            .expanded()
            .a11y_id(a11y)
            .a11y_focusable(true)
            .on_key_down({
                let data = data.clone();
                let row_nums = self.row_nums.clone();
                on_commands(config, move |cmd| match cmd {
                    Command::SelectAll => {
                        sel_ctl.all();
                        true
                    }
                    Command::Copy => copy::copy_selection(
                        TextFormat::Tsv,
                        &data,
                        row_nums.as_ref().map(|n| n.as_slice()),
                        row_base,
                        &sel_ctl.sel.peek(),
                    ),
                    _ => false,
                })
            })
            .on_sized(move |e: Event<SizedEventData>| viewport.set_if_modified(e.area))
            .on_pointer_down(move |e: Event<PointerEventData>| {
                if e.data().is_primary() {
                    sel_ctl.clear();
                }
            })
            .on_global_pointer_down(move |e: Event<PointerEventData>| {
                if !e.data().is_primary() {
                    return;
                }
                let loc = e.global_location();
                let vp = *viewport.peek();
                let (x, y) = (loc.x as f32, loc.y as f32);
                if x < vp.min_x() as f32
                    || x > vp.max_x() as f32
                    || y < vp.min_y() as f32
                    || y > vp.max_y() as f32
                {
                    sel_ctl.clear();
                }
            })
            .on_global_pointer_press(move |_: Event<PointerEventData>| sel_ctl.end_drag())
            .on_global_key_down({
                let find = self.find;
                let mut commands = on_commands(config, move |cmd| match cmd {
                    Command::Cancel if cell_view.peek().is_some() => {
                        let mut cell_view = cell_view;
                        cell_view.set(None);
                        true
                    }
                    Command::Cancel if record_view.peek().is_some() => {
                        let mut record_view = record_view;
                        record_view.set(None);
                        true
                    }
                    Command::Find => {
                        find.toggle();
                        true
                    }
                    Command::Cancel if *find.open.peek() => {
                        find.dismiss();
                        true
                    }
                    Command::Cancel => {
                        let had = *sel_ctl.sel.peek() != Selection::None;
                        if had {
                            sel_ctl.clear();
                        }
                        had
                    }
                    _ => false,
                });
                move |e: Event<KeyboardEventData>| {
                    match &e.key {
                        Key::Named(NamedKey::Shift) => shift.set(true),
                        Key::Named(NamedKey::Meta | NamedKey::Control) => {
                            meta.set(true);
                        }
                        _ => {}
                    }
                    commands(e);
                }
            })
            .on_global_key_up(move |e: Event<KeyboardEventData>| match &e.key {
                Key::Named(NamedKey::Shift) => shift.set(false),
                Key::Named(NamedKey::Meta | NamedKey::Control) => meta.set(false),
                _ => {}
            })
            .child(
                ResultsToolbar::new(self.tab, self.find, self.export.clone())
                    .shape(self.shape.clone()),
            )
            .child(scroll)
            .maybe_child(
                cell_view
                    .read()
                    .clone()
                    .map(|value| CellView::new(value, cell_view)),
            )
            .maybe_child((*record_view.read()).and_then(|row| {
                (!data.rows.is_empty()).then(|| {
                    RecordView::new(
                        row.min(data.rows.len() - 1),
                        record_view,
                        data.clone(),
                        self.row_nums.clone(),
                        row_base,
                        self.total,
                    )
                })
            }))
            .into_element()
    }
}

#[cfg(test)]
mod window {
    use super::{col_range, ColWindow, GUTTER_W};

    /// Ten 100px columns behind the gutter: column `ci` spans
    /// `[GUTTER_W + 100·ci, GUTTER_W + 100·(ci+1))`. The window is the half-open range of
    /// columns intersecting the asked-for band, empty when the band misses them all.
    #[test]
    fn col_range_is_the_intersecting_span() {
        let widths = [100.0f32; 10];
        assert_eq!(col_range(&widths, 0., 2000.), (0, 10));
        assert_eq!(col_range(&widths, 300., 500.), (2, 5));
        assert_eq!(col_range(&widths, 2000., 2400.), (10, 10));
        assert_eq!(col_range(&widths, 0., GUTTER_W), (0, 0));
        assert_eq!(col_range(&[], 0., 100.), (0, 0));
    }

    /// The resolved window carries the off-window extents — the spacers' widths — so the
    /// consumers never re-derive them.
    #[test]
    fn resolve_carries_the_spacer_extents() {
        let widths = [100.0f32; 10];
        let win = ColWindow::resolve(&widths, 2, 5);
        assert_eq!((win.start, win.end), (2, 5));
        assert_eq!((win.lead, win.tail), (200., 500.));
        assert_eq!(ColWindow::resolve(&widths, 0, 10).tail, 0.);
        assert_eq!(ColWindow::resolve(&widths, 10, 10).lead, 1000.);
    }
}
