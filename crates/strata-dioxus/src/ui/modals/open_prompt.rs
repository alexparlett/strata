//! Open-target prompt (B10) — an always-mounted host reading `overlays::open_prompt`.
//! When the open preference is "Ask" and a project is opened from a project window
//! (Open Project or Open Recent), this asks whether to open it in the current window
//! or a new one, with a "remember, don't ask again" checkbox that persists the pref.
//!
//! The host stays mounted but delegates the card to a child component that is only
//! mounted while a prompt is pending — so the `remember` checkbox is a fresh signal
//! on every open (reset to unchecked), even when the previous one closed via Cancel.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::ui::components::{Button, ButtonVariant, Checkbox, Dialog, Icon, Prose, Title};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub fn OpenPromptHost() -> Element {
    let Some(path) = crate::overlays::OVERLAYS
        .resolve()
        .read()
        .open_prompt
        .clone()
    else {
        return rsx! {};
    };
    rsx! { OpenPromptCard { path } }
}

#[component]
fn OpenPromptCard(path: std::path::PathBuf) -> Element {
    let mut remember = use_signal(|| false);
    // The picked project folder (the parent of its `.strata` dir).
    let name = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| path.display().to_string());

    rsx! {
        Dialog { on_close: move |_| crate::overlays::close_open_prompt(), card_class: "confirm".to_string(), z: 80,
            div { style: "padding:var(--sp-6) var(--sp-6) var(--sp-5);",
                div { style: "display:flex;align-items:center;gap:var(--sp-4);margin-bottom:var(--sp-4);",
                    span { style: "width:38px;height:38px;flex:none;border-radius:var(--r-2);background:var(--accent-soft);color:var(--accent);display:flex;align-items:center;justify-content:center;",
                        Icon { name: IconName::Folder, size: IconSize::Lg }
                    }
                    div { style: "min-width:0;",
                        Title { "Open Project" }
                        Prose { style: "color:var(--dim2);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;",
                            "{name}"
                        }
                    }
                }
                Prose { style: "display:block;line-height:1.5;color:var(--text3);",
                    "Open this project in the current window, or in a new window?"
                }
                div { style: "margin-top:var(--sp-5);",
                    Checkbox { checked: remember(), on_toggle: move |v| remember.set(v), "Remember, don't ask again" }
                }
            }
            div { style: "display:flex;align-items:center;justify-content:flex-end;gap:var(--sp-3);padding:var(--sp-4) var(--sp-6);background:var(--panel);border-top:1px solid var(--line);",
                Button { variant: ButtonVariant::Ghost, onclick: move |_| crate::overlays::close_open_prompt(), "Cancel" }
                Button {
                    variant: ButtonVariant::Secondary,
                    onclick: move |_| dispatch(Action::OpenChosen { new_window: true, remember: remember() }),
                    "New Window"
                }
                Button {
                    variant: ButtonVariant::Primary,
                    onclick: move |_| dispatch(Action::OpenChosen { new_window: false, remember: remember() }),
                    "This Window"
                }
            }
        }
    }
}
