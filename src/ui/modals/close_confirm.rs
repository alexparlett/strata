//! Discard-on-close confirm (A6) — an always-mounted host reading
//! `overlays::close_confirm`. A tab with unsaved changes routes its close here;
//! Discard force-closes it, Cancel dismisses (leaving the tab so ⌘S can save it).

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::components::Dialog;
use crate::ui::icons;

#[component]
pub fn CloseConfirmHost() -> Element {
    let state = use_context::<Signal<AppState>>();
    let Some(id) = crate::overlays::OVERLAYS.read().close_confirm else {
        return rsx! {};
    };
    let name = crate::session::snapshot()
        .workspaces
        .iter()
        .find(|w| w.id == id)
        .map(|w| w.name.clone())
        .unwrap_or_default();

    rsx! {
        Dialog { on_close: move |_| crate::overlays::close_close_confirm(), card_class: "confirm".to_string(), z: 80,
            div { class: "confirm-head",
                div { class: "confirm-ico", {icons::trash(20)} }
                div { style: "flex:1;min-width:0;",
                    div { class: "confirm-title", "Discard changes to " span { class: "nm", "{name}" } "?" }
                    div { class: "confirm-body",
                        "This tab has unsaved edits. Cancel and press ⌘S to save it, or discard them."
                    }
                }
            }
            div { class: "confirm-foot",
                button { class: "btn-ghost", onclick: move |_| crate::overlays::close_close_confirm(), "Cancel" }
                button {
                    class: "btn-danger",
                    onclick: move |_| {
                        crate::overlays::close_close_confirm();
                        dispatch(state, Action::CloseTabForce(id));
                    },
                    {icons::trash(14)}
                    "Discard"
                }
            }
        }
    }
}
