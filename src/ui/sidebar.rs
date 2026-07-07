//! Left sidebar: catalog filter + TABLES + VIEWS.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, CatalogKind, RegStatus, RemoveKind, RemoveTarget};
use crate::ui::components::{Dialog, MenuItem, MenuSep, Point, Popup};
use crate::ui::icons;

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
    let filter = state.read().filter.clone();
    let selected = state.read().selected_col.clone();
    let width = state.read().sidebar_w;

    rsx! {
        aside { class: "ps-sidebar", style: "width:{width}px;",
            div { class: "filter",
                div { class: "field", style: "flex:1;",
                    {icons::search(13)}
                    input {
                        class: "input",
                        placeholder: "Filter catalog…",
                        value: "{filter}",
                        oninput: move |e| dispatch(state, Action::SetFilter(e.value())),
                    }
                }
                button {
                    class: "icon-btn",
                    title: "Collapse panel",
                    onclick: move |_| dispatch(state, Action::ToggleSidebar),
                    {icons::collapse_left(15)}
                }
            }

            div { class: "ps-catalog ps-scroll",
                // ---- TABLES ----
                div { class: "cat-head",
                    span { class: "sec-label", "TABLES · {ntab}" }
                    button { class: "cat-new", onclick: move |_| dispatch(state, Action::OpenConfigNew),
                        {icons::plus(11)} "New"
                    }
                }

                for i in 0..ntab {
                    {render_table(state, menu, i, &filter, &selected)}
                }

                // ---- VIEWS ----
                div { class: "row", style: "gap:6px;padding:14px 6px 6px;",
                    span { class: "sec-label", "VIEWS · {nview}" }
                }
                for i in 0..nview {
                    {render_view(state, menu, i)}
                }

                // ---- SAVED QUERIES (always shown, like Tables/Views) ----
                div { class: "row", style: "gap:6px;padding:14px 6px 6px;",
                    span { class: "sec-label", "QUERIES · {nquery}" }
                }
                if nquery == 0 {
                    div { style: "padding:4px 8px 6px;font:400 11.5px var(--ui);color:var(--faint);",
                        "No saved queries yet" }
                } else {
                    for i in 0..nquery {
                        {render_saved_query(state, menu, i)}
                    }
                }
            }

            // Self-contained catalog row menu (egui-style Popup container).
            if let Some(t) = menu() {
                Popup { on_close: move |_| menu.set(None), at: t.at,
                    {catalog_menu_items(state, menu, remove, t)}
                }
            }
            // The remove-confirm dialog, also sidebar-local (opened from a row menu).
            if let Some(t) = remove() {
                {remove_dialog(state, remove, t)}
            }
        }
    }
}

/// The rows for a catalog context menu, by kind. Each item dismisses the popup
/// then dispatches its concrete action.
fn catalog_menu_items(
    state: Signal<AppState>,
    mut menu: Signal<Option<CtxTarget>>,
    mut remove: Signal<Option<RemoveTarget>>,
    t: CtxTarget,
) -> Element {
    let name = t.name.clone();
    match t.kind {
        CatalogKind::Table => {
            let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
            rsx! {
                MenuItem { icon: icons::play(14), label: "View table".to_string(),
                    onclick: move |_| { menu.set(None); dispatch(state, Action::LoadSelectStar(n1.clone())); } }
                MenuItem { icon: icons::gear(14), label: "Configure".to_string(),
                    onclick: move |_| { menu.set(None); dispatch(state, Action::OpenConfigEdit(n2.clone())); } }
                MenuSep {}
                MenuItem { icon: icons::trash(14), label: "Drop table".to_string(), danger: true,
                    onclick: move |_| { menu.set(None); remove.set(Some(RemoveTarget { kind: RemoveKind::Table, name: n3.clone() })); } }
            }
        }
        CatalogKind::View => {
            let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
            rsx! {
                MenuItem { icon: icons::play(14), label: "View view".to_string(),
                    onclick: move |_| { menu.set(None); dispatch(state, Action::LoadSelectStar(n1.clone())); } }
                MenuItem { icon: icons::pencil(14), label: "Edit query".to_string(),
                    onclick: move |_| { menu.set(None); dispatch(state, Action::EditView(n2.clone())); } }
                MenuSep {}
                MenuItem { icon: icons::trash(14), label: "Drop view".to_string(), danger: true,
                    onclick: move |_| { menu.set(None); remove.set(Some(RemoveTarget { kind: RemoveKind::View, name: n3.clone() })); } }
            }
        }
        CatalogKind::Query => {
            let (n1, n2) = (name.clone(), name.clone());
            rsx! {
                MenuItem { icon: icons::pencil(14), label: "Open in new tab".to_string(),
                    onclick: move |_| { menu.set(None); dispatch(state, Action::OpenSavedQuery(n1.clone())); } }
                MenuItem { icon: icons::trash(14), label: "Delete query".to_string(), danger: true,
                    onclick: move |_| { menu.set(None); dispatch(state, Action::DeleteSavedQuery(n2.clone())); } }
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
                div { class: "confirm-ico", {icons::trash(20)} }
                div { style: "flex:1;min-width:0;",
                    div { class: "confirm-title", "{title} " span { class: "nm", "{name}" } "?" }
                    div { class: "confirm-body", "{body}" }
                }
            }
            div { class: "confirm-foot",
                button { class: "btn-ghost", onclick: move |_| remove.set(None), "Cancel" }
                button {
                    class: "btn-danger",
                    onclick: move |_| {
                        dispatch(state, Action::ConfirmRemove { kind, name: confirm_name.clone() });
                        remove.set(None);
                    },
                    {icons::trash(14)}
                    "{btn}"
                }
            }
        }
    }
}

fn render_saved_query(
    state: Signal<AppState>,
    mut menu: Signal<Option<CtxTarget>>,
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
        div { style: "margin-bottom:3px;",
            div {
                class: "tbl-row",
                onclick: move |_| dispatch(state, Action::OpenSavedQuery(nm_open.clone())),
                oncontextmenu: move |e| {
                    e.prevent_default();
                    let c = e.client_coordinates();
                    menu.set(Some(CtxTarget { kind: CatalogKind::Query, name: nm_ctx.clone(), at: Point { x: c.x, y: c.y } }));
                },
                span { style: "color:var(--purple);display:flex;", {icons::brackets(14)} }
                span { class: "tname", "{name}" }
                button {
                    class: "row-menu",
                    title: "Actions",
                    onclick: move |e| {
                        e.stop_propagation();
                        let c = e.client_coordinates();
                        menu.set(Some(CtxTarget { kind: CatalogKind::Query, name: nm_menu.clone(), at: Point { x: c.x, y: c.y } }));
                    },
                    {icons::dots(14)}
                }
            }
        }
    }
}

fn render_table(
    state: Signal<AppState>,
    mut menu: Signal<Option<CtxTarget>>,
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
                c.kind.dot_class(),
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
        div { style: "margin-bottom:3px;",
            div {
                class: "tbl-row",
                onclick: move |_| dispatch(state, Action::ToggleTableOpen(i)),
                oncontextmenu: move |e| {
                    e.prevent_default();
                    let c = e.client_coordinates();
                    menu.set(Some(CtxTarget { kind: CatalogKind::Table, name: nm_ctx.clone(), at: Point { x: c.x, y: c.y } }));
                },
                span { style: "color:var(--dim2);display:flex;",
                    if open { {icons::chevron_down(12)} } else { {icons::chevron_right(12)} }
                }
                span { style: "color:var(--dim);display:flex;", {icons::table(14)} }
                span { class: "tname", "{name}" }
                button {
                    class: "row-menu",
                    title: "Actions",
                    onclick: move |e| {
                        e.stop_propagation();
                        let c = e.client_coordinates();
                        menu.set(Some(CtxTarget { kind: CatalogKind::Table, name: nm_menu.clone(), at: Point { x: c.x, y: c.y } }));
                    },
                    {icons::dots(14)}
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
                                    span { class: "dot {dot}" }
                                    span { class: "cname", "{cn}" }
                                    if is_part { span { class: "pill", "PART" } }
                                    span { class: "ctype {tcls}", "{ct}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_view(state: Signal<AppState>, mut menu: Signal<Option<CtxTarget>>, i: usize) -> Element {
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
                c.kind.dot_class(),
                c.kind.text_class(),
            )
        })
        .collect();
    drop(s);

    let nm_ctx = name.clone();
    let nm_menu = name.clone();

    rsx! {
        div { style: "margin-bottom:3px;",
            div {
                class: "tbl-row",
                onclick: move |_| dispatch(state, Action::ToggleViewOpen(i)),
                oncontextmenu: move |e| {
                    e.prevent_default();
                    let c = e.client_coordinates();
                    menu.set(Some(CtxTarget { kind: CatalogKind::View, name: nm_ctx.clone(), at: Point { x: c.x, y: c.y } }));
                },
                span { style: "color:var(--dim2);display:flex;",
                    if open { {icons::chevron_down(12)} } else { {icons::chevron_right(12)} }
                }
                span { style: "color:var(--purple);display:flex;", {icons::eye(14)} }
                span { class: "tname", "{name}" }
                button {
                    class: "row-menu",
                    title: "Actions",
                    onclick: move |e| {
                        e.stop_propagation();
                        let c = e.client_coordinates();
                        menu.set(Some(CtxTarget { kind: CatalogKind::View, name: nm_menu.clone(), at: Point { x: c.x, y: c.y } }));
                    },
                    {icons::dots(14)}
                }
            }
            if open {
                div { class: "tbl-cols",
                    for (cn, ct, dot, tcls) in cols {
                        div { class: "col-row",
                            span { class: "dot {dot}" }
                            span { class: "cname", "{cn}" }
                            span { class: "ctype {tcls}", "{ct}" }
                        }
                    }
                }
            }
        }
    }
}
