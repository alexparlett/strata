//! The results grid — type-coloured cells, zebra striping, find-filtering, Excel-style
//! **selection** (Rz3), and the nested-cell JSON view. Selection is page-local (the visible
//! rows), lives in `runs.sel`, and drives the status-bar aggregate. The grid owns the
//! nested-cell view signal locally and renders its own `CellDialog`.

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use dioxus_code::{Code, SourceCode};
use std::collections::HashSet;
use std::rc::Rc;

use crate::action::{dispatch, Action};
use crate::engine::Cell;
use crate::runs::Selection;
use crate::serialize::TextFormat;
use crate::session::WorkspaceId;
use crate::state::{AppState, ResizeTarget, Resizing};
use crate::ui::components::{
    Badge, BadgeVariant, ContextMenu, Dialog, DropdownMenu, Eyebrow, Icon, IconButton,
    IconButtonVariant, MenuItem, Meta, MonoValue, Point, Readout, RectAlign, Spacer,
};
use crate::ui::icons::{IconName, IconSize};

thread_local! {
    /// Set by a cell / row-number / column-header `onmousedown`; read (and reset) by the
    /// grid-scroll's own `onmousedown`, which bubbles *after* them. If nothing set it, the
    /// press landed on the empty grid background (e.g. below the last row, when the rows
    /// don't fill the viewport) → clear the selection.
    static PRESSED_TARGET: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// Mark that a selectable target (cell / row / column) claimed this mousedown, so the
/// grid-scroll background handler won't treat it as a click-to-deselect.
fn mark_pressed_target() {
    PRESSED_TARGET.with(|f| f.set(true));
}

/// Take the "a target was pressed" flag, resetting it. `false` ⇒ the press hit the empty
/// grid background and the selection should clear.
fn take_pressed_target() -> bool {
    PRESSED_TARGET.with(|f| f.replace(false))
}

/// A nested-cell view target (struct/list/map cell), opened from a grid cell and
/// shown in a `CellDialog`. Workspace-local to the grid.
#[derive(Clone, PartialEq)]
struct CellView {
    name: String,
    type_label: String,
    json: String,
}

#[component]
pub(crate) fn ResultsGrid(ws_id: WorkspaceId) -> Element {
    let state = use_context::<Signal<AppState>>();
    // The nested-cell view is grid-local, opened from a cell, closed by the dialog.
    let cell_view = use_signal(|| None::<CellView>);
    // The record (row-detail) view (Rz5) — a page-local *filtered* row index; `None` = closed.
    // Opened by double-clicking the row-number gutter; the dialog reads the run to render.
    let mut record_view = use_signal(|| None::<usize>);
    // Grid-scroll handle, so a click can focus it (WKWebView doesn't reliably focus a
    // tabindex div on click) → its onkeydown gets ⌘A / Esc.
    let mut grid_ref = use_signal(|| None::<Rc<MountedData>>);
    // True while a cell-selection drag is in progress (a cell `onmousedown` starts it,
    // mouse-up ends it). The drag-paint (`onmouseenter`) extends the rectangle only while
    // this holds, so a button held for a column-resize grip never paints cells.
    let mut drag_sel = use_signal(|| false);
    // Right-click copy menu anchor (Rz4); `None` = closed.
    let mut ctx_menu = use_signal(|| None::<Point>);

    let zebra = crate::settings::SETTINGS.resolve().read().zebra;
    // Default column width (V20) from settings; per-column overrides on the run win over it.
    let default_w = crate::settings::SETTINGS.resolve().read().default_col_width;
    let type_color = state.read().type_color_cells;
    // Rendered only alongside a result, so the `else` arms are defensive.
    let Some(entry) = crate::runs::RUNS.resolve().get(ws_id) else {
        return rsx! { super::results::Empty { ws_id } };
    };
    let run = entry.read();
    let Some(result) = run.result.clone() else {
        return rsx! { super::results::Empty { ws_id } };
    };
    let page = run.page;
    let page_size = run.page_size;
    let search = run.result_search.to_lowercase();
    let sel = run.sel.clone();
    let col_widths = run.col_widths.clone();
    let sort = run.sort;
    drop(run);

    // (name, type, type-text-class, cell-class, nested)
    let cols: Vec<(String, String, &'static str, &'static str, bool)> = result
        .columns
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.dtype.clone(),
                c.kind.text_class(),
                c.kind.cell_class(),
                c.kind.is_nested(),
            )
        })
        .collect();

    // `result.rows` is already the current page (in-app snapshot slice). Number by global
    // position; the find-box filters within the visible page (Rz3 selection is page-local).
    let base = page.saturating_sub(1) * page_size;
    let rows_page: Vec<(usize, Vec<Cell>)> = result
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search))
        })
        .map(|(i, r)| (base + i + 1, r.clone()))
        .collect();

    let ncols = cols.len();
    let nrows = rows_page.len();
    let last_row = nrows.saturating_sub(1);
    let last_col = ncols.saturating_sub(1);
    // Per-column widths (V20): the override for this result set, else the default. Header
    // and every body cell read the same value, so resizing keeps them aligned.
    let col_ws: Vec<f64> = (0..ncols)
        .map(|ci| col_widths.get(&ci).copied().unwrap_or(default_w))
        .collect();
    let has_sel = sel.is_some();
    // Precompute selection lookups for O(1) per-cell styling.
    let sel_bounds = sel.as_ref().and_then(|s| s.cell_bounds());
    let sel_rows: HashSet<usize> = match &sel {
        Some(Selection::Rows(v)) => v.iter().copied().collect(),
        _ => HashSet::new(),
    };
    let sel_cols: HashSet<usize> = match &sel {
        Some(Selection::Cols(v)) => v.iter().copied().collect(),
        _ => HashSet::new(),
    };

    rsx! {
        div {
            class: "grid-scroll ps-scroll",
            tabindex: "0",
            onmounted: move |e| grid_ref.set(Some(e.data())),
            // Focus on any press inside the grid so its onkeydown receives Esc, and so ⌘A —
            // routed via the Edit menu — knows to target cells (the grid holds Select All scope).
            onmousedown: move |e: MouseEvent| {
                if let Some(el) = grid_ref.peek().clone() {
                    spawn(async move { let _ = el.set_focus(true).await; });
                }
                // Bubbles *after* any cell/row/col press. Always consume the "a target claimed
                // it" flag; clear only on a *primary* press of the empty background (right-click
                // keeps the selection so the copy menu can act on it).
                let claimed = take_pressed_target();
                if e.trigger_button() == Some(MouseButton::Primary) && !claimed {
                    sel_clear(ws_id);
                    drag_sel.set(false);
                }
            },
            // Mouse-up anywhere over the grid ends a cell-selection drag.
            onmouseup: move |_| drag_sel.set(false),
            // Right-click → copy menu, but only when there's a selection to copy (Rz4).
            oncontextmenu: move |e: MouseEvent| {
                e.prevent_default();
                if has_sel {
                    let c = e.client_coordinates();
                    ctx_menu.set(Some(Point { x: c.x, y: c.y }));
                }
            },
            onfocusin: move |_| crate::menu::set_select_all_scope(crate::menu::SelectAllScope::Grid),
            // Clicking anywhere off the grid (it loses focus) clears the selection — so a
            // selection only ever exists while the grid is focused (Esc always reaches it).
            // Exception: opening the copy context menu pulls focus off the grid, but the menu
            // needs the selection to act on — so keep it while the menu is open.
            onfocusout: move |_| {
                crate::menu::set_select_all_scope(crate::menu::SelectAllScope::None);
                if ctx_menu.peek().is_none() {
                    sel_clear(ws_id);
                }
            },
            // Esc clears the selection (context-tier — stopped so it doesn't also hit the
            // global Cancel). ⌘A is owned by the Edit menu: it intercepts the accelerator at
            // the AppKit level before the webview, then routes to `select_all_active_grid`.
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape && has_sel {
                    e.stop_propagation();
                    sel_clear(ws_id);
                }
            },
            div { class: "grid-inner",
                div { class: "grid-head",
                    // The `#` corner selects the whole page (spreadsheet select-all). A
                    // `display:contents` wrapper carries the click so the Eyebrow renders
                    // unchanged; `mark_pressed_target` keeps the grid-background handler from
                    // treating this press as a click-to-deselect.
                    div {
                        style: "display:contents",
                        onmousedown: move |_| {
                            mark_pressed_target();
                            sel_all_cells(ws_id, nrows, ncols);
                        },
                        Eyebrow { class: "hnum", "#" }
                    }
                    for (ci, col) in cols.iter().cloned().enumerate() {
                        {render_hcol(
                            state, drag_sel, ws_id, ci, col, col_ws[ci],
                            sel_cols.contains(&ci),
                            sort.filter(|s| s.col == ci).map(|s| s.asc),
                        )}
                    }
                }
                for (i, (rownum, cells)) in rows_page.iter().cloned().enumerate() {
                    div { class: if zebra && rownum % 2 == 0 { "grid-row zebra" } else { "grid-row" },
                        div {
                            class: if sel_rows.contains(&i) { "rnum sel" } else { "rnum" },
                            title: "Double-click to view record",
                            onmousedown: move |e: MouseEvent| {
                                mark_pressed_target();
                                if e.trigger_button() != Some(MouseButton::Primary) {
                                    return;
                                }
                                let m = e.modifiers();
                                sel_row(ws_id, i, m.meta() || m.ctrl(), m.shift());
                            },
                            // Double-click the gutter → open the record (row-detail) view (Rz5).
                            ondoubleclick: move |_| record_view.set(Some(i)),
                            "{rownum}"
                        }
                        for (ci, cell) in cells.iter().enumerate() {
                            {render_cell(
                                ws_id, i, ci,
                                cols.get(ci).cloned(), cell.clone(),
                                cell_view, drag_sel, type_color,
                                col_ws.get(ci).copied().unwrap_or(default_w),
                                cell_sel_style(&sel_bounds, &sel_rows, &sel_cols, i, ci, last_row, last_col),
                            )}
                        }
                    }
                }
            }
        }
        if let Some(c) = cell_view() {
            CellDialog { view: c, cell_view }
        }
        // Record (row-detail) view (Rz5) — reads the run to render row `record_view`.
        if record_view().is_some() {
            RecordDialog { ws_id, idx: record_view }
        }
        // Right-click copy menu (Rz4). Operates on the current selection.
        if let Some(at) = ctx_menu() {
            ContextMenu { on_close: move |_| ctx_menu.set(None), at: Some(at), width: 200,
                {copy_menu_items(state, ctx_menu)}
            }
        }
    }
}

/// Rows for the results right-click copy menu (Rz4). Four peer "Copy as …" formats over the
/// same selection; TSV is also the ⌘C default (hence its keyboard hint).
fn copy_menu_items(state: Signal<AppState>, mut ctx_menu: Signal<Option<Point>>) -> Element {
    rsx! {
        MenuItem {
            label: "Copy as TSV".to_string(),
            meta: "⌘C".to_string(),
            onclick: move |_| { ctx_menu.set(None); dispatch(state, Action::CopySelection(TextFormat::Tsv)); },
        }
        MenuItem {
            label: "Copy as CSV".to_string(),
            onclick: move |_| { ctx_menu.set(None); dispatch(state, Action::CopySelection(TextFormat::Csv)); },
        }
        MenuItem {
            label: "Copy as JSON".to_string(),
            onclick: move |_| { ctx_menu.set(None); dispatch(state, Action::CopySelection(TextFormat::Json)); },
        }
        MenuItem {
            label: "Copy as Markdown".to_string(),
            onclick: move |_| { ctx_menu.set(None); dispatch(state, Action::CopySelection(TextFormat::Markdown)); },
        }
    }
}

// ── selection mutators (write `runs.sel` directly — ephemeral per-tab view state) ──

fn sel_cell_start(ws: WorkspaceId, r: usize, c: usize) {
    crate::runs::edit(ws, |run| {
        run.sel = Some(Selection::Cell {
            ar: r,
            ac: c,
            fr: r,
            fc: c,
        })
    });
}

/// Move the focus corner of a cell rectangle (drag / ⇧-click), starting one if absent.
fn sel_cell_to(ws: WorkspaceId, r: usize, c: usize) {
    crate::runs::edit(ws, |run| {
        let (ar, ac) = match run.sel {
            Some(Selection::Cell { ar, ac, .. }) => (ar, ac),
            _ => (r, c),
        };
        run.sel = Some(Selection::Cell {
            ar,
            ac,
            fr: r,
            fc: c,
        });
    });
}

/// Select row `i`, Excel-style. Plain click selects only it; `add` (⌘/Ctrl-click) toggles it
/// in/out of the multi-selection (this is how one row is removed from a group); `extend`
/// (shift-click) selects the contiguous range from the anchor to `i`.
fn sel_row(ws: WorkspaceId, i: usize, add: bool, extend: bool) {
    crate::runs::edit(ws, |run| {
        let is_rows = matches!(run.sel, Some(Selection::Rows(_)));
        // Shift-click → contiguous range from the anchor (only within an existing row
        // selection; otherwise it falls through to a plain select). The anchor is left where
        // it was so successive shift-clicks re-extend from the same point.
        if extend {
            if let (true, Some(a)) = (is_rows, run.sel_anchor) {
                let (lo, hi) = if a <= i { (a, i) } else { (i, a) };
                run.sel = Some(Selection::Rows((lo..=hi).collect()));
                return;
            }
        }
        if add {
            let mut rows = match &run.sel {
                Some(Selection::Rows(v)) => v.clone(),
                _ => Vec::new(),
            };
            match rows.iter().position(|&x| x == i) {
                Some(p) => {
                    rows.remove(p);
                }
                None => rows.push(i),
            }
            run.sel = (!rows.is_empty()).then_some(Selection::Rows(rows));
        } else {
            run.sel = Some(Selection::Rows(vec![i]));
        }
        run.sel_anchor = Some(i);
    });
}

/// Select column `ci`, Excel-style. Plain click selects only it; `add` (⌘/Ctrl-click) toggles
/// it in/out of the multi-selection; `extend` (shift-click) selects the contiguous range
/// from the anchor to `ci`.
fn sel_col(ws: WorkspaceId, ci: usize, add: bool, extend: bool) {
    crate::runs::edit(ws, |run| {
        let is_cols = matches!(run.sel, Some(Selection::Cols(_)));
        if extend {
            if let (true, Some(a)) = (is_cols, run.sel_anchor) {
                let (lo, hi) = if a <= ci { (a, ci) } else { (ci, a) };
                run.sel = Some(Selection::Cols((lo..=hi).collect()));
                return;
            }
        }
        if add {
            let mut cols = match &run.sel {
                Some(Selection::Cols(v)) => v.clone(),
                _ => Vec::new(),
            };
            match cols.iter().position(|&x| x == ci) {
                Some(p) => {
                    cols.remove(p);
                }
                None => cols.push(ci),
            }
            run.sel = (!cols.is_empty()).then_some(Selection::Cols(cols));
        } else {
            run.sel = Some(Selection::Cols(vec![ci]));
        }
        run.sel_anchor = Some(ci);
    });
}

/// Select every cell on the active tab's current result page. The Edit menu routes ⌘A
/// here when the grid holds the Select All scope; dims are recomputed from the run (the
/// menu handler has no component scope) to match the grid's page-local filtering.
pub(crate) fn select_all_active_grid() {
    let ws = crate::session::active_id();
    if ws == 0 {
        return;
    }
    crate::runs::edit_existing(ws, |run| {
        let search = run.result_search.to_lowercase();
        let dims = run.result.as_ref().map(|result| {
            let nrows = result
                .rows
                .iter()
                .filter(|r| {
                    search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search))
                })
                .count();
            (nrows, result.columns.len())
        });
        if let Some((nrows, ncols)) = dims {
            if nrows > 0 && ncols > 0 {
                run.sel = Some(Selection::Cell {
                    ar: 0,
                    ac: 0,
                    fr: nrows - 1,
                    fc: ncols - 1,
                });
            }
        }
    });
}

fn sel_clear(ws: WorkspaceId) {
    crate::runs::edit(ws, |run| {
        run.sel = None;
        run.sel_anchor = None;
    });
}

/// Select every cell on the page (the `#` corner header, spreadsheet select-all). Uses the
/// grid's already-computed `nrows`/`ncols` so it matches exactly what's rendered. Deselect
/// is via clicking off / Esc — matching common spreadsheets.
fn sel_all_cells(ws: WorkspaceId, nrows: usize, ncols: usize) {
    if nrows > 0 && ncols > 0 {
        crate::runs::edit(ws, |run| {
            run.sel = Some(Selection::Cell {
                ar: 0,
                ac: 0,
                fr: nrows - 1,
                fc: ncols - 1,
            });
        });
    }
}

// ── column resize (V20) ──

/// Auto-fit column `ci` to the widest value on the current page (grip double-click):
/// `width = clamp(maxLen*7.6 + 28, 64, 520)`, where `maxLen` is the longest of the header
/// name (+3 for the affordance) and each page row's cell text. A char-count estimate — no
/// off-thread text metrics available — matching the V20 prototype.
fn col_autofit(ws: WorkspaceId, ci: usize) {
    crate::runs::edit(ws, |run| {
        let w = run.result.as_ref().and_then(|result| {
            let col = result.columns.get(ci)?;
            let mut max_len = col.name.chars().count() + 3;
            for row in &result.rows {
                if let Some(cell) = row.get(ci) {
                    max_len = max_len.max(cell.text.chars().count());
                }
            }
            Some((max_len as f64 * 7.6 + 28.0).clamp(64.0, 520.0))
        });
        if let Some(w) = w {
            run.col_widths.insert(ci, w);
        }
    });
}

// ── selection rendering ──

/// Accent inset box-shadow *value* for whichever outer edges of the selection region this
/// cell sits on (a 2px border around the rectangle; full ring for a single focused cell).
/// Returns `none` when off every edge, so the property is always set explicitly.
fn edge_shadow(top: bool, bot: bool, left: bool, right: bool, focus: bool) -> String {
    let mut sh: Vec<&str> = Vec::new();
    if top {
        sh.push("inset 0 2px 0 var(--accent)");
    }
    if bot {
        sh.push("inset 0 -2px 0 var(--accent)");
    }
    if left {
        sh.push("inset 2px 0 0 var(--accent)");
    }
    if right {
        sh.push("inset -2px 0 0 var(--accent)");
    }
    if focus {
        sh.push("inset 0 0 0 2px var(--accent)");
    }
    if sh.is_empty() {
        "none".to_string()
    } else {
        sh.join(",")
    }
}

/// The inline selection style for cell `(i, ci)`. **Always** emits `background` +
/// `box-shadow` (transparent / none when unselected): Dioxus's per-property style diffing
/// doesn't clear a property that merely vanishes from the string, so a lingering edge
/// highlight would otherwise stay after deselect.
fn cell_sel_style(
    bounds: &Option<(usize, usize, usize, usize)>,
    rows: &HashSet<usize>,
    cols: &HashSet<usize>,
    i: usize,
    ci: usize,
    last_row: usize,
    last_col: usize,
) -> String {
    // `checked_sub(1).unwrap_or(MAX)` → the neighbour "before index 0" is never in a set, so
    // the outer edge lands correctly at index 0.
    let (selected, shadow) = if !cols.is_empty() {
        if cols.contains(&ci) {
            (
                true,
                edge_shadow(
                    i == 0,
                    i == last_row,
                    !cols.contains(&ci.checked_sub(1).unwrap_or(usize::MAX)),
                    !cols.contains(&(ci + 1)),
                    false,
                ),
            )
        } else {
            (false, "none".to_string())
        }
    } else if !rows.is_empty() {
        if rows.contains(&i) {
            (
                true,
                edge_shadow(
                    !rows.contains(&i.checked_sub(1).unwrap_or(usize::MAX)),
                    !rows.contains(&(i + 1)),
                    ci == 0,
                    ci == last_col,
                    false,
                ),
            )
        } else {
            (false, "none".to_string())
        }
    } else if let Some((minr, maxr, minc, maxc)) = *bounds {
        if i >= minr && i <= maxr && ci >= minc && ci <= maxc {
            let single = minr == maxr && minc == maxc;
            (
                true,
                edge_shadow(i == minr, i == maxr, ci == minc, ci == maxc, single),
            )
        } else {
            (false, "none".to_string())
        }
    } else {
        (false, "none".to_string())
    };
    let bg = if selected { "var(--c-sel)" } else { "transparent" };
    format!("background:{bg};box-shadow:{shadow};")
}

/// A column header — click selects the whole column (⌘/Ctrl toggles one, ⇧ a range). Carries
/// the V20 resize grip and the Rz6 sort chevron. `sort_dir`: `Some(true)` = this column sorts
/// ascending, `Some(false)` = descending, `None` = unsorted.
#[allow(clippy::too_many_arguments)]
fn render_hcol(
    state: Signal<AppState>,
    mut drag_sel: Signal<bool>,
    ws: WorkspaceId,
    ci: usize,
    col: (String, String, &'static str, &'static str, bool),
    w: f64,
    selected: bool,
    sort_dir: Option<bool>,
) -> Element {
    let (cn, ct, tcls, _cc, _nested) = col;
    // Always emit `background` (see `cell_sel_style`) so the tint clears on deselect.
    let bg = if selected { "var(--c-sel)" } else { "transparent" };
    let cn_cls = if selected { "cn sel" } else { "cn" };
    // This column's grip stays lit while *it* is the one being resized (not just on hover).
    let grip_active = matches!(
        state.read().resizing,
        Some(Resizing { target: ResizeTarget::Column { ws: rws, ci: rci }, .. }) if rws == ws && rci == ci
    );
    let grip_cls = if grip_active { "col-grip resizing" } else { "col-grip" };
    // Sort chevron: up = asc, down = desc / unsorted (unsorted is faint, revealed on hover).
    let sort_icon = if sort_dir == Some(true) {
        IconName::ChevronUp
    } else {
        IconName::ChevronDown
    };
    let sort_cls = if sort_dir.is_some() {
        "col-sort active"
    } else {
        "col-sort"
    };
    rsx! {
        div {
            class: "hcol",
            style: "width:{w}px;background:{bg};",
            onmousedown: move |e: MouseEvent| {
                mark_pressed_target();
                if e.trigger_button() != Some(MouseButton::Primary) {
                    return;
                }
                let m = e.modifiers();
                sel_col(ws, ci, m.meta() || m.ctrl(), m.shift());
            },
            div { class: "hcol-top",
                MonoValue { class: "{cn_cls}", "{cn}" }
                // Sort toggle (Rz6, asc→desc→clear). `stop_propagation` so grabbing it never
                // selects the column; the click re-fetches page 1 sorted over the snapshot.
                button {
                    class: "{sort_cls}",
                    title: "Sort by this column",
                    onmousedown: move |e: MouseEvent| e.stop_propagation(),
                    onclick: move |_| dispatch(state, Action::SortColumn(ci)),
                    {sort_icon.el(IconSize::Sm)}
                }
            }
            Meta { class: "ct {tcls}", "{ct}" }
            // V20 drag-to-resize grip on the right edge. `stop_propagation` so a grab never
            // triggers column-select (or sort). Drag → StartResize with this column's current
            // width as `start`; the root's move/up handlers drive it. Double-click → auto-fit.
            div {
                class: "{grip_cls}",
                title: "Drag to resize · double-click to auto-fit",
                onmousedown: move |e: MouseEvent| {
                    if e.trigger_button() != Some(MouseButton::Primary) {
                        return;
                    }
                    e.prevent_default();
                    e.stop_propagation();
                    // A grip grab is never a cell-selection drag — cancel any in-progress one.
                    drag_sel.set(false);
                    let origin = e.client_coordinates().x;
                    dispatch(state, Action::StartResize {
                        target: ResizeTarget::Column { ws, ci },
                        origin,
                        start: w,
                    });
                },
                ondoubleclick: move |e: MouseEvent| {
                    e.stop_propagation();
                    col_autofit(ws, ci);
                },
            }
        }
    }
}

/// One grid cell. A plain fn (called once per cell — thousands per page) so it stays a
/// lightweight `Element`. Mousedown starts/extends the selection (⇧ extends, drag paints);
/// double-click opens the nested-cell view for struct/list/map cells.
#[allow(clippy::too_many_arguments)]
fn render_cell(
    ws: WorkspaceId,
    i: usize,
    ci: usize,
    col: Option<(String, String, &'static str, &'static str, bool)>,
    cell: Cell,
    mut cell_view: Signal<Option<CellView>>,
    mut drag_sel: Signal<bool>,
    type_color: bool,
    w: f64,
    sel_style: String,
) -> Element {
    let (name, ty, cell_cls, nested) = match col {
        Some((n, t, _tc, cc, nested)) => (n, t, cc, nested),
        None => (String::new(), String::new(), "", false),
    };
    let mut class = String::from("cell");
    if cell.null {
        class.push_str(" null");
    } else if type_color && !cell_cls.is_empty() {
        class.push(' ');
        class.push_str(cell_cls);
    }
    let text = cell.text.clone();

    rsx! {
        div {
            class: "{class}",
            style: "width:{w}px;{sel_style}",
            // No `prevent_default` — it would block the grid-scroll from taking focus (so
            // ⌘A/Esc wouldn't fire). Text-selection is suppressed via `user-select:none`.
            onmousedown: move |e: MouseEvent| {
                mark_pressed_target();
                // Right/middle-click keeps the current selection (for the copy menu); only
                // primary starts/moves a cell selection.
                if e.trigger_button() != Some(MouseButton::Primary) {
                    return;
                }
                drag_sel.set(true);
                if e.modifiers().shift() {
                    sel_cell_to(ws, i, ci);
                } else {
                    sel_cell_start(ws, i, ci);
                }
            },
            // Extend the rectangle while a cell-selection drag is in progress. `drag_sel`
            // (set only by a cell mousedown) means a button held for something else — e.g.
            // dragging a column-resize grip across the grid — never paints. `held_buttons`
            // stays as a self-correcting backstop so a mouse-up we miss can't stick the drag.
            onmouseenter: move |e: MouseEvent| {
                if drag_sel() && e.held_buttons().contains(MouseButton::Primary) {
                    sel_cell_to(ws, i, ci);
                }
            },
            ondoubleclick: move |_| {
                if nested {
                    cell_view.set(Some(CellView {
                        name: name.clone(),
                        type_label: ty.clone(),
                        json: text.clone(),
                    }));
                }
            },
            Readout { style: "display:inline;", "{cell.text}" }
        }
    }
}

/// The nested-cell JSON view (struct/list/map cell) — a workspace-local `Dialog`
/// with a static highlighted `Code` body. The `cell_view` signal owns open/close.
#[component]
pub(crate) fn CellDialog(cell_view: Signal<Option<CellView>>, view: CellView) -> Element {
    let mut cell_view = cell_view;
    rsx! {
        Dialog { on_close: move |_| cell_view.set(None), card_class: "modal cell-modal".to_string(), z: 64,
            div { class: "row", style: "gap:var(--sp-4);padding:var(--sp-4) var(--sp-5);border-bottom:1px solid var(--line);",
                MonoValue { style: "color:var(--text);", "{view.name}" }
                Badge { variant: BadgeVariant::Accent, "{view.type_label}" }
                Spacer {}
                IconButton { icon: IconName::Close, variant: IconButtonVariant::Ghost, title: "Close", onclick: move |_| cell_view.set(None), }
            }
            div { style: "overflow:auto;max-height:70vh;",
                Code {
                    src: SourceCode::new(crate::ui::lang("json"), view.json.clone()),
                    theme: crate::ui::code_theme(),
                }
            }
        }
    }
}

/// The record (row-detail) view (Rz5) — a workspace-local modal showing one row as a **key → value**
/// card, with page-local prev/next navigation and a `⋯` menu to copy the record in any format. It
/// reads the run directly (result + filter), rebuilding the same filtered page the grid shows, so
/// `idx` (a page-local filtered row index) matches the double-clicked gutter row without prop clones.
#[component]
fn RecordDialog(ws_id: WorkspaceId, idx: Signal<Option<usize>>) -> Element {
    let state = use_context::<Signal<AppState>>();
    let mut idx = idx;

    let Some(entry) = crate::runs::RUNS.resolve().get(ws_id) else {
        return rsx! {};
    };
    let run = entry.read();
    let Some(result) = run.result.clone() else {
        return rsx! {};
    };
    let page_batch = run.page_batch.clone();
    let search = run.result_search.to_lowercase();
    let base = run.page.saturating_sub(1) * run.page_size;
    drop(run);

    let type_color = state.read().type_color_cells;
    let total = result.total;
    // (name, arrow dtype, type-text class, value cell class, nested?). The key shows the name over
    // its type (type-coloured); values are coloured like the grid, nested ones shown as a block.
    let cols: Vec<(String, String, &'static str, &'static str, bool)> = result
        .columns
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.dtype.clone(),
                c.kind.text_class(),
                c.kind.cell_class(),
                c.kind.is_nested(),
            )
        })
        .collect();
    // Same filtered page the grid renders (find-box filter is page-local), numbered globally.
    let rows: Vec<(usize, Vec<Cell>)> = result
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search)))
        .map(|(i, r)| (base + i + 1, r.clone()))
        .collect();

    let n = rows.len();
    if n == 0 {
        // The filter emptied the page out from under us — nothing to render (don't mutate the
        // signal mid-render); the record reappears if the rows come back.
        return rsx! {};
    }
    let cur = idx().unwrap_or(0).min(n - 1);
    let (rownum, cells) = rows[cur].clone();
    // Page-batch row for this record = its original (unfiltered) page index = rownum - base - 1.
    let batch_row = rownum.saturating_sub(base + 1);

    rsx! {
        Dialog { on_close: move |_| idx.set(None), card_class: "modal record-modal".to_string(), z: 64,
            div { class: "record-head",
                MonoValue { class: "record-title", "Row {rownum} of {total}" }
                Spacer {}
                IconButton {
                    icon: IconName::ChevronUp, variant: IconButtonVariant::Ghost, title: "Previous row",
                    disabled: cur == 0,
                    onclick: move |_| if cur > 0 { idx.set(Some(cur - 1)); },
                }
                IconButton {
                    icon: IconName::ChevronDown, variant: IconButtonVariant::Ghost, title: "Next row",
                    disabled: cur + 1 >= n,
                    onclick: move |_| if cur + 1 < n { idx.set(Some(cur + 1)); },
                }
                DropdownMenu {
                    class: "icon-btn plain", style: "width:28px;height:28px;", title: "Copy record",
                    align: RectAlign::BOTTOM_END, width: 190,
                    trigger: rsx! { Icon { name: IconName::Dots, size: IconSize::Sm } },
                    {record_copy_items(state, cur)}
                }
                IconButton {
                    icon: IconName::Close, variant: IconButtonVariant::Ghost, title: "Close",
                    onclick: move |_| idx.set(None),
                }
            }
            div { class: "record-body ps-scroll",
                for (ci, (name, dtype, tclass, cclass, nested)) in cols.iter().cloned().enumerate() {
                    div { class: "record-row",
                        div { class: "record-key",
                            MonoValue { class: "record-name", "{name}" }
                            Meta { class: if type_color { format!("record-type {tclass}") } else { "record-type".to_string() }, "{dtype}" }
                        }
                        {
                            match cells.get(ci) {
                                Some(c) if c.null => rsx! { Meta { class: "record-val null", "NULL" } },
                                // Nested (struct/list/map) → pretty JSON of the value (arrow-json +
                                // serde_json indent), in a recessed box. Falls back to the display
                                // text if the page batch isn't available.
                                Some(c) if nested => {
                                    let json = page_batch
                                        .as_ref()
                                        .and_then(|b| crate::serialize::cell_pretty_json(b, ci, batch_row))
                                        .unwrap_or_else(|| c.text.clone());
                                    rsx! {
                                        div { class: "record-val record-nested",
                                            Code {
                                                src: SourceCode::new(crate::ui::lang("json"), json),
                                                theme: crate::ui::code_theme(),
                                            }
                                        }
                                    }
                                },
                                Some(c) => rsx! {
                                    Readout {
                                        class: if type_color { format!("record-val {cclass}") } else { "record-val".to_string() },
                                        "{c.text}"
                                    }
                                },
                                None => rsx! { Readout { class: "record-val", "" } },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The four "Copy as …" rows for the record `⋯` menu — copies row `row` (all columns) in each
/// format via [`Action::CopyRecord`]. The `DropdownMenu` closes itself on any row click.
fn record_copy_items(state: Signal<AppState>, row: usize) -> Element {
    rsx! {
        MenuItem { label: "Copy as TSV".to_string(), onclick: move |_| dispatch(state, Action::CopyRecord(row, TextFormat::Tsv)) }
        MenuItem { label: "Copy as CSV".to_string(), onclick: move |_| dispatch(state, Action::CopyRecord(row, TextFormat::Csv)) }
        MenuItem { label: "Copy as JSON".to_string(), onclick: move |_| dispatch(state, Action::CopyRecord(row, TextFormat::Json)) }
        MenuItem { label: "Copy as Markdown".to_string(), onclick: move |_| dispatch(state, Action::CopyRecord(row, TextFormat::Markdown)) }
    }
}
