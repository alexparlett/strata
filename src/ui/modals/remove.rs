//! Remove-confirmation dialog (drop table / view).
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, RemoveKind};
use crate::ui::icons;

// ---------------------------------------------------------------------------
// Remove confirmation
// ---------------------------------------------------------------------------

#[component]
pub fn RemoveConfirm() -> Element {
    let state = use_context::<Signal<AppState>>();
    let Some(target) = state.read().remove_target.clone() else {
        return rsx! {};
    };
    let (title, body, btn) = match target.kind {
        RemoveKind::Table => (
            "Drop table",
            "Removes the table from the catalog. Files on disk are not deleted.",
            "Drop table",
        ),
        RemoveKind::View => (
            "Drop view",
            "Drops the saved view. The tables it queries are unaffected.",
            "Drop view",
        ),
    };
    let name = target.name.clone();

    rsx! {
        div { class: "overlay", style: "z-index:78;", onclick: move |_| dispatch(state, Action::CancelRemove),
            div { class: "confirm", onclick: move |e| e.stop_propagation(),
                div { class: "confirm-head",
                    div { class: "confirm-ico", {icons::trash(20)} }
                    div { style: "flex:1;min-width:0;",
                        div { class: "confirm-title", "{title} " span { class: "nm", "{name}" } "?" }
                        div { class: "confirm-body", "{body}" }
                    }
                }
                div { class: "confirm-foot",
                    button { class: "btn-ghost", onclick: move |_| dispatch(state, Action::CancelRemove), "Cancel" }
                    button { class: "btn-danger", onclick: move |_| dispatch(state, Action::ConfirmRemove),
                        {icons::trash(14)}
                        "{btn}"
                    }
                }
            }
        }
    }
}

