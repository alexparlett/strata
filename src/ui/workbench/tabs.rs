//! The workspace tab strip and its self-contained context menu (a `Popup`). All
//! transient tab-strip UI — the context menu, plus the inline-rename target +
//! draft text — is component-local `use_signal` state, never on `AppState`; only
//! the durable rename commit (`Action::RenameTab`) goes through the action layer.
//!
//! Tabs are addressed by their stable `crate::session::WorkspaceId`; the strip is
//! built from the ordered `crate::session::snapshot()`.

use dioxus::prelude::*;
use dioxus_stores::*;

use crate::action::{dispatch, Action};
use crate::session::{SessionStoreExt, WorkspaceId, WorkspaceStoreExt};
use crate::state::AppState;
use crate::ui::components::{MenuItem, MenuSep, Point, Popup};
use crate::ui::icons;

#[component]
pub(crate) fn Tabs() -> Element {
    let state = use_context::<Signal<AppState>>();
    let mut tab_menu = use_signal(|| None::<(WorkspaceId, Point)>);
    // S8 tab-bar controls: the ⋯ overflow menu + the "show all tabs" searchable popover.
    let mut overflow_menu = use_signal(|| None::<Point>);
    let mut tab_list = use_signal(|| None::<Point>);
    let mut tab_list_query = use_signal(String::new);
    // Inline-rename mode: which workspace (by id) is being renamed, and its draft.
    let mut renaming = use_signal(|| None::<WorkspaceId>);
    let mut rename_val = use_signal(String::new);

    let sidebar_open = state.read().sidebar_open;
    // Read the active id + each entry through their lenses, so a `switch`
    // (`.active().set`) or a structural / per-field write re-renders the strip —
    // matching how `session` mutates the store.
    let sess = crate::session::store();
    let active = sess.active().cloned();
    let renaming_now = renaming();
    let rename_draft = rename_val();
    let mut ws: Vec<(WorkspaceId, String, bool)> = Vec::new();
    for w in sess.workspaces().iter() {
        ws.push((w.id().cloned(), w.name().cloned(), w.read().is_dirty()));
    }

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
            for (id, name, dirty) in ws {
                {
                    let is_rename = renaming_now == Some(id);
                    let rv = rename_draft.clone();
                    let name_seed = name.clone();
                    let tab_class = match (id == active, dirty) {
                        (true, true) => "ws-tab active dirty",
                        (true, false) => "ws-tab active",
                        (false, true) => "ws-tab dirty",
                        (false, false) => "ws-tab",
                    };
                    rsx! {
                        div {
                            class: "{tab_class}",
                            onclick: move |_| dispatch(state, Action::SwitchTab(id)),
                            ondoubleclick: move |e| {
                                e.stop_propagation();
                                rename_val.set(name_seed.clone());
                                renaming.set(Some(id));
                            },
                            oncontextmenu: move |e| {
                                e.prevent_default();
                                e.stop_propagation();
                                let c = e.client_coordinates();
                                tab_menu.set(Some((id, Point { x: c.x, y: c.y })));
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
                                        Key::Enter => { e.prevent_default(); commit_rename(state, renaming, rename_val, id); }
                                        Key::Escape => { e.prevent_default(); renaming.set(None); }
                                        _ => {}
                                    },
                                    onblur: move |_| commit_rename(state, renaming, rename_val, id),
                                    onclick: move |e| e.stop_propagation(),
                                }
                            } else {
                                span { "{name}" }
                                span { class: "close", onclick: move |e| { e.stop_propagation(); dispatch(state, Action::CloseTab(id)); }, "×" }
                            }
                        }
                    }
                }
            }
            }

            // Pinned right cluster (S8): new-tab · show-all-tabs · overflow.
            div { class: "ws-tab-cluster",
                button { class: "icon-btn plain", style: "width:28px;height:28px;",
                    title: "New query", onclick: move |_| dispatch(state, Action::NewTab),
                    {icons::plus(15)} }
                button { class: "icon-btn plain", style: "width:26px;height:28px;",
                    title: "Show all tabs",
                    onclick: move |e| {
                        let c = e.client_coordinates();
                        tab_list_query.set(String::new());
                        tab_list.set(Some(Point { x: (c.x - 290.0).max(8.0), y: c.y + 10.0 }));
                    },
                    {icons::chevron_down(14)} }
                button { class: "icon-btn plain", style: "width:24px;height:28px;",
                    title: "Tab actions",
                    onclick: move |e| {
                        let c = e.client_coordinates();
                        overflow_menu.set(Some(Point { x: (c.x - 180.0).max(8.0), y: c.y + 10.0 }));
                    },
                    {icons::dots(15)} }
            }

            // Self-contained tab context menu (egui-style Popup container).
            if let Some((id, at)) = tab_menu() {
                Popup { on_close: move |_| tab_menu.set(None), at,
                    {tab_menu_items(state, tab_menu, renaming, rename_val, id)}
                }
            }
            // ⋯ overflow menu — whole-strip actions (S8).
            if let Some(at) = overflow_menu() {
                Popup { on_close: move |_| overflow_menu.set(None), at,
                    {overflow_menu_items(state, overflow_menu, active, !state.read().closed_tabs.is_empty())}
                }
            }
            // "Show all tabs" searchable popover (S8).
            if let Some(at) = tab_list() {
                Popup { on_close: move |_| tab_list.set(None), at, card_class: "menu".to_string(), width: 320,
                    {tab_list_body(state, tab_list, tab_list_query, active)}
                }
            }
        }
    }
}

/// Commit the inline rename for workspace `id` (Enter / blur): dispatch the durable
/// rename, then leave rename mode. A no-op when not renaming, so the Enter that
/// already committed doesn't fire again on the follow-up blur.
fn commit_rename(
    state: Signal<AppState>,
    mut renaming: Signal<Option<WorkspaceId>>,
    rename_val: Signal<String>,
    id: WorkspaceId,
) {
    if renaming.peek().is_none() {
        return;
    }
    let v = rename_val.peek().clone();
    dispatch(state, Action::RenameTab(id, v));
    renaming.set(None);
}

/// Rows for a workspace-tab context menu. Each dismisses the popup then acts —
/// "Rename" seeds the component-local rename signals; the rest dispatch actions.
fn tab_menu_items(
    state: Signal<AppState>,
    mut tab_menu: Signal<Option<(WorkspaceId, Point)>>,
    mut renaming: Signal<Option<WorkspaceId>>,
    mut rename_val: Signal<String>,
    id: WorkspaceId,
) -> Element {
    let can_reopen = !state.read().closed_tabs.is_empty();
    rsx! {
        MenuItem { label: "Rename".to_string(),
            onclick: move |_| {
                tab_menu.set(None);
                let name = crate::session::snapshot()
                    .workspaces
                    .iter()
                    .find(|w| w.id == id)
                    .map(|w| w.name.clone())
                    .unwrap_or_default();
                rename_val.set(name);
                renaming.set(Some(id));
            } }
        MenuItem { label: "Duplicate".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::DuplicateTab(id)); } }
        MenuSep {}
        MenuItem { label: "Close".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::CloseTab(id)); } }
        MenuItem { label: "Close others".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::CloseOtherTabs(id)); } }
        MenuItem { label: "Close to the right".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::CloseTabsRight(id)); } }
        MenuItem { label: "Close all".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::CloseAllTabs); } }
        MenuSep {}
        MenuItem { label: "Reopen closed tab".to_string(), meta: "⇧⌘T".to_string(), disabled: !can_reopen,
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::ReopenTab); } }
    }
}

/// Rows for the ⋯ tab-overflow menu (S8): whole-strip actions (not tied to one tab).
fn overflow_menu_items(
    state: Signal<AppState>,
    mut overflow: Signal<Option<Point>>,
    active: WorkspaceId,
    can_reopen: bool,
) -> Element {
    rsx! {
        MenuItem { label: "Close all tabs".to_string(),
            onclick: move |_| { overflow.set(None); dispatch(state, Action::CloseAllTabs); } }
        MenuItem { label: "Close other tabs".to_string(),
            onclick: move |_| { overflow.set(None); dispatch(state, Action::CloseOtherTabs(active)); } }
        MenuSep {}
        MenuItem { label: "Reopen closed tab".to_string(), meta: "⇧⌘T".to_string(), disabled: !can_reopen,
            onclick: move |_| { overflow.set(None); dispatch(state, Action::ReopenTab); } }
    }
}

/// Body of the "show all tabs" searchable popover (S8): a filter box + a row per
/// workspace (click switches, × closes). Reads the live session so closing a row
/// updates the list in place; Enter opens the first match.
fn tab_list_body(
    state: Signal<AppState>,
    mut tab_list: Signal<Option<Point>>,
    mut query: Signal<String>,
    active: WorkspaceId,
) -> Element {
    let q = query();
    let ql = q.to_lowercase();
    let rows: Vec<(WorkspaceId, String, bool, bool)> = crate::session::snapshot()
        .workspaces
        .iter()
        .filter(|w| ql.is_empty() || w.name.to_lowercase().contains(ql.as_str()))
        .map(|w| (w.id, w.name.clone(), w.id == active, w.is_dirty()))
        .collect();
    let first = rows.first().map(|r| r.0);
    rsx! {
        div { class: "tablist-search",
            span { class: "tablist-search-ic", {icons::search(13)} }
            input {
                class: "tablist-input",
                value: "{q}",
                placeholder: "Find a query tab…",
                spellcheck: false,
                onmounted: move |e| { spawn(async move { let _ = e.set_focus(true).await; }); },
                oninput: move |e| query.set(e.value()),
                onkeydown: move |e| match e.key() {
                    Key::Enter => {
                        if let Some(id) = first {
                            e.prevent_default();
                            tab_list.set(None);
                            dispatch(state, Action::SwitchTab(id));
                        }
                    }
                    Key::Escape => { e.prevent_default(); tab_list.set(None); }
                    _ => {}
                },
            }
        }
        div { class: "tablist-rows",
            if rows.is_empty() {
                div { class: "tablist-empty", "No matching tabs" }
            }
            for (id, name, is_active, dirty) in rows {
                div {
                    class: if is_active { "tablist-row active" } else { "tablist-row" },
                    onclick: move |_| { tab_list.set(None); dispatch(state, Action::SwitchTab(id)); },
                    span { class: if dirty { "tdot dirty" } else { "tdot" } }
                    span { class: "tablist-name", "{name}" }
                    span {
                        class: "tablist-close",
                        onclick: move |e| { e.stop_propagation(); dispatch(state, Action::CloseTab(id)); },
                        "×"
                    }
                }
            }
        }
    }
}
