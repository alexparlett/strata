//! Discard-on-close confirm (A6) — an always-mounted host reading
//! `overlays::close_confirm`. A tab with unsaved changes routes its close here;
//! Discard force-closes it, Cancel dismisses (leaving the tab so ⌘S can save it).

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::components::{Button, ButtonVariant, Dialog, Icon, Readout, Title};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub fn CloseConfirmHost() -> Element {
    let state = use_context::<Signal<AppState>>();
    let Some(id) = crate::overlays::OVERLAYS.resolve().read().close_confirm else {
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
                div { class: "confirm-ico", Icon { name: IconName::Trash, size: IconSize::Px(20) } }
                div { style: "flex:1;min-width:0;",
                    Title { class: "confirm-title", "Discard changes to " span { class: "nm", "{name}" } "?" }
                    Readout { class: "confirm-body",
                        {format!("This tab has unsaved edits. Cancel and press {} to save it, or discard them.", crate::keymap::hint(crate::config::Command::SaveQuery))}
                    }
                }
            }
            div { class: "confirm-foot",
                Button { variant: ButtonVariant::Secondary, onclick: move |_| crate::overlays::close_close_confirm(), "Cancel" }
                Button {
                    variant: ButtonVariant::Danger,
                    icon: IconName::Trash, icon_size: IconSize::Sm,
                    onclick: move |_| {
                        crate::overlays::close_close_confirm();
                        dispatch(state, Action::CloseTabForce(id));
                    },
                    "Discard"
                }
            }
        }
    }
}
