//! The results **datagrid** — our custom virtualized grid (distinct from Freya's built-in `Table`,
//! which renders a component per cell with no virtualization). A [`VirtualScrollView`] over the rows
//! (only the ~viewport rows are ever built) with hand-rolled `rect` cells: a row-number gutter,
//! per-column resizable widths, a type-labelled sticky header, type-coloured cell text, zebra rows,
//! column / row / header dividers, per-cell hover, and Excel-style selection. Horizontal scroll pans
//! header + body together for wide tables.
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

use strata_core::config::{Command, Settings};
use strata_core::engine::serialize::TextFormat;

use super::cell_view::{CellValue, CellView};
use super::copy;
use super::error::ErrorState;
use super::find::FindState;
use super::record_view::RecordView;
use super::selection::{SelCtl, Selection};
use super::sort::SortState;
use super::toolbar::ResultsToolbar;
use crate::apps::project::state::TabId;
use crate::components::divider::Divider;

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
    density: Density,
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
            density: Density::Comfortable,
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
        let settings = use_consume::<State<Settings>>();
        let anchor = use_state(|| None::<usize>);
        let drag = use_state(|| false);
        let mut shift = use_state(|| false);
        let mut meta = use_state(|| false);
        // The grid surface's a11y identity (P2-11): selection interactions focus it (via SelCtl),
        // and the focused `on_key_down` below is what routes ⌘A / ⌘C here — text surfaces keep
        // both whenever *they* hold the focus, with no menu-side coordination.
        let a11y = use_a11y();
        // The nested-cell view (P2-12): the value a double-clicked nested cell snapshotted;
        // `None` = closed. Lives here — beside the widths — so it survives page flips, and the
        // Esc arm below can arbitrate it ahead of find / the selection.
        let cell_view = use_state(|| None::<CellValue>);
        // The record view (P2-10): the page row index a double-clicked gutter cell opened;
        // `None` = closed. Same placement rationale — but unlike the snapshotted cell view it
        // is a *live* pointer: the modal renders whatever the current page holds at that index.
        let record_view = use_state(|| None::<usize>);

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
        // (No selection snapshot here: each cell reads the selection reactively and styles itself, so a
        // selection change re-renders only the affected cells — the grid itself doesn't re-render.)

        // The columns' natural span, including the trailing dead zone (so the last grip stays reachable).
        // It's the content's `min-width` (à la CSS `min-width: max-content`): the header + rows are `fill`
        // so they fill the viewport when the columns are narrower, and overflow into horizontal scroll
        // when wider — a `flex` trailing cell in each row absorbs whatever slack is left.
        let min_w = GUTTER_W + widths.read().iter().sum::<f32>() + TRAIL_W;

        // Sticky header: the `#` corner + column cells + resize grips, as one component
        // ([`HeaderRow`] owns the auto-fit measurement too).
        let header = HeaderRow {
            data: data.clone(),
            widths,
            controller,
            viewport,
            hold_w,
            sel: sel_ctl,
            sort: self.sort,
            theme: theme.clone(),
        };

        // Virtualized body: the builder runs only for rows scrolled into view, building a
        // [`Row`] per visible index — [`Row`] reads `widths` (and each cell the selection)
        // reactively, so resizes and selection changes repaint without this builder re-running.
        // The page's rows — and the find filter's gutter numbers, which must swap in lockstep
        // with them — ride as `builder_data` (not a plain capture) so flipping pages or
        // retyping the filter rebuilds the visible rows.
        let len = data.rows.len();
        // Absolute row numbers: the gutter continues across pages (page 2 starts at page_size + 1).
        let row_base = self.row_base;
        let theme_b = theme.clone();
        let body_data = (data.clone(), self.row_nums.clone());
        let body = VirtualScrollView::new_with_data(body_data, move |index, page| {
            let (data, row_nums) = page;
            Row {
                index,
                data: data.clone(),
                row_nums: row_nums.clone(),
                row_base,
                widths,
                sel: sel_ctl,
                cell_view,
                record_view,
                row_h,
                cell_pad,
                theme: theme_b.clone(),
            }
            .into_element()
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
            // The grid is an a11y-focusable surface: selection interactions focus it (SelCtl),
            // and keyboard dispatch routes location-less key events by a11y focus — so the
            // focused `on_key_down` below claims the edit chords exactly while the grid holds
            // focus, and the SQL editor / inputs keep them whenever they do.
            .a11y_id(a11y)
            .a11y_focusable(true)
            .on_key_down({
                // The grid-focused edit chords (P2-11): ⌘A selects every cell, ⌘C copies the
                // selection as TSV (declining when empty, so the press stays unconsumed).
                let data = data.clone();
                let row_nums = self.row_nums.clone();
                crate::keymap::on_commands(settings, move |cmd| match cmd {
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
            .on_global_key_down({
                // The results scope's shortcuts (P2-09): ⌘F toggles the toolbar's find
                // popover; Esc dismisses that popover first (this node is the popover's
                // ancestor, so it must arbitrate — the popover's own listener would fire
                // too late), then falls through to clearing the selection — the tail of
                // the dismiss chain (menus, a rename, and a running body all sit earlier
                // in document order and consume first). Declines when neither applies,
                // leaving the press unconsumed. The modifier mirroring is separate
                // bookkeeping for the pointer events (which carry no modifiers).
                let find = self.find;
                let mut commands = crate::keymap::on_commands(settings, move |cmd| match cmd {
                    // The modals sit above the popover, so they dismiss first (only one is
                    // ever open — each opens off its own double-click target).
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
                        Key::Named(NamedKey::Meta) | Key::Named(NamedKey::Control) => {
                            meta.set(true)
                        }
                        _ => {}
                    }
                    commands(e);
                }
            })
            .on_global_key_up(move |e: Event<KeyboardEventData>| match &e.key {
                Key::Named(NamedKey::Shift) => shift.set(false),
                Key::Named(NamedKey::Meta) | Key::Named(NamedKey::Control) => meta.set(false),
                _ => {}
            })
            .child(ResultsToolbar::new(self.tab, self.find))
            .child(scroll)
            // The open nested-cell modal (an overlay layer — it renders above everything).
            .maybe_child(cell_view.read().clone().map(|value| CellView::new(value, cell_view)))
            // The open record view (P2-10) — a live pointer into the current page, clamped in
            // case a page flip / filter change shortened the page under it (an emptied page
            // has no row to show, so the modal simply doesn't render until one is back).
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

