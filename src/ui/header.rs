//! Top header: logo, project switcher, and the ⌘K search + ⌘, settings on the
//! right. The per-query toolbar (Run · Format · Clear · Save-as-view · Save-query)
//! now lives inside the editor pane (see `ui::workbench::editor`), bound to the
//! active workspace — the header keeps only project-scoped controls.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::components::{Backdrop, Point, Popup};
use crate::ui::icons;

#[component]
pub fn Header() -> Element {
    let state = use_context::<Signal<AppState>>();
    // The command palette + Settings are app-global overlays: the header buttons
    // toggle them through the per-window overlay store (⌘K / ⌘, do the same).
    // Self-contained: the project switcher dropdown lives here, not in `AppState`.
    let mut proj_menu = use_signal(|| false);
    let project = state.read().project.name.clone();

    rsx! {
        header {
            class: "ps-header",
            // The child webview covers the native title bar, so drag the window
            // from the header background (interactive children below stop
            // propagation). `prevent_default` stops the drag-selection.
            onmousedown: move |e| { e.prevent_default(); dioxus::desktop::window().drag(); },
            // Double-click the title bar to fill the screen / restore.
            ondoubleclick: move |_| dispatch(state, Action::ToggleWindowFill),

            div { class: "row", style: "gap:9px;",
                div { class: "ps-logo", {icons::strata_logo(22)} }
                span { class: "ps-wordmark", "Strata" }
            }

            div { class: "hsep" }

            button {
                class: "proj-btn",
                title: "Switch project",
                onmousedown: move |e| e.stop_propagation(),
                ondoubleclick: move |e| e.stop_propagation(),
                onclick: move |_| { let v = proj_menu(); proj_menu.set(!v); },
                {icons::folder(14)}
                span { class: "name", "{project}" }
                {icons::chevron_down(12)}
            }
            if proj_menu() {
                Backdrop { on_close: move |_| proj_menu.set(false),
                    Popup {
                        at: Point { x: 232.0, y: 46.0 },
                        card_class: "menu".to_string(),
                        width: 328,
                        {project_menu_body(state, proj_menu)}
                    }
                }
            }

            div { class: "spacer" }

            div { class: "row", style: "gap:8px;",
                button {
                    class: "icon-btn",
                    title: "Search (⌘K)",
                    onmousedown: move |e| e.stop_propagation(),
                    ondoubleclick: move |e| e.stop_propagation(),
                    onclick: move |_| crate::overlays::toggle_cmdk(),
                    {icons::search(15)}
                }
                button {
                    class: "icon-btn",
                    title: "Settings (⌘,)",
                    onmousedown: move |e| e.stop_propagation(),
                    ondoubleclick: move |e| e.stop_propagation(),
                    onclick: move |_| crate::overlays::toggle_settings(),
                    {icons::gear(15)}
                }
            }
        }
    }
}

/// The project switcher dropdown body (Open… + open/recent projects). Rendered as
/// a `Popup`'s children. "Open folder…" closes the menu then dispatches — the
/// async picker task is spawned on the Header scope, so it survives the unmount.
fn project_menu_body(state: Signal<AppState>, mut proj_menu: Signal<bool>) -> Element {
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
    let recents: Vec<(String, String)> = state
        .read()
        .recent_projects
        .iter()
        .filter(|r| Some(&r.path) != current.as_ref())
        .map(|r| (r.name.clone(), r.path.clone()))
        .collect();

    rsx! {
        div {
            class: "menu-item",
            onclick: move |_| { proj_menu.set(false); dispatch(state, Action::OpenProject); },
            {icons::folder(14)}
            span { "Open folder…" }
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
                            onclick: move |_| { proj_menu.set(false); dispatch(state, Action::OpenRecent(open_path.clone())); },
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
