//! Header project switcher dropdown (Open… + open/recent projects).
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::icons;

// ---------------------------------------------------------------------------
// Project menu
// ---------------------------------------------------------------------------

/// Two-letter initials for a project avatar (splits on `_ - space`).
fn initials_of(name: &str) -> String {
    let mut parts = name
        .split(|c: char| c == '_' || c == '-' || c == ' ')
        .filter(|s| !s.is_empty());
    let a = parts.next().and_then(|s| s.chars().next());
    let b = parts.next().and_then(|s| s.chars().next());
    match (a, b) {
        (Some(a), Some(b)) => format!("{}{}", a.to_ascii_uppercase(), b.to_ascii_uppercase()),
        (Some(a), None) => a.to_ascii_uppercase().to_string(),
        _ => "?".into(),
    }
}

#[component]
pub fn ProjectMenu() -> Element {
    let state = use_context::<Signal<AppState>>();
    let active = state.read().project.name.clone();
    let active_ini = initials_of(&active);
    let active_path = state
        .read()
        .project_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unsaved · in-memory".to_string());
    let current = state
        .read()
        .project_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    // Recents excluding the currently-open project.
    let recents: Vec<(String, String)> = state
        .read()
        .recent_projects
        .iter()
        .filter(|r| Some(&r.path) != current.as_ref())
        .map(|r| (r.name.clone(), r.path.clone()))
        .collect();

    rsx! {
        div {
            style: "position:fixed;inset:0;z-index:55;",
            onclick: move |_| dispatch(state, Action::ToggleProjectMenu),
            div { class: "menu", style: "position:absolute;top:46px;left:232px;width:328px;",
                onclick: move |e| e.stop_propagation(),
                div {
                    class: "menu-item",
                    // Opens the chosen folder in a *new* window. Don't pre-close the
                    // menu — the async picker runs on a task spawned from this
                    // component's scope, so it must stay mounted through the dialog;
                    // `open_dir` closes the menu once the picker resolves.
                    onclick: move |_| dispatch(state, Action::OpenProject),
                    {icons::folder(14)} span { "Open folder…" }
                }
                div { class: "menu-sep" }

                div { class: "sec-label", style: "padding:8px 10px 6px;", "OPEN PROJECT" }
                div { class: "proj-item on",
                    span { class: "avatar", style: "background:var(--accent);", "{active_ini}" }
                    div { class: "meta",
                        div { class: "nm", "{active}" }
                        div { class: "pth mono", "{active_path}" }
                    }
                }

                if !recents.is_empty() {
                    div { class: "menu-sep" }
                    div { class: "sec-label", style: "padding:8px 10px 6px;", "RECENT PROJECTS" }
                    for (name, path) in recents {
                        {
                            let ini = initials_of(&name);
                            let open_path = path.clone();
                            rsx! {
                                div {
                                    class: "proj-item",
                                    onclick: move |_| { dispatch(state, Action::ToggleProjectMenu); dispatch(state, Action::OpenRecent(open_path.clone())); },
                                    span { class: "avatar", style: "background:#7ee787;", "{ini}" }
                                    div { class: "meta",
                                        div { class: "nm", "{name}" }
                                        div { class: "pth mono", "{path}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

