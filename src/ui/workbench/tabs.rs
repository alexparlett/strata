//! The workspace tab strip: right-click `ContextMenu`, ⋯ overflow + ⌄ "show all tabs"
//! `DropdownMenu`s, and inline rename. All
//! transient tab-strip UI — the context menu, plus the inline-rename target +
//! draft text — is component-local `use_signal` state, never in a shared store; only
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

use crate::action::{dispatch, Action};
use crate::session::{SessionStoreExt, WorkspaceId, WorkspaceStoreExt};
use crate::ui::components::{
    Body, Caption, ContextMenu, Dot, DropdownMenu, Icon, IconButton, IconButtonVariant, MenuItem,
    MenuSep, Point, Prose, RectAlign, TextInput,
};
use crate::ui::icons::{IconName, IconSize};

/// Live tab drag-to-reorder state — component-local to [`Tabs`], never global. The
/// dragged tab, its original strip index, and the current drop slot (both in full
/// strip order). The floating ghost is the webview's native drag image.
#[derive(Clone, PartialEq)]
struct TabDrag {
    id: WorkspaceId,
    from: usize,
    insert: usize,
}

#[component]
pub fn Tabs() -> Element {
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
    // Per-tab element handles, so activating a tab (new tab, switch, or a jump from
    // the Problems view) can scroll it into view within the horizontal strip.
    let mut tab_refs = use_signal(|| HashMap::<WorkspaceId, Rc<MountedData>>::new());
    // Live drag-to-reorder (T1) — Tabs-local, never global. `drag` holds the render-
    // affecting drop slot; `drag_x` is the high-frequency pointer x for edge auto-
    // scroll, kept separate so updating it doesn't re-render the strip.
    let mut drag = use_signal(|| None::<TabDrag>);
    let mut drag_x = use_signal(|| 0.0_f64);

    // Read the active id + each entry through their lenses, so a `switch`
    // (`.active().set`) or a structural / per-field write re-renders the strip —
    // matching how `session` mutates the store.
    let sess = crate::session::store();
    let active = sess.active().cloned();
    // Keep the active tab visible: when the active id changes (switch, new tab, or a
    // Problems jump) scroll its element into view. Reads `active` *inside* the effect so
    // it re-runs on change; a fresh tab whose ref isn't stored yet is covered by the
    // per-tab `onmounted` scroll below.
    use_effect(move || {
        let a = crate::session::store().active().cloned();
        if let Some(m) = tab_refs.peek().get(&a).cloned() {
            spawn(async move {
                let _ = m.scroll_to(ScrollBehavior::Instant).await;
            });
        }
    });
    let renaming_now = renaming();
    let rename_draft = rename_val();
    let mut ws: Vec<(WorkspaceId, String, bool)> = Vec::new();
    for w in sess.workspaces().iter() {
        ws.push((w.id().cloned(), w.name().cloned(), w.read().is_dirty()));
    }
    // Live drag-to-reorder (T1). Reading the local `drag` signal here re-renders the
    // strip as the drop slot changes, so the insertion gap tracks the pointer. The
    // dragged tab stays in place (the floating ghost is the webview's native drag
    // image); the gap shows where it will land.
    let dragc = drag();
    let dragging = dragc.is_some();
    let drag_id = dragc.as_ref().map(|d| d.id);
    let insert = dragc.as_ref().map(|d| d.insert);
    let ghost_name = drag_id
        .and_then(|did| ws.iter().find(|(i, _, _)| *i == did).map(|(_, n, _)| n.clone()))
        .unwrap_or_default();

    // Every tab renders in strip order — the dragged one stays put (its native drag
    // image is the ghost), so `vi == oi` and nothing is lifted out.
    let mut render: Vec<(usize, usize, WorkspaceId, String, bool)> = Vec::new();
    for (oi, (id, name, dirty)) in ws.iter().enumerate() {
        render.push((oi, oi, *id, name.clone(), *dirty));
    }
    let visible_len = ws.len();

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
                            // Draggable enables HTML5 drag events for the reorder. Off while
                            // renaming so the inline input keeps normal text interaction.
                            draggable: !is_rename,
                            // Store the handle (for scroll-into-view) + measure the tab so
                            // drag hit-testing knows its midpoint. If this tab mounts already
                            // active (freshly created), scroll it into view straight away.
                            onmounted: move |e| {
                                let data = e.data();
                                tab_refs.write().insert(id, data.clone());
                                if id == active {
                                    let d = data.clone();
                                    spawn(async move {
                                        let _ = d.scroll_to(ScrollBehavior::Instant).await;
                                    });
                                }
                                spawn(async move {
                                    if let Ok(r) = data.get_client_rect().await {
                                        widths.write().insert(id, r.size.width);
                                    }
                                });
                            },
                            // Mousedown just selects the tab; the reorder runs on the HTML5 drag
                            // events below, so the tab keeps receiving move/end events even once
                            // the pointer leaves the strip. No `prevent_default` — it would block
                            // the native drag from starting.
                            onmousedown: move |e| {
                                if is_rename { return; }
                                if e.trigger_button() != Some(MouseButton::Primary) { return; }
                                if id != active { dispatch(Action::SwitchTab(id)); }
                            },
                            // Arm the Tabs-local drag and start edge auto-scroll: while the pointer
                            // sits near the track's left/right edge, keep scrolling the strip so
                            // off-screen tabs stay reachable. Self-terminates when the drag clears.
                            ondragstart: move |e| {
                                drag.set(Some(TabDrag { id, from: oi, insert: oi }));
                                drag_x.set(e.client_coordinates().x);
                                spawn(async move {
                                    loop {
                                        if drag.peek().is_none() {
                                            break;
                                        }
                                        let track = scroll_ref.peek().clone();
                                        let x = drag_x.peek().clone();
                                        if let Some(track) = track {
                                            if let Ok(r) = track.get_client_rect().await {
                                                let left = r.origin.x;
                                                let right = r.origin.x + r.size.width;
                                                let dir = if x < left + 52.0 {
                                                    -1.0
                                                } else if x > right - 52.0 {
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
                                        tokio::time::sleep(std::time::Duration::from_millis(16)).await;
                                    }
                                });
                            },
                            // `ondrag` fires on the source tab window-wide — feed the live pointer
                            // x to the auto-scroll loop (kept off `drag` so it doesn't re-render
                            // the strip). The final drag event reports 0, so skip it.
                            ondrag: move |e| {
                                let x = e.client_coordinates().x;
                                if x != 0.0 {
                                    drag_x.set(x);
                                }
                            },
                            // Dragging over this tab picks the drop slot: left half → before it,
                            // right half → after. `prevent_default` keeps the drag a valid move.
                            ondragover: move |e| {
                                e.prevent_default();
                                let Some(d) = drag.peek().clone() else { return };
                                let w = widths.peek().get(&id).copied().unwrap_or(140.0);
                                let want = if e.element_coordinates().x < w / 2.0 { vi } else { vi + 1 };
                                if d.insert != want {
                                    if let Some(dd) = drag.write().as_mut() {
                                        dd.insert = want;
                                    }
                                }
                            },
                            // Drop: commit the reorder (durable → autosaves) unless it landed back
                            // in place, then clear the local drag.
                            ondragend: move |_| {
                                if let Some(d) = drag.write().take() {
                                    let target = if d.insert > d.from { d.insert - 1 } else { d.insert };
                                    if target != d.from {
                                        dispatch(Action::MoveTab { id: d.id, insert: target });
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
                            // No leading dot (V29): the active tab already reads as active
                            // from its background + accent bar, and dirty is the trailing
                            // close slot (T4) — the dot only duplicated both.
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
                                        Key::Enter => { e.prevent_default(); commit_rename(renaming, rename_val, id); }
                                        Key::Escape => { e.prevent_default(); renaming.set(None); }
                                        _ => {}
                                    },
                                    onblur: move |_| commit_rename(renaming, rename_val, id),
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
                                    onclick: move |e| { e.stop_propagation(); dispatch(Action::CloseTab(id)); },
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
                    onclick: move |_| dispatch(Action::NewTab),
                }
                DropdownMenu {
                    class: "icon-btn plain", style: "width:26px;height:28px;", title: "Show all tabs",
                    align: RectAlign::BOTTOM_END, width: 320, open: tab_list_open,
                    trigger: rsx! { Icon { name: IconName::ChevronDown, size: IconSize::Sm } },
                    {tab_list_body(tab_list_open, tab_list_query, active)}
                }
                DropdownMenu {
                    class: "icon-btn plain", style: "width:24px;height:28px;", title: "Tab actions",
                    align: RectAlign::BOTTOM_END,
                    trigger: rsx! { Icon { name: IconName::Dots, size: IconSize::Sm } },
                    {overflow_menu_items(active, crate::session::has_closed())}
                }
            }

            // Self-contained tab context menu (right-click → ContextMenu).
            if let Some((id, at)) = tab_menu() {
                ContextMenu { on_close: move |_| tab_menu.set(None), at: Some(at),
                    {tab_menu_items(tab_menu, renaming, rename_val, id)}
                }
            }

        }
    }
}

/// Commit the inline rename for workspace `id` (Enter / blur): dispatch the durable
/// rename, then leave rename mode. A no-op when not renaming, so the Enter that
/// already committed doesn't fire again on the follow-up blur.
fn commit_rename(
    mut renaming: Signal<Option<WorkspaceId>>,
    rename_val: Signal<String>,
    id: WorkspaceId,
) {
    if renaming.peek().is_none() {
        return;
    }
    let v = rename_val.peek().clone();
    dispatch(Action::RenameTab(id, v));
    renaming.set(None);
}

/// Rows for a workspace-tab context menu. Each dismisses the popup then acts —
/// "Rename" seeds the component-local rename signals; the rest dispatch actions.
fn tab_menu_items(
    mut tab_menu: Signal<Option<(WorkspaceId, Point)>>,
    mut renaming: Signal<Option<WorkspaceId>>,
    mut rename_val: Signal<String>,
    id: WorkspaceId,
) -> Element {
    let can_reopen = crate::session::has_closed();
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
            onclick: move |_| { tab_menu.set(None); dispatch(Action::DuplicateTab(id)); } }
        MenuSep {}
        MenuItem { label: "Close".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(Action::CloseTab(id)); } }
        MenuItem { label: "Close others".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(Action::CloseOtherTabs(id)); } }
        MenuItem { label: "Close to the right".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(Action::CloseTabsRight(id)); } }
        MenuItem { label: "Close all".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(Action::CloseAllTabs); } }
        MenuSep {}
        MenuItem { label: "Reopen closed tab".to_string(), meta: crate::keymap::hint(crate::config::Command::ReopenTab), disabled: !can_reopen,
            onclick: move |_| { tab_menu.set(None); dispatch(Action::ReopenTab); } }
    }
}

/// Rows for the ⋯ tab-overflow menu (S8): whole-strip actions (not tied to one tab).
fn overflow_menu_items(active: WorkspaceId, can_reopen: bool) -> Element {
    rsx! {
        MenuItem { label: "Close all tabs".to_string(),
            onclick: move |_| dispatch(Action::CloseAllTabs) }
        MenuItem { label: "Close other tabs".to_string(),
            onclick: move |_| dispatch(Action::CloseOtherTabs(active)) }
        MenuSep {}
        MenuItem { label: "Reopen closed tab".to_string(), meta: crate::keymap::hint(crate::config::Command::ReopenTab), disabled: !can_reopen,
            onclick: move |_| dispatch(Action::ReopenTab) }
    }
}

/// Body of the "show all tabs" searchable popover (S8): a filter box + a row per
/// workspace (click switches, × closes). Reads the live session so closing a row
/// updates the list in place; Enter opens the first match.
fn tab_list_body(
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
                            dispatch(Action::SwitchTab(id));
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
                    onclick: move |_| dispatch(Action::SwitchTab(id)),
                    Dot { size: 6, color: if dirty { "var(--orange)" } else if is_active { "var(--accent)" } else { "var(--dim2)" } }
                    Body { class: "tablist-name", "{name}" }
                    span {
                        class: "tablist-close",
                        onclick: move |e| { e.stop_propagation(); dispatch(Action::CloseTab(id)); },
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
