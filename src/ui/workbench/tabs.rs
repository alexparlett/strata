//! The workspace tab strip and its self-contained context menu (a `Popup`). All
//! transient tab-strip UI — the context menu, plus the inline-rename target +
//! draft text — is component-local `use_signal` state, never on `AppState`; only
//! the durable rename commit (`Action::RenameTab`) goes through the action layer.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::components::{MenuItem, MenuSep, Point, Popup};
use crate::ui::icons;

#[component]
pub(crate) fn Tabs() -> Element {
    let state = use_context::<Signal<AppState>>();
    let mut tab_menu = use_signal(|| None::<(usize, Point)>);
    // Inline-rename mode: which tab (by index) is being renamed, and its draft.
    let mut renaming = use_signal(|| None::<usize>);
    let mut rename_val = use_signal(String::new);

    let sidebar_open = state.read().sidebar_open;
    let active = state.read().project.active_ws;
    let renaming_now = renaming();
    let rename_draft = rename_val();
    let ws: Vec<(usize, String)> = state
        .read()
        .project
        .workspaces
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
                    let is_rename = renaming_now == Some(i);
                    let rv = rename_draft.clone();
                    let name_seed = name.clone();
                    rsx! {
                        div {
                            class: if i == active { "ws-tab active" } else { "ws-tab" },
                            onclick: move |_| dispatch(state, Action::SwitchTab(i)),
                            ondoubleclick: move |e| {
                                e.stop_propagation();
                                rename_val.set(name_seed.clone());
                                renaming.set(Some(i));
                            },
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
                                    oninput: move |e| rename_val.set(e.value()),
                                    onkeydown: move |e| match e.key() {
                                        Key::Enter => { e.prevent_default(); commit_rename(state, renaming, rename_val, i); }
                                        Key::Escape => { e.prevent_default(); renaming.set(None); }
                                        _ => {}
                                    },
                                    onblur: move |_| commit_rename(state, renaming, rename_val, i),
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
                    {tab_menu_items(state, tab_menu, renaming, rename_val, idx)}
                }
            }
        }
    }
}

/// Commit the inline rename for tab `idx` (Enter / blur): dispatch the durable
/// rename, then leave rename mode. A no-op when not renaming, so the Enter that
/// already committed doesn't fire again on the follow-up blur.
fn commit_rename(
    state: Signal<AppState>,
    mut renaming: Signal<Option<usize>>,
    rename_val: Signal<String>,
    idx: usize,
) {
    if renaming.peek().is_none() {
        return;
    }
    let v = rename_val.peek().clone();
    dispatch(state, Action::RenameTab(idx, v));
    renaming.set(None);
}

/// Rows for a workspace-tab context menu. Each dismisses the popup then acts —
/// "Rename" seeds the component-local rename signals; the rest dispatch actions.
fn tab_menu_items(
    state: Signal<AppState>,
    mut tab_menu: Signal<Option<(usize, Point)>>,
    mut renaming: Signal<Option<usize>>,
    mut rename_val: Signal<String>,
    idx: usize,
) -> Element {
    let can_reopen = !state.read().closed_tabs.is_empty();
    rsx! {
        MenuItem { label: "Rename".to_string(),
            onclick: move |_| {
                tab_menu.set(None);
                let name = state
                    .read()
                    .project
                    .workspaces
                    .get(idx)
                    .map(|w| w.name.clone())
                    .unwrap_or_default();
                rename_val.set(name);
                renaming.set(Some(idx));
            } }
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
