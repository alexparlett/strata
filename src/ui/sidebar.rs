//! Left sidebar: catalog filter + TABLES + VIEWS.

use dioxus::prelude::*;

use crate::action::panel::Resizer;
use crate::action::{dispatch, Action};
use crate::state::{AppState, CatalogKind, RegStatus, RemoveKind, RemoveTarget};
use crate::ui::components::{
    Button, ButtonVariant, Caption, ContextMenu, Dialog, Dot, DropdownMenu, Eyebrow, Icon,
    IconButton, IconButtonVariant, MenuItem, MenuSep, Meta, Micro, MonoValue, Point, Readout,
    RectAlign, SearchBar, Title,
};
use crate::ui::icons::{IconName, IconSize};

/// A catalog row's open context menu (self-contained sidebar state).
#[derive(Clone)]
struct CtxTarget {
    kind: CatalogKind,
    name: String,
    at: Point,
}

#[component]
pub fn Sidebar() -> Element {
    let state = use_context::<Signal<AppState>>();
    // Self-contained: the catalog row menu lives here, not in `AppState`.
    let mut menu = use_signal(|| None::<CtxTarget>);
    // The remove-confirm dialog is likewise sidebar-local, opened from a row menu.
    let remove = use_signal(|| None::<RemoveTarget>);
    let ntab = state.read().project.tables.len();
    let nview = state.read().project.views.len();
    let nquery = state.read().project.saved_queries.len();
    // Catalog filter is pure sidebar UI — kept local (F7), not in AppState.
    let mut filter = use_signal(String::new);
    let selected = state.read().selected_col.clone();
    // The sidebar owns its own width — a local reactive signal, not global state.
    let width = use_signal(|| 288.0);

    rsx! {
        aside { class: "ps-sidebar", style: "width:{width}px;",
            div { class: "filter",
                SearchBar {
                    value: filter(),
                    oninput: move |v| filter.set(v),
                    placeholder: "Filter catalog…",
                    grow: true,
                }
                IconButton { icon: IconName::CollapseLeft,
                    variant: IconButtonVariant::Toolbar,
                    title: "Collapse panel",
                    onclick: move |_| dispatch(state, Action::ToggleSidebar),
                }
            }

            div { class: "ps-catalog ps-scroll",
                // ---- TABLES ----
                div { class: "cat-head",
                    Eyebrow { class: "sec-label", "TABLES · {ntab}" }
                    Button { variant: ButtonVariant::Compact, icon: IconName::Plus, icon_size: IconSize::Xs, onclick: move |_| dispatch(state, Action::OpenConfigNew), "New" }
                }

                for i in 0..ntab {
                    {render_table(state, menu, remove, i, &filter(), &selected)}
                }

                // ---- VIEWS ----
                div { class: "row", style: "gap:var(--sp-3);padding:var(--sp-4) var(--sp-3) var(--sp-3);",
                    Eyebrow { class: "sec-label", "VIEWS · {nview}" }
                }
                for i in 0..nview {
                    {render_view(state, menu, remove, i)}
                }

                // ---- SAVED QUERIES (always shown, like Tables/Views) ----
                div { class: "row", style: "gap:var(--sp-3);padding:var(--sp-4) var(--sp-3) var(--sp-3);",
                    Eyebrow { class: "sec-label", "QUERIES · {nquery}" }
                }
                if nquery == 0 {
                    Caption { style: "display:block;padding:var(--sp-2) var(--sp-3) var(--sp-3);color:var(--faint);",
                        "No saved queries yet" }
                } else {
                    for i in 0..nquery {
                        {render_saved_query(state, menu, remove, i)}
                    }
                }
            }

            // Self-contained catalog row menu (right-click → ContextMenu).
            if let Some(t) = menu() {
                ContextMenu { on_close: move |_| menu.set(None), at: Some(t.at),
                    {catalog_menu_items(state, remove, t.kind, t.name.clone())}
                }
            }
            // The remove-confirm dialog, also sidebar-local (opened from a row menu).
            if let Some(t) = remove() {
                {remove_dialog(state, remove, t)}
            }
        }
        // Right-edge resize handle — owns nothing but the sidebar's width signal.
        Resizer { axis_x: true, sign: 1.0, min: 210.0, max: 520.0, size: width }
    }
}

/// The rows for a catalog row menu, by kind. Shared by the ⋮ `DropdownMenu` and the
/// right-click `ContextMenu` — both dismiss via their own close-wrapper, so items just
/// dispatch (open the remove-confirm for drops).
fn catalog_menu_items(
    state: Signal<AppState>,
    mut remove: Signal<Option<RemoveTarget>>,
    kind: CatalogKind,
    name: String,
) -> Element {
    match kind {
        CatalogKind::Table => {
            let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
            rsx! {
                MenuItem { icon: IconName::Play, icon_size: IconSize::Sm, label: "View table".to_string(),
                    onclick: move |_| dispatch(state, Action::LoadSelectStar(n1.clone())) }
                MenuItem { icon: IconName::Gear, icon_size: IconSize::Sm, label: "Configure".to_string(),
                    onclick: move |_| dispatch(state, Action::OpenConfigEdit(n2.clone())) }
                MenuSep {}
                MenuItem { icon: IconName::Trash, icon_size: IconSize::Sm, label: "Drop table".to_string(), danger: true,
                    onclick: move |_| remove.set(Some(RemoveTarget { kind: RemoveKind::Table, name: n3.clone() })) }
            }
        }
        CatalogKind::View => {
            let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
            rsx! {
                MenuItem { icon: IconName::Play, icon_size: IconSize::Sm, label: "View view".to_string(),
                    onclick: move |_| dispatch(state, Action::LoadSelectStar(n1.clone())) }
                MenuItem { icon: IconName::Pencil, icon_size: IconSize::Sm, label: "Edit query".to_string(),
                    onclick: move |_| dispatch(state, Action::EditView(n2.clone())) }
                MenuSep {}
                MenuItem { icon: IconName::Trash, icon_size: IconSize::Sm, label: "Drop view".to_string(), danger: true,
                    onclick: move |_| remove.set(Some(RemoveTarget { kind: RemoveKind::View, name: n3.clone() })) }
            }
        }
        CatalogKind::Query => {
            let (n1, n2) = (name.clone(), name.clone());
            rsx! {
                MenuItem { icon: IconName::Pencil, icon_size: IconSize::Sm, label: "Open in new tab".to_string(),
                    onclick: move |_| dispatch(state, Action::OpenSavedQuery(n1.clone())) }
                MenuItem { icon: IconName::Trash, icon_size: IconSize::Sm, label: "Delete query".to_string(), danger: true,
                    onclick: move |_| dispatch(state, Action::DeleteSavedQuery(n2.clone())) }
            }
        }
    }
}

/// The remove-confirmation dialog (drop table / view) — a sidebar-local `Dialog`.
/// The `remove` signal owns open/close; confirming dispatches the actual drop.
fn remove_dialog(
    state: Signal<AppState>,
    mut remove: Signal<Option<RemoveTarget>>,
    t: RemoveTarget,
) -> Element {
    let (title, body, btn) = match t.kind {
        RemoveKind::Table => (
            "Drop table",
            "Removes the table from the catalog. Files on disk are not deleted.",
            "Drop table",
        ),
        RemoveKind::View => (
            "Drop view",
            "Drops the saved view. The tables it queries are unaffected.",
            "Drop view",
        ),
    };
    let kind = t.kind;
    let name = t.name.clone();
    let confirm_name = t.name;

    rsx! {
        Dialog { on_close: move |_| remove.set(None), card_class: "confirm".to_string(), z: 78,
            div { class: "confirm-head",
                div { class: "confirm-ico", Icon { name: IconName::Trash, size: IconSize::Px(20) } }
                div { style: "flex:1;min-width:0;",
                    Title { class: "confirm-title", "{title} " span { class: "nm", "{name}" } "?" }
                    Readout { class: "confirm-body", "{body}" }
                }
            }
            div { class: "confirm-foot",
                Button { variant: ButtonVariant::Secondary, onclick: move |_| remove.set(None), "Cancel" }
                Button {
                    variant: ButtonVariant::Danger,
                    icon: IconName::Trash, icon_size: IconSize::Sm,
                    onclick: move |_| {
                        dispatch(state, Action::ConfirmRemove { kind, name: confirm_name.clone() });
                        remove.set(None);
                    },
                    "{btn}"
                }
            }
        }
    }
}

fn render_saved_query(
    state: Signal<AppState>,
    mut menu: Signal<Option<CtxTarget>>,
    remove: Signal<Option<RemoveTarget>>,
    i: usize,
) -> Element {
    let s = state.read();
    let Some(q) = s.project.saved_queries.get(i) else {
        return rsx! {};
    };
    let name = q.name.clone();
    drop(s);

    let nm_open = name.clone();
    let nm_ctx = name.clone();
    let nm_menu = name.clone();

    rsx! {
        div { style: "margin-bottom:var(--sp-1);",
            div {
                class: "tbl-row",
                onclick: move |_| dispatch(state, Action::OpenSavedQuery(nm_open.clone())),
                oncontextmenu: move |e| {
                    e.prevent_default();
                    let c = e.client_coordinates();
                    menu.set(Some(CtxTarget { kind: CatalogKind::Query, name: nm_ctx.clone(), at: Point { x: c.x, y: c.y } }));
                },
                Icon { name: IconName::Brackets, size: IconSize::Sm, color: "var(--purple)" }
                MonoValue { class: "tname", "{name}" }
                DropdownMenu {
                    class: "row-menu", title: "Actions", align: RectAlign::BOTTOM_END, width: 180,
                    trigger: rsx! { Icon { name: IconName::Dots, size: IconSize::Sm } },
                    {catalog_menu_items(state, remove, CatalogKind::Query, nm_menu.clone())}
                }
            }
        }
    }
}

fn render_table(
    state: Signal<AppState>,
    mut menu: Signal<Option<CtxTarget>>,
    remove: Signal<Option<RemoveTarget>>,
    i: usize,
    filter: &str,
    selected: &Option<(String, String)>,
) -> Element {
    let s = state.read();
    let Some(t) = s.project.tables.get(i) else {
        return rsx! {};
    };
    if !filter.is_empty() && !t.name.to_lowercase().contains(&filter.to_lowercase()) {
        return rsx! {};
    }
    let name = t.name.clone();
    let open = t.open;
    let status = t.status;
    let parts = t.partition_cols.clone();
    // owned column view models
    let cols: Vec<(String, String, &'static str, &'static str, bool, bool)> = t
        .columns
        .iter()
        .map(|c| {
            let is_part = parts.iter().any(|(n, _)| n == &c.name);
            let is_sel = selected
                .as_ref()
                .map_or(false, |(tn, cn)| tn == &name && cn == &c.name);
            (
                c.name.clone(),
                c.dtype.clone(),
                c.kind.dot_color(),
                c.kind.text_class(),
                is_part,
                is_sel,
            )
        })
        .collect();
    drop(s);

    let nm_ctx = name.clone();
    let nm_menu = name.clone();

    rsx! {
        div { style: "margin-bottom:var(--sp-1);",
            div {
                class: "tbl-row",
                onclick: move |_| dispatch(state, Action::ToggleTableOpen(i)),
                oncontextmenu: move |e| {
                    e.prevent_default();
                    let c = e.client_coordinates();
                    menu.set(Some(CtxTarget { kind: CatalogKind::Table, name: nm_ctx.clone(), at: Point { x: c.x, y: c.y } }));
                },
                span { style: "color:var(--dim2);display:flex;",
                    if open { Icon { name: IconName::ChevronDown, size: IconSize::Xs } } else { Icon { name: IconName::ChevronRight, size: IconSize::Xs } }
                }
                Icon { name: IconName::Table, size: IconSize::Sm, color: "var(--dim)" }
                MonoValue { class: "tname", "{name}" }
                DropdownMenu {
                    class: "row-menu", title: "Actions", align: RectAlign::BOTTOM_END, width: 180,
                    trigger: rsx! { Icon { name: IconName::Dots, size: IconSize::Sm } },
                    {catalog_menu_items(state, remove, CatalogKind::Table, nm_menu.clone())}
                }
            }
            if open {
                div { class: "tbl-cols",
                    for (cn, ct, dot, tcls, is_part, is_sel) in cols {
                        {
                            let table_nm = name.clone();
                            let col_nm = cn.clone();
                            rsx! {
                                div {
                                    class: if is_sel { "col-row sel" } else { "col-row" },
                                    onclick: move |_| dispatch(state, Action::SelectColumn {
                                        table: table_nm.clone(),
                                        column: col_nm.clone(),
                                    }),
                                    Dot { color: "{dot}", square: true, size: 6 }
                                    MonoValue { class: "cname", "{cn}" }
                                    if is_part { Micro { class: "pill", "PART" } }
                                    Meta { class: "ctype {tcls}", "{ct}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_view(
    state: Signal<AppState>,
    mut menu: Signal<Option<CtxTarget>>,
    remove: Signal<Option<RemoveTarget>>,
    i: usize,
) -> Element {
    let s = state.read();
    let Some(v) = s.project.views.get(i) else {
        return rsx! {};
    };
    let name = v.name.clone();
    let open = v.open;
    let cols: Vec<(String, String, &'static str, &'static str)> = v
        .columns
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.dtype.clone(),
                c.kind.dot_color(),
                c.kind.text_class(),
            )
        })
        .collect();
    drop(s);

    let nm_ctx = name.clone();
    let nm_menu = name.clone();

    rsx! {
        div { style: "margin-bottom:var(--sp-1);",
            div {
                class: "tbl-row",
                onclick: move |_| dispatch(state, Action::ToggleViewOpen(i)),
                oncontextmenu: move |e| {
                    e.prevent_default();
                    let c = e.client_coordinates();
                    menu.set(Some(CtxTarget { kind: CatalogKind::View, name: nm_ctx.clone(), at: Point { x: c.x, y: c.y } }));
                },
                span { style: "color:var(--dim2);display:flex;",
                    if open { Icon { name: IconName::ChevronDown, size: IconSize::Xs } } else { Icon { name: IconName::ChevronRight, size: IconSize::Xs } }
                }
                Icon { name: IconName::Eye, size: IconSize::Sm, color: "var(--purple)" }
                MonoValue { class: "tname", "{name}" }
                DropdownMenu {
                    class: "row-menu", title: "Actions", align: RectAlign::BOTTOM_END, width: 180,
                    trigger: rsx! { Icon { name: IconName::Dots, size: IconSize::Sm } },
                    {catalog_menu_items(state, remove, CatalogKind::View, nm_menu.clone())}
                }
            }
            if open {
                div { class: "tbl-cols",
                    for (cn, ct, dot, tcls) in cols {
                        div { class: "col-row",
                            Dot { color: "{dot}", square: true, size: 6 }
                            MonoValue { class: "cname", "{cn}" }
                            Meta { class: "ctype {tcls}", "{ct}" }
                        }
                    }
                }
            }
        }
    }
}
