//! The results grid — type-coloured cells, zebra striping, find-filtering — and
//! the nested-cell JSON view. The grid owns the cell-view signal locally and
//! renders its own `CellDialog`.

use dioxus::prelude::*;
use dioxus_code::{Code, SourceCode};

use crate::engine::Cell;
use crate::state::AppState;
use crate::ui::components::Dialog;
use crate::ui::icons;

/// A nested-cell view target (struct/list/map cell), opened from a grid cell and
/// shown in a `CellDialog`. Workspace-local to the grid.
#[derive(Clone, PartialEq)]
struct CellView {
    name: String,
    type_label: String,
    json: String,
}

#[component]
pub(crate) fn ResultsGrid() -> Element {
    let state = use_context::<Signal<AppState>>();
    // The nested-cell view is grid-local, opened from a cell, closed by the dialog.
    let cell_view = use_signal(|| None::<CellView>);

    let zebra = crate::settings::SETTINGS.read().zebra;
    let (type_color, id) = {
        let s = state.read();
        (s.type_color_cells, s.active_tab_id())
    };
    let runs = crate::runs::RUNS.read();
    // Rendered only alongside a result, so the `else` arms are defensive.
    let Some(run) = id.and_then(|id| runs.get(&id)) else {
        return rsx! { super::results::Empty {} };
    };
    let Some(result) = run.result.clone() else {
        return rsx! { super::results::Empty {} };
    };
    let page = run.page;
    let page_size = run.page_size;
    let search = run.result_search.to_lowercase();
    drop(runs);

    // (name, type, type-text-class, cell-class, nested)
    let cols: Vec<(String, String, &'static str, &'static str, bool)> = result
        .columns
        .iter()
        .map(|c| (c.name.clone(), c.dtype.clone(), c.kind.text_class(), c.kind.cell_class(), c.kind.is_nested()))
        .collect();

    // `result.rows` is already the current page (server-side snapshot). Number
    // by global position; the find-box filters within the visible page.
    let base = page.saturating_sub(1) * page_size;
    let rows_page: Vec<(usize, Vec<Cell>)> = result
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search)))
        .map(|(i, r)| (base + i + 1, r.clone()))
        .collect();

    rsx! {
        div { class: "grid-scroll ps-scroll",
            div { class: "grid-inner",
                div { class: "grid-head",
                    div { class: "hnum", "#" }
                    for (cn, ct, tcls, _cc, _nested) in cols.iter().cloned() {
                        div { class: "hcol", style: "width:150px;",
                            span { class: "cn", "{cn}" }
                            span { class: "ct {tcls}", "{ct}" }
                        }
                    }
                }
                for (rownum, cells) in rows_page {
                    div { class: if zebra && rownum % 2 == 0 { "grid-row zebra" } else { "grid-row" },
                        div { class: "rnum", "{rownum}" }
                        for (ci, cell) in cells.iter().enumerate() {
                            {render_cell(cols.get(ci).cloned(), cell.clone(), cell_view, type_color)}
                        }
                    }
                }
            }
        }
        if let Some(c) = cell_view() {
            CellDialog { view: c, cell_view }
        }
    }
}

/// One grid cell. A plain fn (called once per cell — thousands per page) so it
/// stays a lightweight `Element`, not a component scope. Opens the nested-cell
/// view for struct/list/map cells.
fn render_cell(
    col: Option<(String, String, &'static str, &'static str, bool)>,
    cell: Cell,
    mut cell_view: Signal<Option<CellView>>,
    type_color: bool,
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
            style: "width:150px;",
            onclick: move |_| {
                if nested {
                    cell_view.set(Some(CellView {
                        name: name.clone(),
                        type_label: ty.clone(),
                        json: text.clone(),
                    }));
                }
            },
            "{cell.text}"
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
            div { class: "row", style: "gap:10px;padding:13px 16px;border-bottom:1px solid var(--line);",
                span { class: "mono", style: "font-weight:600;font-size:13px;", "{view.name}" }
                span { class: "mono", style: "font-size:10px;color:var(--t-list);background:var(--accent-soft);padding:2px 7px;border-radius:5px;", "{view.type_label}" }
                div { class: "spacer" }
                button { class: "icon-btn plain", style: "width:28px;height:28px;", onclick: move |_| cell_view.set(None), {icons::close(13)} }
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
