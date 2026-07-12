//! The workspace tab strip: right-click `ContextMenu`, ⋯ overflow + ⌄ "show all tabs"
//! `DropdownMenu`s, and inline rename. All
//! transient tab-strip UI — the context menu, plus the inline-rename target +
//! draft text — is component-local `use_signal` state, never on `AppState`; only
//! the durable rename commit (`Action::RenameTab`) goes through the action layer.
//!
//! Tabs are addressed by their stable `crate::session::WorkspaceId`; the strip is
//! built from the ordered `crate::session::snapshot()`.

use std::collections::HashMap;
use std::rc::Rc;

use dioxus::html::geometry::PixelsVector2D;
use dioxus::html::input_data::MouseButton;
use dioxus::html::ScrollBehavior;
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
    // The scrollable tab track — measured during a drag so dragging to the edge
    // auto-scrolls it (T1 edge-scroll).
    let mut scroll_ref = use_signal(|| None::<Rc<MountedData>>);
    // Per-tab widths (measured on mount) so a drag can hit-test each tab's midpoint.
    let mut widths = use_signal(|| HashMap::<WorkspaceId, f64>::new());

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
    // Live tab-drag state (T1). Reading it here re-renders the strip as the pointer
    // moves, so the floating ghost follows the cursor and the drop gap tracks the
    // insertion point. Only set once the drag has *started* (past the click threshold),
    // so a plain mousedown-select never disturbs the strip.
    let drag = state.read().tab_drag.clone();
    let dragging = drag.as_ref().map_or(false, |d| d.started);
    let drag_id = if dragging { drag.as_ref().map(|d| d.id) } else { None };
    // Drop index in *visible* (post-removal) order — a gap can open at any slot,
    // including the origin.
    let insert = if dragging { drag.as_ref().map(|d| d.insert) } else { None };
    let ghost_name = drag.as_ref().map(|d| d.name.clone()).unwrap_or_default();
    let ghost = if dragging { drag } else { None };

    // The tabs actually rendered. While a drag is live the dragged tab is lifted *out*
    // of the strip (JetBrains-style), and the rest are tagged with a visible index used
    // for gap placement + drop hit-testing.
    let mut render: Vec<(usize, usize, WorkspaceId, String, bool)> = Vec::new();
    let mut vis = 0usize;
    for (oi, (id, name, dirty)) in ws.iter().enumerate() {
        if drag_id == Some(*id) {
            continue;
        }
        render.push((oi, vis, *id, name.clone(), *dirty));
        vis += 1;
    }
    let visible_len = vis;

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
            onmounted: move |e| scroll_ref.set(Some(e.data())),
            for (oi, vi, id, name, dirty) in render.into_iter() {
                {
                    let is_rename = renaming_now == Some(id);
                    let rv = rename_draft.clone();
                    let name_seed = name.clone();
                    let md_name = name.clone();
                    let show_slot = insert == Some(vi);
                    let slot_name = ghost_name.clone();
                    let tab_class = match (id == active, dirty) {
                        (true, true) => "ws-tab active dirty",
                        (true, false) => "ws-tab active",
                        (false, true) => "ws-tab dirty",
                        (false, false) => "ws-tab",
                    };
                    rsx! {
                        // Drop gap: opens where the dragged tab will land (incl. the origin).
                        if show_slot {
                            div { class: "ws-tab-slot",
                                span { class: "ws-tab-slot-fill", "{slot_name}" }
                            }
                        }
                        div {
                            key: "{id}",
                            class: "{tab_class}",
                            // Measure the tab so drag hit-testing knows its midpoint.
                            onmounted: move |e| {
                                let data = e.data();
                                spawn(async move {
                                    if let Ok(r) = data.get_client_rect().await {
                                        widths.write().insert(id, r.size.width);
                                    }
                                });
                            },
                            // Mousedown selects the tab and arms a drag (the root pointer-driver
                            // promotes it to a real reorder past the threshold). No onclick, so a
                            // drag never doubles as a switch.
                            onmousedown: move |e| {
                                if is_rename { return; }
                                if e.trigger_button() != Some(MouseButton::Primary) { return; }
                                e.prevent_default();
                                let c = e.client_coordinates();
                                let el = e.element_coordinates();
                                if id != active { dispatch(state, Action::SwitchTab(id)); }
                                dispatch(state, Action::StartTabDrag {
                                    id,
                                    from: oi,
                                    name: md_name.clone(),
                                    off_x: el.x,
                                    off_y: el.y,
                                    x: c.x,
                                    y: c.y,
                                });
                                // Edge auto-scroll for this drag (T1): while the pointer sits
                                // near the track's left/right edge, keep scrolling the strip so
                                // off-screen tabs become reachable. Self-terminates on drop.
                                spawn(async move {
                                    loop {
                                        let Some(d) = state.peek().tab_drag.clone() else { break };
                                        let track = scroll_ref.peek().clone();
                                        if d.started {
                                            if let Some(track) = track {
                                                if let Ok(r) = track.get_client_rect().await {
                                                    let left = r.origin.x;
                                                    let right = r.origin.x + r.size.width;
                                                    let dir = if d.x < left + 52.0 {
                                                        -1.0
                                                    } else if d.x > right - 52.0 {
                                                        1.0
                                                    } else {
                                                        0.0
                                                    };
                                                    if dir != 0.0 {
                                                        if let Ok(cur) = track.get_scroll_offset().await {
                                                            let _ = track
                                                                .scroll(
                                                                    PixelsVector2D::new(cur.x + dir * 16.0, cur.y),
                                                                    ScrollBehavior::Instant,
                                                                )
                                                                .await;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        tokio::time::sleep(std::time::Duration::from_millis(16)).await;
                                    }
                                });
                            },
                            // While a drag is live, the pointer's half within this tab picks the
                            // drop slot in visible space: left half → before it, right half → after.
                            onmousemove: move |e| {
                                // Copy out of the signal first — holding a read guard across the
                                // `dispatch` (which writes state) would panic (already borrowed).
                                let d = state.peek().tab_drag.clone();
                                if let Some(d) = d {
                                    if d.started {
                                        let w = widths.peek().get(&id).copied().unwrap_or(140.0);
                                        let want = if e.element_coordinates().x < w / 2.0 { vi } else { vi + 1 };
                                        if d.insert != want {
                                            dispatch(state, Action::TabDragOver(want));
                                        }
                                    }
                                }
                            },
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
                            // Leading dot = active status only; the dirty marker lives in
                            // the trailing close slot (T4, per the canvas).
                            Dot { size: 6, color: if id == active { "var(--accent)" } else { "var(--dim2)" } }
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
                                // Close affordance (T4): clean tab shows ×; a dirty tab shows
                                // an unsaved dot that becomes × on hover (CSS-driven off `.dirty`).
                                span {
                                    class: "close",
                                    title: if dirty { "Unsaved changes — click to close" } else { "Close tab" },
                                    // Don't let a mousedown on × arm a tab drag / re-select.
                                    onmousedown: move |e| e.stop_propagation(),
                                    onclick: move |e| { e.stop_propagation(); dispatch(state, Action::CloseTab(id)); },
                                    span { class: "close-dot" }
                                    span { class: "close-x", "×" }
                                }
                            }
                        }
                    }
                }
            }
            // Trailing gap when dropping past the last visible tab.
            if dragging && insert == Some(visible_len) {
                div { class: "ws-tab-slot",
                    span { class: "ws-tab-slot-fill", "{ghost_name}" }
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

            // Floating ghost tab that rides the cursor during a drag (T1). Fixed-
            // positioned, so its place in the DOM doesn't matter; `pointer-events:none`
            // keeps it from stealing the mouseenter events that track the drop slot.
            if let Some(g) = ghost {
                {
                    let gx = g.x - g.off_x;
                    let gy = g.y - g.off_y;
                    let gname = g.name.clone();
                    rsx! {
                        div {
                            class: "ws-tab-ghost",
                            style: "transform: translate({gx}px, {gy}px);",
                            Dot { size: 6, color: "var(--accent)" }
                            Body { "{gname}" }
                        }
                    }
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
