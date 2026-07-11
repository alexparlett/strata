//! The workspace tab strip: right-click `ContextMenu`, ⋯ overflow + ⌄ "show all tabs"
//! `DropdownMenu`s, and inline rename. All
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
use crate::ui::components::{
    Body, Caption, ContextMenu, Dot, DropdownMenu, Icon, IconButton, IconButtonVariant, MenuItem,
    MenuSep, Point, Prose, RectAlign, TextInput,
};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub(crate) fn Tabs() -> Element {
    let state = use_context::<Signal<AppState>>();
    let mut tab_menu = use_signal(|| None::<(WorkspaceId, Point)>);
    // S8 tab-bar controls: ⋯ overflow + ⌄ "show all tabs" — both `DropdownMenu`s. The tab
    // list uses a controlled `open` so its search box can dismiss the menu on Enter.
    let tab_list_open = use_signal(|| false);
    let mut tab_list_query = use_signal(String::new);
    // Inline-rename mode: which workspace (by id) is being renamed, and its draft.
    let mut renaming = use_signal(|| None::<WorkspaceId>);
    let mut rename_val = use_signal(String::new);

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
                            Dot { size: 6, color: if dirty { "var(--orange)" } else if id == active { "var(--accent)" } else { "var(--dim2)" } }
                            if is_rename {
                                input {
                                    class: "tab-rename",
                                    value: "{rv}",
                                    autofocus: true,
                                    spellcheck: false,
                                    onmounted: move |e| { spawn(async move { let _ = e.set_focus(true).await; }); },
                                    // Hold the Select All scope so ⌘A selects the rename text (this
                                    // bespoke input isn't a shared `TextInput`, so it opts in here).
                                    onfocusin: move |_| crate::menu::set_select_all_scope(crate::menu::SelectAllScope::Input),
                                    onfocusout: move |_| crate::menu::set_select_all_scope(crate::menu::SelectAllScope::None),
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
                                Body { "{name}" }
                                span { class: "close", onclick: move |e| { e.stop_propagation(); dispatch(state, Action::CloseTab(id)); }, "×" }
                            }
                        }
                    }
                }
            }
            }

            // Pinned right cluster (S8): new-tab · show-all-tabs · overflow.
            div { class: "ws-tab-cluster",
                IconButton { icon: IconName::Plus,
                    variant: IconButtonVariant::Ghost,
                    title: "New query",
                    onclick: move |_| dispatch(state, Action::NewTab),
                }
                DropdownMenu {
                    class: "icon-btn plain", style: "width:26px;height:28px;", title: "Show all tabs",
                    align: RectAlign::BOTTOM_END, width: 320, open: tab_list_open,
                    trigger: rsx! { Icon { name: IconName::ChevronDown, size: IconSize::Sm } },
                    {tab_list_body(state, tab_list_open, tab_list_query, active)}
                }
                DropdownMenu {
                    class: "icon-btn plain", style: "width:24px;height:28px;", title: "Tab actions",
                    align: RectAlign::BOTTOM_END,
                    trigger: rsx! { Icon { name: IconName::Dots, size: IconSize::Sm } },
                    {overflow_menu_items(state, active, !state.read().closed_tabs.is_empty())}
                }
            }

            // Self-contained tab context menu (right-click → ContextMenu).
            if let Some((id, at)) = tab_menu() {
                ContextMenu { on_close: move |_| tab_menu.set(None), at: Some(at),
                    {tab_menu_items(state, tab_menu, renaming, rename_val, id)}
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
fn overflow_menu_items(state: Signal<AppState>, active: WorkspaceId, can_reopen: bool) -> Element {
    rsx! {
        MenuItem { label: "Close all tabs".to_string(),
            onclick: move |_| dispatch(state, Action::CloseAllTabs) }
        MenuItem { label: "Close other tabs".to_string(),
            onclick: move |_| dispatch(state, Action::CloseOtherTabs(active)) }
        MenuSep {}
        MenuItem { label: "Reopen closed tab".to_string(), meta: "⇧⌘T".to_string(), disabled: !can_reopen,
            onclick: move |_| dispatch(state, Action::ReopenTab) }
    }
}

/// Body of the "show all tabs" searchable popover (S8): a filter box + a row per
/// workspace (click switches, × closes). Reads the live session so closing a row
/// updates the list in place; Enter opens the first match.
fn tab_list_body(
    state: Signal<AppState>,
    mut open: Signal<bool>,
    mut query: Signal<String>,
    active: WorkspaceId,
) -> Element {
    let q = query();
    let ql = q.to_lowercase();
    // Most-recently-viewed first, so the capped top-10 are the recent tabs.
    let mut wss = crate::session::snapshot().workspaces;
    wss.sort_by(|a, b| b.last_viewed.cmp(&a.last_viewed));
    let mut rows: Vec<(WorkspaceId, String, bool, bool)> = wss
        .iter()
        .filter(|w| ql.is_empty() || w.name.to_lowercase().contains(ql.as_str()))
        .map(|w| (w.id, w.name.clone(), w.id == active, w.is_dirty()))
        .collect();
    let first = rows.first().map(|r| r.0);
    // Show at most 10; beyond that the user narrows via the filter box.
    let overflow = rows.len().saturating_sub(10);
    rows.truncate(10);
    rsx! {
        // Clicks in the search row must NOT bubble to the DropdownMenu's close-wrapper.
        div { class: "tablist-search", onclick: move |e| e.stop_propagation(),
            span { class: "tablist-search-ic", Icon { name: IconName::Search, size: IconSize::Sm } }
            TextInput {
                bare: true,
                grow: true,
                autofocus: true,
                value: "{q}",
                placeholder: "Find a query tab…",
                oninput: move |v| query.set(v),
                onkeydown: move |e: KeyboardEvent| match e.key() {
                    Key::Enter => {
                        if let Some(id) = first {
                            e.prevent_default();
                            open.set(false);
                            dispatch(state, Action::SwitchTab(id));
                        }
                    }
                    Key::Escape => { e.prevent_default(); open.set(false); }
                    _ => {}
                },
            }
        }
        div { class: "tablist-rows",
            if rows.is_empty() {
                Prose { class: "tablist-empty", "No matching tabs" }
            }
            for (id, name, is_active, dirty) in rows {
                div {
                    class: if is_active { "tablist-row active" } else { "tablist-row" },
                    onclick: move |_| dispatch(state, Action::SwitchTab(id)),
                    Dot { size: 6, color: if dirty { "var(--orange)" } else if is_active { "var(--accent)" } else { "var(--dim2)" } }
                    Body { class: "tablist-name", "{name}" }
                    span {
                        class: "tablist-close",
                        onclick: move |e| { e.stop_propagation(); dispatch(state, Action::CloseTab(id)); },
                        "×"
                    }
                }
            }
        }
        if overflow > 0 {
            Caption { class: "tablist-more", "+{overflow} more — keep typing to filter" }
        }
    }
}
