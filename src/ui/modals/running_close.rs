//! Running-query close confirm (S14) — one dialog, two entry points: a tab whose
//! query has run past the threshold (`tab::close`) or the window with any query
//! running (`projects::close`). "Stop & close" cancels the query/queries and
//! closes; the secondary keeps it/them open; a "don't ask again" flips the setting.
//!
//! A cancelled query has nowhere to keep running, so there is deliberately no
//! "detach / keep running" option.
//!
//! Split Host/Card so the "don't ask again" checkbox resets each time the dialog
//! opens (the host is always mounted; the card mounts only while open).

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::overlays::RunningCloseTarget;
use crate::state::AppState;
use crate::ui::components::{Checkbox, Dialog};
use crate::ui::icons;

#[component]
pub fn RunningCloseHost() -> Element {
    let Some(target) = crate::overlays::OVERLAYS.resolve().read().close_running_confirm else {
        return rsx! {};
    };
    rsx! { RunningCloseCard { target } }
}

#[component]
fn RunningCloseCard(target: RunningCloseTarget) -> Element {
    let state = use_context::<Signal<AppState>>();
    let mut dont_ask = use_signal(|| false);

    let snap = crate::session::snapshot();
    let (title, body): (String, String) = match target {
        RunningCloseTarget::Tab(id) => {
            let name = snap
                .workspaces
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.name.clone())
                .unwrap_or_default();
            (
                format!("Stop the query in “{name}”?"),
                "Closing this tab will cancel the query still running in it — there's nowhere for it to keep running.".to_string(),
            )
        }
        RunningCloseTarget::Window => {
            let n = snap
                .workspaces
                .iter()
                .filter(|w| crate::runs::is_running(w.id))
                .count();
            (
                "Stop running queries?".to_string(),
                format!(
                    "{n} quer{} still running will be cancelled when the project closes.",
                    if n == 1 { "y" } else { "ies" }
                ),
            )
        }
    };
    let keep_label = match target {
        RunningCloseTarget::Tab(_) => "Keep tab open",
        RunningCloseTarget::Window => "Keep open",
    };

    rsx! {
        Dialog { on_close: move |_| crate::overlays::close_running_close(), card_class: "confirm".to_string(), z: 80,
            div { class: "confirm-head",
                div { class: "confirm-ico", {icons::stop(18)} }
                div { style: "flex:1;min-width:0;",
                    div { class: "confirm-title", "{title}" }
                    div { class: "confirm-body", "{body}" }
                }
            }
            div { style: "padding:2px 4px 6px;",
                Checkbox { checked: dont_ask(), on_toggle: move |v| dont_ask.set(v), "Don't ask again" }
            }
            div { class: "confirm-foot",
                button { class: "btn-ghost", onclick: move |_| crate::overlays::close_running_close(), "{keep_label}" }
                button {
                    class: "btn-danger",
                    onclick: move |_| {
                        if dont_ask() {
                            crate::settings::set_confirm_close_running(false);
                        }
                        crate::overlays::close_running_close();
                        match target {
                            RunningCloseTarget::Tab(id) => dispatch(state, Action::CloseTabForce(id)),
                            RunningCloseTarget::Window => dispatch(state, Action::CloseWindowForce),
                        }
                    },
                    {icons::stop(14)}
                    "Stop & close"
                }
            }
        }
    }
}
