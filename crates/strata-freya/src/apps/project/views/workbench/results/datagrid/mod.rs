//! The results **datagrid** — our custom virtualized grid (distinct from Freya's built-in `Table`,
//! which renders a component per cell with no virtualization). A [`VirtualScrollView`] over the rows
//! (only the ~viewport rows are ever built) with hand-rolled `rect` cells: a row-number gutter,
//! per-column resizable widths, a type-labelled sticky header, type-coloured cell text, zebra rows,
//! column / row / header dividers, per-cell hover, and Excel-style selection. Horizontal scroll pans
//! header + body together for wide tables.
//!
//! Layout: this file owns the [`DataGrid`] component + its render, the shared constants, and the
//! `datagrid` [`define_theme!`]. The pieces live in submodules — [`model`] (grid data + type→colour +
//! density), [`cell`] (the body / gutter / `#` cell), [`header`] (the column header +
//! resize grip) — and the selection model is the sibling `super::selection`.
//!
//! Every colour is a `datagrid` component token (`define_theme!` / `get_theme!`) — no semantic sheet
//! reads. Fed by the Run's real [`GridData`]: the results pane resolves the current page (page 1
//! from the Run's own output, anything else via the cached `FetchSnapshotPage`) and hands it in as
//! a [`PageRead`] — the grid itself never touches the engine.

use std::rc::Rc;

use freya::components::{define_theme, get_theme, CircularLoader};
use freya::prelude::*;

use super::error::ErrorState;
use super::selection::{CellRole, SelCtl, Selection};
use super::toolbar::DataGridToolbar;
use crate::components::divider::Divider;

mod cell;
mod header;
mod model;

use cell::Cell;
use header::HeaderCell;
use model::{Density, KindColors};
pub use model::{GridData, PageRead};

const HEADER_H: f32 = 46.;
const GUTTER_W: f32 = 52.; // the `#` row-number column (matches the Dioxus `.hnum` / `.rnum`)
const TRAIL_W: f32 = 48.; // dead space after the last column so its resize grip stays reachable/draggable
const CELL_LINE_H: f32 = 16.; // mono cell line box; a row is this tall plus the density's top+bottom padding
const DEFAULT_COL_W: f32 = 168.;
const MIN_COL_W: f32 = 56.;
const MAX_COL_W: f32 = 2000.;
const GRIP_W: f32 = 6.; // resize hot-zone width on a column's right edge
const EDGE_MARGIN: f32 = 36.; // how close to the viewport edge a resize drag starts auto-scrolling
const EDGE_STEP: f32 = 24.; // px scrolled per pointer-move tick while resizing at an edge
// Wheel axis-lock threshold: a scroll commits to whichever axis dominates, so a mostly-vertical
// gesture never drifts the horizontal pan (and vice-versa). 1.0 = lock to the larger axis; raise it
// to allow more diagonal freedom before locking.
const SCROLL_AXIS_LOCK: f32 = 1.0;
// Content auto-fit (double-click a resize grip): a char-count estimate, à la the Dioxus `col_autofit`.
const AUTOFIT_CHAR_W: f32 = 7.6; // mono char-width estimate
const AUTOFIT_PAD: f32 = 28.; // cell horizontal padding + affordance

/// Per-column content auto-fit width — `max(header name + 3, widest cell) × char-width + padding`,
/// clamped to the resize bounds. Recomputed per page (a grip double-click fits the *visible* cells).
fn autofit_widths(data: &GridData) -> Vec<f32> {
    (0..data.columns.len())
        .map(|ci| {
            let mut max_len = data.columns[ci].name.chars().count() + 3;
            for row in &data.rows {
                if let Some(cell) = row.get(ci) {
                    max_len = max_len.max(cell.text.chars().count());
                }
            }
            (max_len as f32 * AUTOFIT_CHAR_W + AUTOFIT_PAD).clamp(MIN_COL_W, MAX_COL_W)
        })
        .collect()
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
        type_str_color: Color,
        type_num_color: Color,
        type_bool_color: Color,
        type_ts_color: Color,
        type_struct_color: Color,
        type_list_color: Color,
        type_map_color: Color,
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
    /// The page the Run itself returned — the source of the result schema (widths seed off it).
    run: Rc<GridData>,
    /// The resolved current page.
    view: PageRead,
    /// Absolute index of the page's first row (0-based) — the gutter continues across pages.
    row_base: usize,
    density: Density,
    pub(crate) theme: Option<DataGridThemePartial>,
}

impl DataGrid {
    pub fn new(run: Rc<GridData>, view: PageRead, row_base: usize) -> Self {
        Self { run, view, row_base, density: Density::Comfortable, theme: None }
    }

    /// Cell padding density (default [`Comfortable`](Density::Comfortable)). Wire to a user setting
    /// when one exists.
    pub(crate) fn density(mut self, density: Density) -> Self {
        self.density = density;
        self
    }
}

impl Component for DataGrid {
    fn render(&self) -> impl IntoElement {
        // Per-column widths, seeded from the run's schema at mount and mutated by the grips. They
        // live at this level — not per page — so a page flip keeps the user's resizes (the column
        // set is fixed for the life of the snapshot).
        let n = self.run.columns.len();
        let widths = use_state(move || vec![DEFAULT_COL_W; n]);
        // One horizontal scroll controller, shared with the resize grips (so they can auto-scroll the
        // view while dragging past an edge), plus the grid viewport in screen coords for edge detection.
        let controller = use_scroll_controller(ScrollConfig::default);
        let mut viewport = use_state(Area::default);
        // While a column resize is dragging, the content width is held at its high-water mark here (0 =
        // not resizing) so shrinking a column can't shrink the scroll extent mid-drag — which reflowed
        // the view and made the drag janky. The grips write it; it settles back to `min_w` on release.
        let hold_w = use_state(|| 0.0f32);

        // ── selection ──────────────────────────────────────────────────────────────────────────────
        // Shared selection state + a Copy controller the cells call on pointer events. Freya pointer
        // events carry no modifiers, so shift / ⌘ are tracked via the root's global key up/down below.
        let sel = use_consume::<State<Selection>>();
        let anchor = use_state(|| None::<usize>);
        let drag = use_state(|| false);
        let mut shift = use_state(|| false);
        let mut meta = use_state(|| false);

        // The datagrid theme is used directly (no parallel palette): the header + outer scroll borrow
        // it, and the body closure — which must own its captures — takes a cheap clone (all `Color`).
        let theme = get_theme!(&self.theme, DataGridThemePreference, "datagrid");
        // Cell padding comes from the theme via the density selector; the row height follows its
        // vertical extent so the virtual scroller's item size matches.
        let cell_pad = self.density.padding(&theme);
        let row_h = CELL_LINE_H + cell_pad.vertical();

        // The page to render, as the results pane resolved it. A page read in flight (or failed)
        // replaces the grid body; the widths above survive it. (These early returns sit below
        // every hook, so the hook order is stable across states.)
        let data: Rc<GridData> = match &self.view {
            PageRead::Ready(data) => data.clone(),
            PageRead::Failed(err) => {
                return ErrorState::new(err.clone()).into_element();
            }
            // A page read in flight — just the spinner: a snapshot page fetch is not a
            // cancellable run, so it doesn't wear the full running state (timer + Cancel).
            PageRead::Loading => {
                return rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .center()
                    .child(CircularLoader::new().size(30.))
                    .into_element();
            }
        };
        // Per-column content auto-fit widths (grip double-click), from this page's cells.
        let autofit = autofit_widths(&data);
        let sel_ctl = SelCtl {
            sel,
            anchor,
            drag,
            shift,
            meta,
            nrows: data.rows.len(),
            ncols: data.columns.len(),
        };
        // (No selection snapshot here: each cell reads the selection reactively and styles itself, so a
        // selection change re-renders only the affected cells — the grid itself doesn't re-render.)

        // The columns' natural span, including the trailing dead zone (so the last grip stays reachable).
        // It's the content's `min-width` (à la CSS `min-width: max-content`): the header + rows are `fill`
        // so they fill the viewport when the columns are narrower, and overflow into horizontal scroll
        // when wider — a `flex` trailing cell in each row absorbs whatever slack is left.
        let min_w = GUTTER_W + widths.read().iter().sum::<f32>() + TRAIL_W;

        // Sticky header: `#` gutter + per-column name/type/chevron + resize grips. Pans horizontally
        // with the body (shared horizontal scroll) but not vertically (it sits above the scroll region).
        let mut header = rect()
            .width(Size::fill())
            .height(Size::px(HEADER_H))
            .direction(Direction::Horizontal)
            .content(Content::Flex)
            .background(theme.header_background)
            .child(Cell {
                width: Size::px(GUTTER_W),
                text: "#".to_string(),
                color: theme.header_label_color,
                mono: false,
                cross: Alignment::Center,
                pad: Gaps::default(),
                hover_bg: theme.header_hover_background,
                divider: theme.header_divider_fill,
                role: CellRole::Corner,
                sel: sel_ctl,
                sel_border: theme.selection_border_fill,
                active_color: None,
                active_background: None,
            });
        for (ci, col) in data.columns.iter().enumerate() {
            let w = widths.read().get(ci).copied().unwrap_or(DEFAULT_COL_W);
            header = header.child(HeaderCell {
                index: ci,
                name: col.name.clone(),
                dtype: col.dtype.clone(),
                w,
                widths,
                controller,
                viewport,
                hold_w,
                sel: sel_ctl,
                name_color: theme.header_color,
                active_color: theme.header_active_color,
                type_color: col.kind.type_color(&theme),
                arrow: theme.arrow_fill,
                divider: theme.header_divider_fill,
                grip: theme.selection_border_fill,
                hover_bg: theme.header_hover_background,
                active_bg: theme.header_active_background,
                autofit_w: autofit.get(ci).copied().unwrap_or(DEFAULT_COL_W),
            });
        }
        // Trailing dead space: keeps the last column's resize grip clear of the content's right edge so
        // it stays reachable, and gives somewhere to drag when widening the last column.
        header = header.child(rect().width(Size::flex(1.)).min_width(Size::px(TRAIL_W)).height(Size::fill()));

        // Virtualized body: the builder runs only for rows scrolled into view; it reads `widths` fresh
        // so a resize reflows every visible row. The page's rows ride as `builder_data` (not a plain
        // capture) so flipping pages — same length, new cells — rebuilds the visible rows.
        let len = data.rows.len();
        // Absolute row numbers: the gutter continues across pages (page 2 starts at page_size + 1).
        let row_base = self.row_base;
        let theme_b = theme.clone();
        let body = VirtualScrollView::new_with_data(data.clone(), move |index, data| {
            let mut cells = rect()
                .width(Size::fill())
                .height(Size::flex(1.))
                .direction(Direction::Horizontal)
                .content(Content::Flex)
                .child(Cell {
                    width: Size::px(GUTTER_W),
                    text: (row_base + index + 1).to_string(),
                    color: theme_b.gutter_color,
                    mono: false,
                    cross: Alignment::Center,
                    pad: Gaps::default(),
                    hover_bg: theme_b.gutter_active_background,
                    divider: theme_b.column_divider_fill,
                    role: CellRole::Row(index),
                    sel: sel_ctl,
                    sel_border: theme_b.selection_border_fill,
                    active_color: Some(theme_b.gutter_active_color),
                    active_background: Some(theme_b.gutter_active_background),
                });

            for (ci, col) in data.columns.iter().enumerate() {
                let w = widths.read().get(ci).copied().unwrap_or(DEFAULT_COL_W);
                let cell = &data.rows[index][ci];
                cells = cells.child(Cell {
                    width: Size::px(w),
                    text: cell.text.clone(),
                    // Nulls render dimmed (the model keeps the flag exactly for this), in the
                    // gutter's muted tone; everything else takes its type colour.
                    color: if cell.null {
                        theme_b.gutter_color
                    } else {
                        col.kind.cell_color(&theme_b)
                    },
                    mono: true,
                    cross: Alignment::Start,
                    pad: Gaps::new(0., cell_pad.right(), 0., cell_pad.left()),
                    hover_bg: theme_b.cell_hover_background,
                    divider: theme_b.column_divider_fill,
                    role: CellRole::Data(index, ci),
                    sel: sel_ctl,
                    sel_border: theme_b.selection_border_fill,
                    active_color: None,
                    active_background: None,
                });
            }
            // Trailing dead space (matches the header) so the row extends past the last column.
            cells = cells.child(rect().width(Size::flex(1.)).min_width(Size::px(TRAIL_W)).height(Size::fill()));

            rect()
                .width(Size::fill())
                .height(Size::px(row_h))
                .background(if index % 2 == 1 {
                    theme_b.zebra_row_background
                } else {
                    theme_b.row_background
                })
                .content(Content::Flex)
                .child(cells)
                .child(Divider::horizontal().color(theme_b.divider_fill))
                .into()
        })
        .direction(Direction::Vertical)
        .item_size(row_h)
        .length(len)
        .width(Size::fill())
        .height(Size::flex(1.))
        // Commit to the vertical axis so a slightly-diagonal scroll down doesn't scroll the body
        // sideways (or swallow a horizontal pan meant for the outer view).
        .wheel_axis_lock(SCROLL_AXIS_LOCK);

        // Horizontal scroll wraps header + body so wide tables pan together; the body's own
        // VirtualScrollView owns vertical scroll. Height fills the space the parent (results panel,
        // minus the fixed status bar) hands down, so `flex(1)` on the body resolves.
        let scroll = ScrollView::new_controlled(controller)
            .direction(Direction::Horizontal)
            // The header sits in this outer scroll, so a scroll down over it would otherwise pan the
            // table sideways; the lock keeps a vertical gesture from drifting the horizontal position.
            .wheel_axis_lock(SCROLL_AXIS_LOCK)
            .child(
                rect()
                    .width(Size::fill())
                    // Held at the drag high-water mark during a resize so the extent can't shrink
                    // mid-drag; `min_w` (the live natural span) otherwise.
                    .min_width(Size::px(min_w.max(hold_w())))
                    .height(Size::fill())
                    .content(Content::Flex)
                    .background(theme.background)
                    .child(header)
                    .child(Divider::horizontal().color(theme.header_divider_fill))
                    .child(body),
            );
        // Measure the viewport (screen coords) so a resize grip knows when the drag nears an edge.
        rect()
            .expanded()
            .on_sized(move |e: Event<SizedEventData>| viewport.set(e.area))
            // A primary press that reaches here (not consumed by a cell) is on the grid background →
            // clear. A release anywhere ends a drag-paint. Shift / ⌘ are tracked globally (pointer
            // events carry no modifiers), and Esc clears.
            .on_pointer_down(move |e: Event<PointerEventData>| {
                if e.data().is_primary() {
                    sel_ctl.clear();
                }
            })
            // …and a press *anywhere else in the app* — outside the grid's viewport — clears too, so
            // clicking off into the editor / sidebar / tabs deselects. Cells sit inside the bounds, so
            // this skips them (their own handler sets the selection).
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
            .on_global_key_down(move |e: Event<KeyboardEventData>| match &e.key {
                Key::Named(NamedKey::Shift) => shift.set(true),
                Key::Named(NamedKey::Meta) | Key::Named(NamedKey::Control) => meta.set(true),
                Key::Named(NamedKey::Escape) => sel_ctl.clear(),
                _ => {}
            })
            .on_global_key_up(move |e: Event<KeyboardEventData>| match &e.key {
                Key::Named(NamedKey::Shift) => shift.set(false),
                Key::Named(NamedKey::Meta) | Key::Named(NamedKey::Control) => meta.set(false),
                _ => {}
            })
            .child(DataGridToolbar)
            .child(scroll)
            .into_element()
    }
}
