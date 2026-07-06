//! The workspace tab strip and its self-contained context menu (a `Popup`). The
//! tab-menu open-state is a component-local signal, not on `AppState`.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::components::{MenuItem, MenuSep, Point, Popup};
use crate::ui::icons;

#[component]
pub(crate) fn Tabs() -> Element {
    let state = use_context::<Signal<AppState>>();
    // Self-contained: the tab context menu lives here, not in `AppState`.
    let mut tab_menu = use_signal(|| None::<(usize, Point)>);

    let sidebar_open = state.read().sidebar_open;
    let active = state.read().project.active_ws;
    let renaming = state.read().renaming_ws;
    let rename_val = state.read().rename_val.clone();
    let ws: Vec<(usize, String)> = state
        .read()
        .project.workspaces
        .iter()
        .enumerate()
        .map(|(i, w)| (i, w.name.clone()))
        .collect();

    rsx! {
        div {
            class: "ws-tabs",
            // Right-click on empty strip → menu for the active tab.
            oncontextmenu: move |e| {
                e.prevent_default();
                let c = e.client_coordinates();
                tab_menu.set(Some((active, Point { x: c.x, y: c.y })));
            },
            if !sidebar_open {
                button { class: "icon-btn plain", style: "width:28px;height:28px;margin-bottom:1px;",
                    title: "Show panel", onclick: move |_| dispatch(state, Action::ToggleSidebar),
                    {icons::expand_right(15)} }
            }
            div { class: "ws-tabs-scroll",
            for (i, name) in ws {
                {
                    let is_rename = renaming == Some(i);
                    let rv = rename_val.clone();
                    rsx! {
                        div {
                            class: if i == active { "ws-tab active" } else { "ws-tab" },
                            onclick: move |_| dispatch(state, Action::SwitchTab(i)),
                            ondoubleclick: move |e| { e.stop_propagation(); dispatch(state, Action::StartRename(i)); },
                            oncontextmenu: move |e| {
                                e.prevent_default();
                                e.stop_propagation();
                                let c = e.client_coordinates();
                                tab_menu.set(Some((i, Point { x: c.x, y: c.y })));
                            },
                            span { class: "tdot" }
                            if is_rename {
                                input {
                                    class: "tab-rename",
                                    value: "{rv}",
                                    autofocus: true,
                                    spellcheck: false,
                                    onmounted: move |e| { spawn(async move { let _ = e.set_focus(true).await; }); },
                                    oninput: move |e| dispatch(state, Action::RenameInput(e.value())),
                                    onkeydown: move |e| match e.key() {
                                        Key::Enter => { e.prevent_default(); dispatch(state, Action::CommitRename); }
                                        Key::Escape => { e.prevent_default(); dispatch(state, Action::CancelRename); }
                                        _ => {}
                                    },
                                    onblur: move |_| dispatch(state, Action::CommitRename),
                                    onclick: move |e| e.stop_propagation(),
                                }
                            } else {
                                span { "{name}" }
                                span { class: "close", onclick: move |e| { e.stop_propagation(); dispatch(state, Action::CloseTab(i)); }, "×" }
                            }
                        }
                    }
                }
            }
            button { class: "icon-btn plain", style: "width:28px;height:28px;margin-bottom:1px;flex:none;",
                title: "New query", onclick: move |_| dispatch(state, Action::NewTab), {icons::plus(15)} }
            }

            // Self-contained tab context menu (egui-style Popup container).
            if let Some((idx, at)) = tab_menu() {
                Popup { on_close: move |_| tab_menu.set(None), at,
                    {tab_menu_items(state, tab_menu, idx)}
                }
            }
        }
    }
}

/// Rows for a workspace-tab context menu. Each dismisses the popup then dispatches.
fn tab_menu_items(
    state: Signal<AppState>,
    mut tab_menu: Signal<Option<(usize, Point)>>,
    idx: usize,
) -> Element {
    let can_reopen = !state.read().closed_tabs.is_empty();
    rsx! {
        MenuItem { label: "Rename".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::StartRename(idx)); } }
        MenuSep {}
        MenuItem { label: "Close".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::CloseTab(idx)); } }
        MenuItem { label: "Close others".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::CloseOtherTabs(idx)); } }
        MenuItem { label: "Close to the right".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::CloseTabsRight(idx)); } }
        MenuItem { label: "Close all".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::CloseAllTabs); } }
        MenuSep {}
        MenuItem { label: "Reopen closed tab".to_string(), meta: "⇧⌘T".to_string(), disabled: !can_reopen,
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::ReopenTab); } }
    }
}
