//! Left sidebar: catalog filter + TABLES + VIEWS.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, CatalogKind, RegStatus};
use crate::ui::icons;

#[component]
pub fn Sidebar() -> Element {
    let state = use_context::<Signal<AppState>>();
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
                    {render_table(state, i, &filter, &selected)}
                }

                // ---- VIEWS ----
                div { class: "row", style: "gap:6px;padding:14px 6px 6px;",
                    span { class: "sec-label", "VIEWS · {nview}" }
                }
                for i in 0..nview {
                    {render_view(state, i)}
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
                        {render_saved_query(state, i)}
                    }
                }
            }
        }
    }
}

/// Collapsed catalog: a thin icon rail (RustRover-style) shown instead of hiding
/// the sidebar entirely. Rendered by `app` when `!sidebar_open`.
#[component]
pub fn SidebarRail() -> Element {
    let state = use_context::<Signal<AppState>>();
    rsx! {
        aside { class: "ps-rail",
            button {
                class: "rail-btn",
                title: "Expand catalog",
                onclick: move |_| dispatch(state, Action::ToggleSidebar),
                {icons::expand_right(17)}
            }
            div { class: "rail-sep" }
            button {
                class: "rail-btn accent",
                title: "Catalog",
                onclick: move |_| dispatch(state, Action::ToggleSidebar),
                {icons::database(17)}
            }
            button {
                class: "rail-btn",
                title: "New table",
                onclick: move |_| dispatch(state, Action::OpenConfigNew),
                {icons::plus(16)}
            }
        }
    }
}

fn render_saved_query(state: Signal<AppState>, i: usize) -> Element {
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
                    dispatch(state, Action::OpenCatalogMenu { kind: CatalogKind::Query, name: nm_ctx.clone(), x: c.x, y: c.y });
                },
                span { style: "color:var(--purple);display:flex;", {icons::brackets(14)} }
                span { class: "tname", "{name}" }
                div { style: "flex:1;" }
                button {
                    class: "row-menu",
                    title: "Actions",
                    onclick: move |e| {
                        e.stop_propagation();
                        let c = e.client_coordinates();
                        dispatch(state, Action::OpenCatalogMenu { kind: CatalogKind::Query, name: nm_menu.clone(), x: c.x, y: c.y });
                    },
                    {icons::dots(14)}
                }
            }
        }
    }
}

fn render_table(
    state: Signal<AppState>,
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
    let meta = t.meta.clone();
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

    let status_icon = match status {
        RegStatus::Loading => "⏳",
        RegStatus::Ready => "",
        RegStatus::Failed => "⚠",
    };
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
                    dispatch(state, Action::OpenCatalogMenu { kind: CatalogKind::Table, name: nm_ctx.clone(), x: c.x, y: c.y });
                },
                span { style: "color:var(--dim2);display:flex;",
                    if open { {icons::chevron_down(12)} } else { {icons::chevron_right(12)} }
                }
                span { style: "color:var(--dim);display:flex;", {icons::table(14)} }
                span { class: "tname", "{name}" }
                span { class: "tmeta", "{status_icon} {meta}" }
                button {
                    class: "row-menu",
                    title: "Actions",
                    onclick: move |e| {
                        e.stop_propagation();
                        let c = e.client_coordinates();
                        dispatch(state, Action::OpenCatalogMenu { kind: CatalogKind::Table, name: nm_menu.clone(), x: c.x, y: c.y });
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

fn render_view(state: Signal<AppState>, i: usize) -> Element {
    let s = state.read();
    let Some(v) = s.project.views.get(i) else {
        return rsx! {};
    };
    let name = v.name.clone();
    let open = v.open;
    let cols: Vec<(String, String, &'static str, &'static str)> = v
        .columns
        .iter()
        .map(|c| (c.name.clone(), c.dtype.clone(), c.kind.dot_class(), c.kind.text_class()))
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
                    dispatch(state, Action::OpenCatalogMenu { kind: CatalogKind::View, name: nm_ctx.clone(), x: c.x, y: c.y });
                },
                span { style: "color:var(--dim2);display:flex;",
                    if open { {icons::chevron_down(12)} } else { {icons::chevron_right(12)} }
                }
                span { style: "color:var(--purple);display:flex;", {icons::eye(14)} }
                span { class: "tname", "{name}" }
                span { class: "tmeta", "view" }
                button {
                    class: "row-menu",
                    title: "Actions",
                    onclick: move |e| {
                        e.stop_propagation();
                        let c = e.client_coordinates();
                        dispatch(state, Action::OpenCatalogMenu { kind: CatalogKind::View, name: nm_menu.clone(), x: c.x, y: c.y });
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
