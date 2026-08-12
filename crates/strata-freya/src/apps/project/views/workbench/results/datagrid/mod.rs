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
const GUTTER_W: f32 = 52.; // the `#` row-number column (matches the Dioxus `.hnum` / `.rnum`)
const TRAIL_W: f32 = 48.; // dead space after the last column so its resize grip stays reachable/draggable
const CELL_LINE_H: f32 = 16.; // mono cell line box; a row is this tall plus the density's top+bottom padding
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
const GRIP_W: f32 = 6.; // resize hot-zone width on a column's right edge
const EDGE_MARGIN: f32 = 36.; // how close to the viewport edge a resize drag starts auto-scrolling
const EDGE_STEP: f32 = 24.; // px scrolled per pointer-move tick while resizing at an edge
                            // Wheel axis-lock threshold: a scroll commits to whichever axis dominates, so a mostly-vertical
                            // gesture never drifts the horizontal pan (and vice-versa). 1.0 = lock to the larger axis; raise it
                            // to allow more diagonal freedom before locking.
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
    // No column's right edge cleared `lo` — the band sits past them all, so the range is
    // empty at `end`.
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
    // The grid reads three settings, owns the selection controller and its key handlers, and
    // builds header, gutter and virtualized rows in one tree. The handlers close over the
    // controller built above them, so splitting the body would mean threading it back in.
    #[allow(clippy::too_many_lines)]
    fn render(&self) -> impl IntoElement {
        // The grid's three user settings, from the app-global config. This **subscribes**
        // (`ConfigChan::Settings` + `.read()`) rather than peeking the station like the key
        // handlers below: flipping zebra or density in Settings has to repaint every open grid
        // there and then, not whenever something else happens to re-render it.
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
        // **The** column width for this grid: what every column starts at, and what a width
        // lookup past the end of `widths` answers with (`HeaderRow` / `Row` / `ColGrip` take it
        // as `seed_w`). One number, so an out-of-range fallback can never be a different width
        // than the one the user chose.
        //
        // Read *once*, at mount: raising the setting later must not blow away resizes the user
        // has since made — it is the starting width, not a live one.
        let seed_w = use_hook(move || {
            // Clamped to the same bounds a resize drag honours — the setting is hand-editable
            // JSON, and a 0 (or 5000) px seed would make the grid unusable with no control yet
            // to fix it from. A non-finite value can't be clamped at all (`f32::clamp` passes
            // NaN straight through), so it falls back to the grid's own `DEFAULT_COL_W` — which
            // is the number the setting's default mirrors anyway.
            if setting_w.is_finite() {
                setting_w.clamp(MIN_COL_W, MAX_COL_W)
            } else {
                DEFAULT_COL_W
            }
        });

        // Per-column widths, seeded from the run's schema at mount and mutated by the grips. They
        // live at this level — not per page — so a page flip keeps the user's resizes (the column
        // set is fixed for the life of the snapshot).
        let n = self.run.columns.len();
        let widths = use_state(move || vec![seed_w; n]);
        // One horizontal scroll controller, shared with the resize grips (so they can auto-scroll the
        // view while dragging past an edge), plus the grid viewport in screen coords for edge detection.
        let controller = use_scroll_controller(ScrollConfig::default);
        let mut viewport = use_state(Area::default);
        // While a column resize is dragging, the content width is held at its high-water mark here (0 =
        // not resizing) so shrinking a column can't shrink the scroll extent mid-drag — which reflowed
        // the view and made the drag janky. The grips write it; it settles back to `min_w` on release.
        let hold_w = use_state(|| 0.0f32);

        // ── the column window ──────────────────────────────────────────────────────────────────────
        // Which columns the header + rows actually build: only the spans intersecting the horizontal
        // viewport (± OVERSCAN_W), so the tree stays O(visible) in both directions at hundreds of
        // columns. Derived in a side effect, not in this render — reading the controller's position
        // subscribes, and the grid must not rebuild per scrolled pixel — and written through
        // `set_if_modified`, so the consumers re-render only when the window actually moves. Seeded
        // to the columns covering SEED_COVER_W from the left — scroll starts at 0 and no viewport
        // is that wide, so the first frames (before the effect's first run has a sized viewport)
        // are never blank without building every column of a very wide result.
        let mut col_window = use_state(move || {
            let end = n.min((SEED_COVER_W / seed_w).ceil() as usize);
            // Every width is `seed_w` at mount, so the seed's extents need no widths read.
            ColWindow {
                start: 0,
                end,
                lead: 0.,
                tail: (n - end) as f32 * seed_w,
            }
        });
        use_side_effect(move || {
            // Every input is read before any return, so the effect stays subscribed to all of
            // them on every path.
            let (sx, _): (i32, i32) = controller.into();
            let vp_w = viewport.read().width();
            let resizing = *hold_w.read() != 0.0;
            let widths = widths.read();
            if vp_w <= 0. {
                return; // pre-layout: keep the seed until the viewport has a size
            }
            // Scroll x is ≤ 0 (the content's offset), so -sx is the visible band's left edge.
            let x0 = -(sx as f32);
            let (mut start, mut end) = col_range(&widths, x0 - OVERSCAN_W, x0 + vp_w + OVERSCAN_W);
            if resizing {
                // Grow-only while a resize drag holds the extent: recomputing mid-drag could
                // unmount the very grip driving the drag (its global listeners with it), while
                // a drag that auto-scrolls must still get the columns it reveals.
                let w = *col_window.peek();
                start = start.min(w.start);
                end = end.max(w.end);
            }
            col_window.set_if_modified(ColWindow::resolve(&widths, start, end));
        });

        // ── selection ──────────────────────────────────────────────────────────────────────────────
        // Shared selection state + a Copy controller the cells call on pointer events. Freya pointer
        // events carry no modifiers, so shift / ⌘ are tracked via the root's global key up/down below.
        let sel = use_consume::<State<Selection>>();
        let config = use_config_station();
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
        let cell_pad = density.padding(&theme);
        let row_h = CELL_LINE_H + cell_pad.vertical();

        // The page to render, as the results pane resolved it. A page read in flight (or failed)
        // replaces the grid body; the widths above survive it. (These early returns sit below
        // every hook, so the hook order is stable across states.)
        let data: Rc<GridData> = match &self.view {
            PageRead::Ready(data) => data.clone(),
            PageRead::Failed(err) => {
                return ErrorState::new(err.clone(), self.tab).into_element();
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
            seed_w,
            col_window,
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
        // retyping the filter rebuilds the visible rows. The two grid settings ride there for
        // the same reason: a density flip usually moves `item_size` too (which the view *does*
        // compare), but a zebra flip changes nothing else about the view at all — captured,
        // either would leave the built rows dressed the old way until something else rebuilt
        // them.
        let len = data.rows.len();
        // Absolute row numbers: the gutter continues across pages (page 2 starts at page_size + 1).
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
            // Keyed by the page row: the differ matches keyed siblings across positions, so a
            // scroll step *moves* the surviving rows (props unchanged — no re-render) and builds
            // only the row it revealed. Unkeyed, every row would land on a different position's
            // scope and rebuild all its cells per step.
            .key(item.index)
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
            // `set_if_modified`: torin re-emits `Sized` on re-measures that moved nothing, and
            // the column-window effect subscribes to this — a plain `set` would re-run its
            // O(cols) scan per relayout for a byte-identical viewport.
            .on_sized(move |e: Event<SizedEventData>| viewport.set_if_modified(e.area))
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
                let mut commands = on_commands(config, move |cmd| match cmd {
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
            // The open nested-cell modal (an overlay layer — it renders above everything).
            .maybe_child(
                cell_view
                    .read()
                    .clone()
                    .map(|value| CellView::new(value, cell_view)),
            )
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

#[cfg(test)]
mod window {
    use super::{col_range, ColWindow, GUTTER_W};

    /// Ten 100px columns behind the gutter: column `ci` spans
    /// `[GUTTER_W + 100·ci, GUTTER_W + 100·(ci+1))`. The window is the half-open range of
    /// columns intersecting the asked-for band, empty when the band misses them all.
    #[test]
    fn col_range_is_the_intersecting_span() {
        let widths = [100.0f32; 10];
        // The whole content visible.
        assert_eq!(col_range(&widths, 0., 2000.), (0, 10));
        // A band over the middle: 300 falls in column 2's span, 500 in column 4's.
        assert_eq!(col_range(&widths, 300., 500.), (2, 5));
        // Scrolled past every column (the trailing dead zone): empty at the end.
        assert_eq!(col_range(&widths, 2000., 2400.), (10, 10));
        // A band inside the gutter only: empty at the start.
        assert_eq!(col_range(&widths, 0., GUTTER_W), (0, 0));
        // No columns at all.
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
        // Degenerate ranges: everything windowed, and nothing windowed.
        assert_eq!(ColWindow::resolve(&widths, 0, 10).tail, 0.);
        assert_eq!(ColWindow::resolve(&widths, 10, 10).lead, 1000.);
    }
}
