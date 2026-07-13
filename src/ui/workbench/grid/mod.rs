//! The results grid — type-coloured cells, zebra striping, find-filtering, Excel-style
//! **selection** (Rz3), and the nested-cell JSON view. Selection is page-local (the visible
//! rows), lives in `runs.sel`, and drives the status-bar aggregate. The grid owns the
//! nested-cell view signal locally and renders its own `CellDialog`.

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use std::collections::HashSet;
use std::rc::Rc;

use crate::action::{dispatch, Action};
use crate::engine::Cell;
use crate::runs::Selection;
use crate::serialize::TextFormat;
use crate::session::WorkspaceId;
use crate::state::AppState;
use crate::ui::components::{ContextMenu, Eyebrow, MenuItem, Point};

mod cells;
mod record;
mod selection;
use cells::{render_cell, render_hcol, CellDialog};
use record::RecordDialog;
pub(crate) use selection::select_all_active_grid;
use selection::{cell_sel_style, sel_all_cells, sel_clear, sel_row};

thread_local! {
    /// Set by a cell / row-number / column-header `onmousedown`; read (and reset) by the
    /// grid-scroll's own `onmousedown`, which bubbles *after* them. If nothing set it, the
    /// press landed on the empty grid background (e.g. below the last row, when the rows
    /// don't fill the viewport) → clear the selection.
    static PRESSED_TARGET: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// Mark that a selectable target (cell / row / column) claimed this mousedown, so the
/// grid-scroll background handler won't treat it as a click-to-deselect.
pub(super) fn mark_pressed_target() {
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
pub(super) struct CellView {
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

    let zebra = crate::settings::zebra();
    // Default column width (V20) from settings; per-column overrides on the run win over it.
    let default_w = crate::settings::default_col_width();
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

