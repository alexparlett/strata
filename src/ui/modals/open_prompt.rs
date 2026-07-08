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
use crate::state::AppState;
use crate::ui::components::{Checkbox, Dialog};
use crate::ui::icons;

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
    let state = use_context::<Signal<AppState>>();
    let mut remember = use_signal(|| false);
    // The picked project folder (the parent of its `.strata` dir).
    let name = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| path.display().to_string());

    rsx! {
        Dialog { on_close: move |_| crate::overlays::close_open_prompt(), card_class: "confirm".to_string(), z: 80,
            div { style: "padding:22px 24px 16px;",
                div { style: "display:flex;align-items:center;gap:12px;margin-bottom:14px;",
                    span { style: "width:38px;height:38px;flex:none;border-radius:9px;background:var(--accent-soft);color:var(--accent);display:flex;align-items:center;justify-content:center;",
                        {icons::folder(19)}
                    }
                    div { style: "min-width:0;",
                        div { style: "font:600 14.5px var(--ui);color:var(--text);", "Open Project" }
                        div { style: "font:400 12.5px var(--ui);color:var(--dim2);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;",
                            "{name}"
                        }
                    }
                }
                div { style: "font-size:13px;line-height:1.5;color:var(--text3);",
                    "Open this project in the current window, or in a new window?"
                }
                div { style: "margin-top:16px;",
                    Checkbox { checked: remember(), on_toggle: move |v| remember.set(v), "Remember, don't ask again" }
                }
            }
            div { style: "display:flex;align-items:center;justify-content:flex-end;gap:8px;padding:14px 20px;background:var(--panel);border-top:1px solid var(--line);",
                button {
                    style: "height:34px;padding:0 14px;border:none;background:transparent;color:var(--dim);border-radius:8px;cursor:pointer;font:600 12.5px var(--ui);",
                    onclick: move |_| crate::overlays::close_open_prompt(),
                    "Cancel"
                }
                button {
                    style: "height:34px;padding:0 16px;border:1px solid var(--line3);background:var(--elev);color:var(--text);border-radius:8px;cursor:pointer;font:600 12.5px var(--ui);",
                    onclick: move |_| dispatch(state, Action::OpenChosen { new_window: true, remember: remember() }),
                    "New Window"
                }
                button {
                    style: "height:34px;padding:0 16px;border:none;background:var(--accent);color:#fff;border-radius:8px;cursor:pointer;font:600 12.5px var(--ui);",
                    onclick: move |_| dispatch(state, Action::OpenChosen { new_window: false, remember: remember() }),
                    "This Window"
                }
            }
        }
    }
}
