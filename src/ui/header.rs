//! Top header: logo, project switcher, and the ⌘K search + ⌘, settings on the
//! right. The per-query toolbar (Run · Format · Clear · Save-as-view · Save-query)
//! now lives inside the editor pane (see `ui::workbench::editor`), bound to the
//! active workspace — the header keeps only project-scoped controls.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::components::{
    Body, Control, DropdownMenu, Eyebrow, Icon, IconButton, IconButtonVariant, Path, Spacer, Title,
};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub fn Header() -> Element {
    let state = use_context::<Signal<AppState>>();
    // The command palette + Settings are app-global overlays: the header buttons
    // toggle them through the per-window overlay store (⌘K / ⌘, do the same).
    // Self-contained: the project switcher dropdown lives here, not in `AppState`.
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

            div { class: "row", style: "gap:var(--sp-3);",
                div { class: "ps-logo", Icon { name: IconName::StrataLogo, size: IconSize::Px(22) } }
                Title { class: "ps-wordmark", "Strata" }
            }

            div { class: "hsep" }

            DropdownMenu {
                class: "proj-btn",
                title: "Switch project",
                width: 328,
                trigger: rsx! {
                    Icon { name: IconName::Folder, size: IconSize::Sm }
                    Control { class: "name", "{project}" }
                    Icon { name: IconName::ChevronDown, size: IconSize::Xs }
                },
                {project_menu_body(state)}
            }

            Spacer {}

            // Drag-suppress once on the cluster (the child webview covers the native
            // title bar, so an un-stopped mousedown/dblclick here would drag / fill
            // the window). The buttons themselves are plain `IconButton`s.
            div { class: "row", style: "gap:var(--sp-3);",
                onmousedown: move |e| e.stop_propagation(),
                ondoubleclick: move |e| e.stop_propagation(),
                IconButton { icon: IconName::Search,
                    variant: IconButtonVariant::Toolbar,
                    title: "Search (⌘K)",
                    onclick: move |_| crate::overlays::toggle_cmdk(),
                }
                IconButton { icon: IconName::Gear,
                    variant: IconButtonVariant::Toolbar,
                    title: "Settings (⌘,)",
                    onclick: move |_| crate::window::spawn_settings_window(),
                }
            }
        }
    }
}

/// The project switcher dropdown body (Open… + open/recent projects) — the
/// `DropdownMenu`'s children. Closing is the `DropdownMenu`'s job (any inner click), so
/// items just dispatch; the async open-picker is spawned on the Header scope so it
/// survives the unmount.
fn project_menu_body(state: Signal<AppState>) -> Element {
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
            onclick: move |_| dispatch(state, Action::OpenProject),
            Icon { name: IconName::Folder, size: IconSize::Sm }
            Body { "Open folder…" }
        }
        div { class: "menu-sep" }

        Eyebrow { class: "sec-label", style: "padding:var(--sp-3) var(--sp-4) var(--sp-3);", "OPEN PROJECT" }
        div { class: "proj-item on",
            Control { class: "avatar", style: "background:var(--accent);", "{active_ini}" }
            div { class: "meta",
                Body { class: "nm", "{active}" }
                Path { class: "pth mono", "{active_path}" }
            }
        }

        if !recents.is_empty() {
            div { class: "menu-sep" }
            Eyebrow { class: "sec-label", style: "padding:var(--sp-3) var(--sp-4) var(--sp-3);", "RECENT PROJECTS" }
            for (name, path) in recents {
                {
                    let ini = initials_of(&name);
                    let open_path = path.clone();
                    rsx! {
                        div {
                            class: "proj-item",
                            onclick: move |_| dispatch(state, Action::OpenRecent(open_path.clone())),
                            Control { class: "avatar", style: "background:#7ee787;", "{ini}" }
                            div { class: "meta",
                                Body { class: "nm", "{name}" }
                                Path { class: "pth mono", "{path}" }
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
