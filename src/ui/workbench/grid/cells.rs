//! Grid cell + header rendering (`render_hcol`, `render_cell`) and the nested-cell JSON viewer
//! (`CellDialog`). Split out of the grid module; the selection helpers it calls live in the parent.

use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use dioxus_code::{Code, SourceCode};

use crate::action::{dispatch, Action};
use crate::engine::Cell;
use crate::session::WorkspaceId;
use crate::state::{AppState, ResizeTarget, Resizing};
use crate::ui::components::{
    Badge, BadgeVariant, Dialog, IconButton, IconButtonVariant, Meta, MonoValue, Readout, Spacer,
};
use crate::ui::icons::{IconName, IconSize};

use super::selection::{col_autofit, sel_cell_start, sel_cell_to, sel_col};
use super::{mark_pressed_target, CellView};

/// A column header — click selects the whole column (⌘/Ctrl toggles one, ⇧ a range). Carries
/// the V20 resize grip and the Rz6 sort chevron. `sort_dir`: `Some(true)` = this column sorts
/// ascending, `Some(false)` = descending, `None` = unsorted.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_hcol(
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
pub(super) fn render_cell(
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

