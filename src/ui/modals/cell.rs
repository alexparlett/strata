//! Nested-cell JSON popover (static highlighted view).
use dioxus::prelude::*;
use dioxus_code::{Code, SourceCode};

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::icons;

// ---------------------------------------------------------------------------
// Nested cell popover (Code = static highlighted view)
// ---------------------------------------------------------------------------

#[component]
pub fn CellPopover() -> Element {
    let state = use_context::<Signal<AppState>>();
    let s = state.read();
    let name = s.cell.name.clone();
    let ty = s.cell.type_label.clone();
    let json = s.cell.json.clone();
    drop(s);

    rsx! {
        div { class: "overlay", style: "z-index:64;", onclick: move |_| dispatch(state, Action::CloseOverlays),
            div { class: "modal cell-modal", onclick: move |e| e.stop_propagation(),
                div { class: "row", style: "gap:10px;padding:13px 16px;border-bottom:1px solid var(--line);",
                    span { class: "mono", style: "font-weight:600;font-size:13px;", "{name}" }
                    span { class: "mono", style: "font-size:10px;color:var(--t-list);background:var(--accent-soft);padding:2px 7px;border-radius:5px;", "{ty}" }
                    div { class: "spacer" }
                    button { class: "icon-btn plain", style: "width:28px;height:28px;", onclick: move |_| dispatch(state, Action::CloseOverlays), {icons::close(13)} }
                }
                div { style: "overflow:auto;max-height:70vh;",
                    Code {
                        src: SourceCode::new(crate::ui::lang("json"), json.clone()),
                        theme: crate::ui::code_theme(),
                    }
                }
            }
        }
    }
}

