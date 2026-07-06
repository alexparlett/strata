//! Right-click context menus (catalog rows + workspace tabs).
use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, CatalogKind, RemoveKind};
use crate::ui::icons;

/// Resolve a context-menu icon by name (kept local to the menu components).
fn menu_icon(name: &str) -> Element {
    match name {
        "play" => icons::play(14),
        "gear" => icons::gear(14),
        "pencil" => icons::pencil(14),
        "trash" => icons::trash(14),
        _ => rsx! {},
    }
}

// ---------------------------------------------------------------------------
// Catalog row context menu
// ---------------------------------------------------------------------------

#[component]
pub fn CatalogMenu() -> Element {
    let state = use_context::<Signal<AppState>>();
    let Some(cm) = state.read().ctx_menu.clone() else {
        return rsx! {};
    };
    // Each row maps to the concrete action it performs (no wrapper indirection).
    let name = cm.name.clone();
    // (action, label, icon, danger)
    let items: Vec<(Action, &'static str, &'static str, bool)> = match cm.kind {
        CatalogKind::Table => vec![
            (Action::LoadSelectStar(name.clone()), "View table", "play", false),
            (Action::OpenConfigEdit(name.clone()), "Configure", "gear", false),
            (
                Action::RequestRemove { kind: RemoveKind::Table, name: name.clone() },
                "Drop table",
                "trash",
                true,
            ),
        ],
        CatalogKind::View => vec![
            (Action::LoadSelectStar(name.clone()), "View view", "play", false),
            (Action::EditView(name.clone()), "Edit query", "pencil", false),
            (
                Action::RequestRemove { kind: RemoveKind::View, name: name.clone() },
                "Drop view",
                "trash",
                true,
            ),
        ],
        CatalogKind::Query => vec![
            (Action::OpenSavedQuery(name.clone()), "Open in new tab", "pencil", false),
            (Action::DeleteSavedQuery(name.clone()), "Delete query", "trash", true),
        ],
    };
    let (x, y) = (cm.x, cm.y);

    rsx! {
        div {
            class: "ctx-backdrop",
            onclick: move |_| dispatch(state, Action::CloseCatalogMenu),
            oncontextmenu: move |e| { e.prevent_default(); dispatch(state, Action::CloseCatalogMenu); },
            div {
                class: "ctx-menu",
                style: "left:{x}px;top:{y}px;",
                onclick: move |e| e.stop_propagation(),
                for (action, label, icon, danger) in items {
                    {
                        rsx! {
                            if danger { div { class: "ctx-sep" } }
                            div {
                                class: if danger { "ctx-item danger" } else { "ctx-item" },
                                onclick: move |_| {
                                    dispatch(state, Action::CloseCatalogMenu);
                                    dispatch(state, action.clone());
                                },
                                span { class: "ci", {menu_icon(icon)} }
                                span { "{label}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace tab context menu
// ---------------------------------------------------------------------------

#[component]
pub fn TabContextMenu() -> Element {
    let state = use_context::<Signal<AppState>>();
    let Some(tm) = state.read().tab_menu.clone() else {
        return rsx! {};
    };
    let can_reopen = !state.read().closed_tabs.is_empty();
    let idx = tm.idx;
    let (x, y) = (tm.x, tm.y);
    // (action, label, kbd, divider-before)
    let items: [(Action, &'static str, &'static str, bool); 6] = [
        (Action::StartRename(idx), "Rename", "", false),
        (Action::CloseTab(idx), "Close", "", true),
        (Action::CloseOtherTabs(idx), "Close others", "", false),
        (Action::CloseTabsRight(idx), "Close to the right", "", false),
        (Action::CloseAllTabs, "Close all", "", false),
        (Action::ReopenTab, "Reopen closed tab", "⇧⌘T", true),
    ];

    rsx! {
        div {
            class: "ctx-backdrop",
            onclick: move |_| dispatch(state, Action::CloseTabMenu),
            oncontextmenu: move |e| { e.prevent_default(); dispatch(state, Action::CloseTabMenu); },
            div {
                class: "ctx-menu tab",
                style: "left:{x}px;top:{y}px;",
                onclick: move |e| e.stop_propagation(),
                for (action, label, kbd, divider) in items {
                    {
                        let disabled = matches!(action, Action::ReopenTab) && !can_reopen;
                        rsx! {
                            if divider { div { class: "ctx-sep" } }
                            div {
                                class: if disabled { "ctx-item disabled" } else { "ctx-item" },
                                onclick: move |_| {
                                    if !disabled {
                                        dispatch(state, Action::CloseTabMenu);
                                        dispatch(state, action.clone());
                                    }
                                },
                                span { style: "flex:1;", "{label}" }
                                if !kbd.is_empty() { span { class: "kbd-hint", "{kbd}" } }
                            }
                        }
                    }
                }
            }
        }
    }
}

