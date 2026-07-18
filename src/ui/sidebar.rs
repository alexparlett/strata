//! Left sidebar: catalog filter + TABLES + VIEWS.

use std::collections::HashSet;

use dioxus::prelude::*;

use crate::action::panel::Resizer;
use crate::action::{dispatch, Action};
use crate::model::ColRef;
use crate::model::{CatalogKind, RemoveKind, RemoveTarget};
use crate::project::ProjectStoreExt;
use crate::ui::components::{
    Button, ButtonVariant, Caption, ContextMenu, Dialog, Dot, DropdownMenu, Eyebrow, Icon,
    IconButton, IconButtonVariant, MenuItem, MenuSep, Meta, Micro, MonoValue, Point, Readout,
    RectAlign, SearchBar, Title, Tooltip,
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
    // Self-contained: the catalog row menu lives here, component-local.
    let mut menu = use_signal(|| None::<CtxTarget>);
    // The remove-confirm dialog is likewise sidebar-local, opened from a row menu.
    let remove = use_signal(|| None::<RemoveTarget>);
    let ntab = crate::project::store().tables().read().len();
    let nview = crate::project::store().views().read().len();
    let nquery = crate::project::store().saved_queries().read().len();
    // Catalog filter is pure sidebar UI — kept component-local (F7).
    let mut filter = use_signal(String::new);
    // Collapsible catalog sections (default expanded) — sidebar-local UI.
    let mut tables_open = use_signal(|| true);
    let mut views_open = use_signal(|| true);
    let mut queries_open = use_signal(|| true);
    // Which struct columns are expanded (keyed "table::path") — sidebar-local.
    let expanded = use_signal(HashSet::<String>::new);
    // Catalog schema refresh — a brief optimistic spin while the re-infer fires.
    let mut rescanning = use_signal(|| false);
    let selected = crate::inspector::selected();
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
                IconButton {
                    icon: IconName::Refresh,
                    variant: IconButtonVariant::Ghost,
                    class: if rescanning() { "ps-spin" } else { "" },
                    disabled: rescanning(),
                    title: "Refresh schemas",
                    onclick: move |_| {
                        dispatch(Action::RescanCatalog);
                        rescanning.set(true);
                        spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                            rescanning.set(false);
                        });
                    },
                }
                IconButton { icon: IconName::Close,
                    variant: IconButtonVariant::Ghost,
                    title: "Close panel",
                    onclick: move |_| dispatch(Action::ToggleSidebar),
                }
            }

            div { class: "ps-catalog ps-scroll",
                // ---- TABLES ----
                div { class: "cat-head",
                    div { class: "sec-toggle", onclick: move |_| tables_open.set(!tables_open()),
                        span { class: "sec-chev",
                            if tables_open() { Icon { name: IconName::ChevronDown, size: IconSize::Xs } } else { Icon { name: IconName::ChevronRight, size: IconSize::Xs } }
                        }
                        Eyebrow { class: "sec-label", "TABLES · {ntab}" }
                    }
                    Button { variant: ButtonVariant::Compact, icon: IconName::Plus, icon_size: IconSize::Xs, onclick: move |_| dispatch(Action::OpenConfigNew), "New" }
                }

                if tables_open() {
                    for i in 0..ntab {
                        {render_table(menu, remove, i, &filter(), &selected, expanded)}
                    }
                }

                // ---- VIEWS ----
                div { class: "sec-toggle", style: "padding:var(--sp-4) var(--sp-3) var(--sp-3);", onclick: move |_| views_open.set(!views_open()),
                    span { class: "sec-chev",
                        if views_open() { Icon { name: IconName::ChevronDown, size: IconSize::Xs } } else { Icon { name: IconName::ChevronRight, size: IconSize::Xs } }
                    }
                    Eyebrow { class: "sec-label", "VIEWS · {nview}" }
                }
                if views_open() {
                    for i in 0..nview {
                        {render_view(menu, remove, i, &filter(), &selected, expanded)}
                    }
                }

                // ---- SAVED QUERIES ----
                div { class: "sec-toggle", style: "padding:var(--sp-4) var(--sp-3) var(--sp-3);", onclick: move |_| queries_open.set(!queries_open()),
                    span { class: "sec-chev",
                        if queries_open() { Icon { name: IconName::ChevronDown, size: IconSize::Xs } } else { Icon { name: IconName::ChevronRight, size: IconSize::Xs } }
                    }
                    Eyebrow { class: "sec-label", "QUERIES · {nquery}" }
                }
                if queries_open() {
                    if nquery == 0 {
                        Caption { style: "display:block;padding:var(--sp-2) var(--sp-3) var(--sp-3);color:var(--faint);",
                            "No saved queries yet" }
                    } else {
                        for i in 0..nquery {
                            {render_saved_query(menu, remove, i, &filter())}
                        }
                    }
                }
            }

            // Self-contained catalog row menu (right-click → ContextMenu).
            if let Some(t) = menu() {
                ContextMenu { on_close: move |_| menu.set(None), at: Some(t.at),
                    {catalog_menu_items(remove, t.kind, t.name.clone())}
                }
            }
            // The remove-confirm dialog, also sidebar-local (opened from a row menu).
            if let Some(t) = remove() {
                {remove_dialog(remove, t)}
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
    mut remove: Signal<Option<RemoveTarget>>,
    kind: CatalogKind,
    name: String,
) -> Element {
    match kind {
        CatalogKind::Table => {
            let (n1, n2, n3, n4) = (name.clone(), name.clone(), name.clone(), name.clone());
            // A scan already running for this table can't be started again (the engine
            // drops the duplicate anyway) — so say so rather than offer a no-op.
            let scanning = crate::project::is_profiling(&name);
            rsx! {
                MenuItem { icon: IconName::Play, icon_size: IconSize::Sm, label: "View table".to_string(),
                    onclick: move |_| dispatch(Action::LoadSelectStar(n1.clone())) }
                // Asks first, like the inspector's button — a full scan (D4).
                MenuItem { icon: IconName::Chart, icon_size: IconSize::Sm,
                    label: if scanning { "Profiling…".to_string() } else { "Profile".to_string() },
                    disabled: scanning,
                    onclick: move |_| dispatch(Action::AskProfileTable(n4.clone())) }
                MenuItem { icon: IconName::Gear, icon_size: IconSize::Sm, label: "Configure".to_string(),
                    onclick: move |_| dispatch(Action::OpenConfigEdit(n2.clone())) }
                MenuSep {}
                MenuItem { icon: IconName::Trash, icon_size: IconSize::Sm, label: "Drop table".to_string(), danger: true,
                    onclick: move |_| remove.set(Some(RemoveTarget { kind: RemoveKind::Table, name: n3.clone() })) }
            }
        }
        CatalogKind::View => {
            let (n1, n2, n3, n4) = (name.clone(), name.clone(), name.clone(), name.clone());
            let scanning = crate::project::is_profiling(&name);
            rsx! {
                MenuItem { icon: IconName::Play, icon_size: IconSize::Sm, label: "View view".to_string(),
                    onclick: move |_| dispatch(Action::LoadSelectStar(n1.clone())) }
                // A view has no footer, so a scan is the only way it learns anything —
                // it's worth more here than on a table.
                MenuItem { icon: IconName::Chart, icon_size: IconSize::Sm,
                    label: if scanning { "Profiling…".to_string() } else { "Profile".to_string() },
                    disabled: scanning,
                    onclick: move |_| dispatch(Action::AskProfileTable(n4.clone())) }
                MenuItem { icon: IconName::Pencil, icon_size: IconSize::Sm, label: "Edit query".to_string(),
                    onclick: move |_| dispatch(Action::EditView(n2.clone())) }
                MenuSep {}
                MenuItem { icon: IconName::Trash, icon_size: IconSize::Sm, label: "Drop view".to_string(), danger: true,
                    onclick: move |_| remove.set(Some(RemoveTarget { kind: RemoveKind::View, name: n3.clone() })) }
            }
        }
        CatalogKind::Query => {
            let (n1, n2) = (name.clone(), name.clone());
            rsx! {
                MenuItem { icon: IconName::Pencil, icon_size: IconSize::Sm, label: "Open in new tab".to_string(),
                    onclick: move |_| dispatch(Action::OpenSavedQuery(n1.clone())) }
                MenuItem { icon: IconName::Trash, icon_size: IconSize::Sm, label: "Delete query".to_string(), danger: true,
                    onclick: move |_| dispatch(Action::DeleteSavedQuery(n2.clone())) }
            }
        }
    }
}

/// The remove-confirmation dialog (drop table / view) — a sidebar-local `Dialog`.
/// The `remove` signal owns open/close; confirming dispatches the actual drop.
/// The drop-confirm consequence line (D10) — "N view(s) read this {noun} and will be
/// left invalid." Count only, not names: a busy table can back dozens of views, and a
/// list that long warns no better than a number. Empty when nothing depends on it.
///
/// Not "stop working": a view captures its sources by reference, so it keeps *running*
/// after the drop and only fails when the project is reopened and its SQL re-planned. So
/// it's flagged invalid in the catalog (the warning triangle) rather than broken now.
fn phrase(count: usize, noun: &str) -> String {
    match count {
        0 => String::new(),
        1 => format!("1 view reads this {noun} and will be left invalid."),
        n => format!("{n} views read this {noun} and will be left invalid."),
    }
}

fn remove_dialog(
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
    // What this drop leaves invalid (D10), resolved by the planner so it's transitive
    // through nested views. Neither case breaks anything *now* — a view holds its
    // sources by reference and keeps running until the project reloads and re-plans its
    // SQL. So dependents are flagged invalid, not broken (see `phrase`).
    let breaks_msg = match kind {
        RemoveKind::Table => phrase(crate::project::views_using(&name).len(), "table"),
        RemoveKind::View => phrase(crate::project::views_referencing(&name).len(), "view"),
    };

    rsx! {
        Dialog { on_close: move |_| remove.set(None), card_class: "confirm".to_string(), z: 78,
            div { class: "confirm-head",
                div { class: "confirm-ico", Icon { name: IconName::Trash, size: IconSize::Px(20) } }
                div { style: "flex:1;min-width:0;",
                    Title { class: "confirm-title", "{title} " span { class: "nm", "{name}" } "?" }
                    Readout { class: "confirm-body", "{body}" }
                    if !breaks_msg.is_empty() {
                        Readout { class: "confirm-warn", "{breaks_msg}" }
                    }
                }
            }
            div { class: "confirm-foot",
                Button { variant: ButtonVariant::Secondary, onclick: move |_| remove.set(None), "Cancel" }
                Button {
                    variant: ButtonVariant::Danger,
                    icon: IconName::Trash, icon_size: IconSize::Sm,
                    onclick: move |_| {
                        dispatch(Action::ConfirmRemove { kind, name: confirm_name.clone() });
                        remove.set(None);
                    },
                    "{btn}"
                }
            }
        }
    }
}

fn render_saved_query(
    mut menu: Signal<Option<CtxTarget>>,
    remove: Signal<Option<RemoveTarget>>,
    i: usize,
    filter: &str,
) -> Element {
    let store = crate::project::store();
    let sq = store.saved_queries();
    let s = sq.read();
    let Some(q) = s.get(i) else {
        return rsx! {};
    };
    if !filter.is_empty() && !q.name.to_lowercase().contains(&filter.to_lowercase()) {
        return rsx! {};
    }
    let name = q.name.clone();
    drop(s);

    let nm_open = name.clone();
    let nm_ctx = name.clone();
    let nm_menu = name.clone();

    rsx! {
        div { style: "margin-bottom:var(--sp-1);",
            div {
                class: "tbl-row",
                onclick: move |_| dispatch(Action::OpenSavedQuery(nm_open.clone())),
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
                    {catalog_menu_items(remove, CatalogKind::Query, nm_menu.clone())}
                }
            }
        }
    }
}

/// One flattened, visible catalog column row (a top-level column or an expanded
/// struct child). `depth` drives the indent; `has_children` / `is_expanded` the chevron.
struct ColRow {
    key: String,
    name: String,
    /// The column's path within the table (`["address", "city"]`) — what identifies it
    /// to the inspector. A `Vec`, not a dotted string: a column name may contain dots.
    path: Vec<String>,
    dtype: String,
    dot: &'static str,
    tcls: &'static str,
    is_part: bool,
    is_sel: bool,
    depth: usize,
    has_children: bool,
    is_expanded: bool,
}

/// Walk a catalog entry's column tree into the flat list of *visible* rows: every
/// column, plus the children of any expanded struct column, depth-first.
fn flatten_cols(
    owner: &str,
    parent: &[String],
    depth: usize,
    cols: &[crate::model::ColumnInfo],
    parts: &[(String, String)],
    selected: &Option<ColRef>,
    expanded: &HashSet<String>,
    out: &mut Vec<ColRow>,
) {
    for c in cols {
        let mut path = parent.to_vec();
        path.push(c.name.clone());
        // The expand key stays a display string — it only has to be unique per row, and
        // a collision would merely expand the wrong twig. Identity uses `ColRef`.
        let key = format!("{owner}::{}", path.join("."));
        let has_children = !c.children.is_empty();
        let is_expanded = has_children && expanded.contains(&key);
        out.push(ColRow {
            key,
            name: c.name.clone(),
            dtype: c.dtype.clone(),
            dot: c.kind.dot_color(),
            tcls: c.kind.text_class(),
            // Partition columns are a top-level concept only.
            is_part: depth == 0 && parts.iter().any(|(n, _)| n == &c.name),
            // Compare the whole path: by name alone, selecting `city` lit up every
            // `city` at any depth in the table.
            is_sel: selected
                .as_ref()
                .is_some_and(|s| s.owner == owner && s.path == path),
            depth,
            has_children,
            is_expanded,
            path,
        });
        if is_expanded {
            let child_parent = out.last().map(|r| r.path.clone()).unwrap_or_default();
            flatten_cols(
                owner,
                &child_parent,
                depth + 1,
                &c.children,
                parts,
                selected,
                expanded,
                out,
            );
        }
    }
}

/// A warning triangle for an invalid catalog row (D-validity), with the reason on hover.
/// Empty when there's no problem, so callers can drop it in unconditionally.
fn problem_badge(problem: &Option<String>) -> Element {
    let Some(msg) = problem else {
        return rsx! {};
    };
    rsx! {
        Tooltip { message: msg.clone(),
            span { class: "cat-warn", Icon { name: IconName::Warning, size: IconSize::Xs } }
        }
    }
}

fn render_table(
    mut menu: Signal<Option<CtxTarget>>,
    remove: Signal<Option<RemoveTarget>>,
    i: usize,
    filter: &str,
    selected: &Option<ColRef>,
    mut expanded: Signal<HashSet<String>>,
) -> Element {
    let store = crate::project::store();
    let tl = store.tables();
    let s = tl.read();
    let Some(t) = s.get(i) else {
        return rsx! {};
    };
    if !filter.is_empty() && !t.name.to_lowercase().contains(&filter.to_lowercase()) {
        return rsx! {};
    }
    let name = t.name.clone();
    let open = t.open;
    let parts = t.partition_cols.clone();
    // The catalog is definitions, not a mirror of DataFusion — a row can exist yet be
    // broken. Flag it (a failed register: missing file, bad path).
    let problem = crate::project::table_problem(t);
    // Scans are per-table and concurrent (D4), so the inspector's PROFILE spinner only
    // ever speaks for the *selected* column's table — start one on `orders`, click to
    // `users`, and it vanishes. The row is the only place a scan is always visible.
    // (Not from the canvas: it runs one profile at a time and force-selects the table
    // it profiled, so it can't lose one.)
    let profiling = t.profiling;
    // Flatten the (possibly nested) columns into the visible rows, expanding only
    // the struct columns whose key is in `expanded`.
    let rows: Vec<ColRow> = {
        let exp = expanded();
        let mut out = Vec::new();
        flatten_cols(&name, &[], 0, &t.columns, &parts, selected, &exp, &mut out);
        out
    };
    drop(s);

    let nm_ctx = name.clone();
    let nm_menu = name.clone();

    rsx! {
        div { style: "margin-bottom:var(--sp-1);",
            div {
                class: "tbl-row",
                onclick: move |_| dispatch(Action::ToggleTableOpen(i)),
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
                {problem_badge(&problem)}
                if profiling {
                    Tooltip { message: "Profiling...",
                        span { class: "tbl-spin ps-spin", Icon { name: IconName::Spinner, size: IconSize::Xs } }
                    }
                }
                DropdownMenu {
                    class: "row-menu", title: "Actions", align: RectAlign::BOTTOM_END, width: 180,
                    trigger: rsx! { Icon { name: IconName::Dots, size: IconSize::Sm } },
                    {catalog_menu_items(remove, CatalogKind::Table, nm_menu.clone())}
                }
            }
            if open {
                {col_rows(CatalogKind::Table, &name, rows, expanded)}
            }
        }
    }
}

/// The column rows under an open catalog entry — shared by tables and views, because a
/// view's columns are columns: clickable, selectable, and expandable if they're nested.
/// They only ever differed by omission (view rows had no `onclick` at all, so clicking
/// one silently did nothing), which is exactly what a second copy of a list buys you.
fn col_rows(
    kind: CatalogKind,
    owner: &str,
    rows: Vec<ColRow>,
    mut expanded: Signal<HashSet<String>>,
) -> Element {
    rsx! {
        div { class: "tbl-cols",
            for r in rows {
                {
                    let col = ColRef { kind, owner: owner.to_string(), path: r.path.clone() };
                    let key = r.key.clone();
                    let indent = r.depth * 12;
                    rsx! {
                        div {
                            class: if r.is_sel { "col-row sel" } else { "col-row" },
                            onclick: move |_| dispatch(Action::SelectColumn(col.clone())),
                            span { style: "width:{indent}px;flex:none;" }
                            span { class: "col-chev",
                                if r.has_children {
                                    span {
                                        style: "display:flex;cursor:pointer;",
                                        onclick: move |e| {
                                            e.stop_propagation();
                                            let mut set = expanded.write();
                                            if !set.insert(key.clone()) { set.remove(&key); }
                                        },
                                        if r.is_expanded { Icon { name: IconName::ChevronDown, size: IconSize::Xs } } else { Icon { name: IconName::ChevronRight, size: IconSize::Xs } }
                                    }
                                }
                            }
                            Dot { color: "{r.dot}", square: true, size: 6 }
                            MonoValue { class: "cname", "{r.name}" }
                            if r.is_part { Micro { class: "pill", "PART" } }
                            Meta { class: "ctype {r.tcls}", "{r.dtype}" }
                        }
                    }
                }
            }
        }
    }
}

fn render_view(
    mut menu: Signal<Option<CtxTarget>>,
    remove: Signal<Option<RemoveTarget>>,
    i: usize,
    filter: &str,
    selected: &Option<ColRef>,
    expanded: Signal<HashSet<String>>,
) -> Element {
    let store = crate::project::store();
    let vl = store.views();
    let s = vl.read();
    let Some(v) = s.get(i) else {
        return rsx! {};
    };
    if !filter.is_empty() && !v.name.to_lowercase().contains(&filter.to_lowercase()) {
        return rsx! {};
    }
    let name = v.name.clone();
    let open = v.open;
    let profiling = v.profiling;
    // Validity is derived against the *live* tables (D-validity): a hard SQL failure, or
    // a base table that's since gone missing. A separate read of the tables lens —
    // simultaneous immutable reads don't collide.
    let problem = {
        let tl = store.tables();
        let tables = tl.read();
        crate::project::view_problem(v, &tables)
    };
    // Same flatten as a table's: a view's columns can be nested too, and selecting one
    // has to carry a path for the inspector to resolve it. Views have no partition
    // columns, hence the empty `parts`.
    let rows: Vec<ColRow> = {
        let exp = expanded();
        let mut out = Vec::new();
        flatten_cols(&name, &[], 0, &v.columns, &[], selected, &exp, &mut out);
        out
    };
    drop(s);

    let nm_ctx = name.clone();
    let nm_menu = name.clone();

    rsx! {
        div { style: "margin-bottom:var(--sp-1);",
            div {
                class: "tbl-row",
                onclick: move |_| dispatch(Action::ToggleViewOpen(i)),
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
                {problem_badge(&problem)}
                if profiling {
                    Tooltip { message: "Profiling...",
                        span { class: "tbl-spin ps-spin", Icon { name: IconName::Spinner, size: IconSize::Xs } }
                    }
                }
                DropdownMenu {
                    class: "row-menu", title: "Actions", align: RectAlign::BOTTOM_END, width: 180,
                    trigger: rsx! { Icon { name: IconName::Dots, size: IconSize::Sm } },
                    {catalog_menu_items(remove, CatalogKind::View, nm_menu.clone())}
                }
            }
            if open {
                {col_rows(CatalogKind::View, &name, rows, expanded)}
            }
        }
    }
}
