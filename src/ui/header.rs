//! Top header: logo, project switcher, the query toolbar (Run · Format · Clear ·
//! Save-as-view · Save-query — moved up from the editor run-bar, S4), and the
//! ⌘K search + ⌘, settings on the right.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::icons;

/// A 30×30 header icon button that dispatches `action`. `onmousedown` is stopped
/// so clicking a button never starts a window drag.
fn tool_btn(state: Signal<AppState>, action: Action, title: &str, icon: Element) -> Element {
    rsx! {
        button {
            class: "icon-btn",
            title: "{title}",
            onmousedown: move |e| e.stop_propagation(),
            ondoubleclick: move |e| e.stop_propagation(),
            onclick: move |_| dispatch(state, action.clone()),
            {icon}
        }
    }
}

#[component]
pub fn Header() -> Element {
    let state = use_context::<Signal<AppState>>();
    let project = state.read().project.name.clone();
    let has_ws = !state.read().project.workspaces.is_empty();
    let running = state.read().running;

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
                onclick: move |_| dispatch(state, Action::ToggleProjectMenu),
                {icons::folder(14)}
                span { class: "name", "{project}" }
                {icons::chevron_down(12)}
            }

            div { class: "spacer" }

            // Query toolbar — only when a tab is open.
            if has_ws {
                div { class: "row", style: "gap:6px;",
                    button {
                        class: "btn accent",
                        style: "height:30px;",
                        title: "Run query (⌘/Ctrl+Enter)",
                        disabled: running,
                        onmousedown: move |e| e.stop_propagation(),
                        ondoubleclick: move |e| e.stop_propagation(),
                        onclick: move |_| dispatch(state, Action::RunQuery),
                        {icons::play(13)}
                        if running { "Running…" } else { "Run" }
                        span { class: "kbd", style: "background:rgba(7,16,25,.22);color:inherit;border:none;margin-left:2px;", "⌘↵" }
                    }
                    {tool_btn(state, Action::FormatSql, "Format SQL", icons::format(15))}
                    {tool_btn(state, Action::ClearSql, "Clear editor", icons::trash(15))}
                    {tool_btn(state, Action::SaveAsView, "Save as view", icons::eye(15))}
                    {tool_btn(state, Action::SaveQuery, "Save query (⌘S)", icons::save(15))}
                }
                div { class: "hsep" }
            }

            div { class: "row", style: "gap:8px;",
                {tool_btn(state, Action::ToggleCmdk, "Search (⌘K)", icons::search(15))}
                {tool_btn(state, Action::OpenSettings, "Settings (⌘,)", icons::gear(15))}
            }
        }
    }
}
