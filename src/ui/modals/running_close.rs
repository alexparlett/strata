//! Running-query close confirm (S14) — one **mode-driven** dialog (v11), two entry
//! points: a tab whose query is **in flight** (`tab::close` — no threshold; a
//! finished query has `running == false`, so quick queries never prompt) or the
//! window / Close Project with any query running (`projects::close`). "Stop & close"
//! / "Stop & exit" cancels the query and closes; the secondary keeps it open; a
//! "don't ask again" flips the setting.
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

    // Mode-driven copy (v11): tab close vs project exit.
    let (title, name, body, is_project): (&str, String, &str, bool) = match target {
        RunningCloseTarget::Tab(id) => {
            let name = crate::session::snapshot()
                .workspaces
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.name.clone())
                .unwrap_or_default();
            (
                "Confirm close",
                name,
                "A query is running. Are you sure you want to stop it and close this tab?",
                false,
            )
        }
        RunningCloseTarget::Window => (
            "Confirm exit",
            state.read().project.name.clone(),
            "Queries are running. Are you sure you want to stop them and exit?",
            true,
        ),
    };
    let keep_label = if is_project { "Cancel" } else { "Keep tab open" };
    let stop_label = if is_project { "Stop & exit" } else { "Stop & close" };

    rsx! {
        Dialog { on_close: move |_| crate::overlays::close_running_close(), card_class: "confirm".to_string(), z: 80,
            div { class: "confirm-pad",
                div { class: "confirm-head-row",
                    div { class: "confirm-ico warn", {icons::warning(18)} }
                    div { style: "min-width:0;",
                        div { class: "confirm-title", "{title}" }
                        div { class: "confirm-sub", "{name}" }
                    }
                }
                div { class: "confirm-msg", "{body}" }
                div { class: "confirm-check",
                    Checkbox { checked: dont_ask(), on_toggle: move |v| dont_ask.set(v), "Don't ask again" }
                }
            }
            div { class: "confirm-foot split",
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
                    if is_project { {icons::logout(14)} } else { {icons::stop(14)} }
                    "{stop_label}"
                }
            }
        }
    }
}
