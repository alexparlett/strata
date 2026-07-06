//! Center pane: query tabs, SQL editor, and the results area. The results area
//! is a four-way switch (S6): a spinner while `running`, the structured error
//! view when the last query failed (`query_error`), the grid + search/export
//! toolbar + pager when a `result` is loaded, or a "no results yet" empty state.
//!
//! `state` is read once in `Workspace` and threaded into the render helpers, so
//! no hooks are called inside loops/branches (Dioxus rules of hooks).

use dioxus::prelude::*;
use dioxus_code::{Code, SourceCode};
use dioxus_code_editor::CodeEditor;

use crate::action::{dispatch, Action};
use crate::engine::Cell;
use crate::state::AppState;
use crate::ui::icons;
use crate::ui::components::{Dialog, MenuItem, MenuSep, Point, Popup};

/// A nested-cell view target (struct/list/map cell), shown in a `Dialog`.
#[derive(Clone)]
struct CellView {
    name: String,
    type_label: String,
    json: String,
}

#[component]
pub fn Workspace() -> Element {
    let state = use_context::<Signal<AppState>>();
    // Self-contained: the tab context menu lives here, not in `AppState`.
    let tab_menu = use_signal(|| None::<(usize, Point)>);
    // The nested-cell view is likewise workspace-local, opened from a grid cell.
    let cell_view = use_signal(|| None::<CellView>);
    let has_ws = !state.read().project.workspaces.is_empty();
    rsx! {
        main { class: "ps-main",
            {tabs(state, tab_menu)}
            if has_ws {
                {editor(state)}
                {crate::action::panel::resize_handle(state, crate::state::ResizeTarget::Editor)}
                {results_area(state, cell_view)}
            } else {
                {empty_state(state)}
            }
            if let Some(c) = cell_view() {
                {cell_dialog(cell_view, c)}
            }
        }
    }
}

/// The results area is one of four mutually-exclusive states: a spinner while a
/// query runs, the structured error view, the grid (with its search/export
/// toolbar + pager), or the "no results yet" empty state. The toolbar and pager
/// only appear alongside a grid.
fn results_area(state: Signal<AppState>, cell_view: Signal<Option<CellView>>) -> Element {
    let (running, has_err, has_plan, has_result) = {
        let s = state.read();
        (
            s.running,
            s.query_error.is_some(),
            s.plan.is_some(),
            s.result.is_some(),
        )
    };
    if running {
        rsx! { {results_running(state)} }
    } else if has_err {
        rsx! { {results_error(state)} }
    } else if has_plan {
        rsx! { {results_plan(state)} }
    } else if has_result {
        rsx! {
            {results_toolbar(state)}
            {results_grid(state, cell_view)}
            {pager(state)}
        }
    } else {
        rsx! { {results_empty(state)} }
    }
}

/// EXPLAIN plan view (S12): a toolbar (Physical/Logical tabs, summary, ANALYZE
/// badge, Raw/Tree toggle) over an indented tree of operator cards — or the raw
/// plan text. ANALYZE forces the physical "Plan with Metrics" and adds per-node
/// rows/time, a time-share bar, and a HOTSPOT badge for the slowest operators.
fn results_plan(state: Signal<AppState>) -> Element {
    use crate::state::PlanTab;
    let (plan, tab, raw) = {
        let s = state.read();
        let Some(plan) = s.plan.clone() else {
            return rsx! { div {} };
        };
        (plan, s.plan_tab, s.plan_raw)
    };

    let analyze = plan.analyze;
    let has_logical = !plan.logical.is_empty();
    let has_physical = !plan.physical.is_empty();
    // Honour the selected tab, falling back to whichever tree is present. ANALYZE
    // defaults to physical (the metrics plan) but the logical tab stays available.
    let eff_physical = if !has_physical {
        false
    } else if !has_logical {
        true
    } else {
        tab == PlanTab::Physical
    };
    // Offer the Physical/Logical switch whenever both trees exist — incl. ANALYZE.
    let show_tabs = has_logical && has_physical;
    let nodes = if eff_physical { &plan.physical } else { &plan.logical };
    let raw_text = if eff_physical { &plan.physical_text } else { &plan.logical_text };
    let max_ms = plan.max_ms();

    // Summary reflects the *active* tab: the logical tab never shows metrics, even
    // during an ANALYZE run.
    let summary = format!(
        "{} · {} operators",
        if !eff_physical {
            "Logical plan"
        } else if analyze {
            "Plan with metrics"
        } else {
            "Physical plan"
        },
        nodes.len()
    );

    let phys_cls = if eff_physical { "plan-tab on" } else { "plan-tab" };
    let log_cls = if !eff_physical { "plan-tab on" } else { "plan-tab" };
    let raw_label = if raw { "Tree" } else { "Raw" };

    rsx! {
        div { class: "res-plan",
            div { class: "plan-tb",
                if show_tabs {
                    div { class: "plan-tabs",
                        button { class: "{phys_cls}", onclick: move |_| dispatch(state, Action::SetPlanTab(PlanTab::Physical)), "Physical" }
                        button { class: "{log_cls}", onclick: move |_| dispatch(state, Action::SetPlanTab(PlanTab::Logical)), "Logical" }
                    }
                }
                span { class: "plan-summary mono", "{summary}" }
                if analyze && eff_physical {
                    span { class: "plan-analyze mono", "ANALYZE" }
                }
                div { class: "spacer" }
                button { class: "btn sm", style: "height:28px;", onclick: move |_| dispatch(state, Action::TogglePlanRaw),
                    svg { width: "13", height: "13", "viewBox": "0 0 24 24", fill: "none",
                        stroke: "currentColor", "stroke-width": "1.8", "stroke-linecap": "round", "stroke-linejoin": "round",
                        path { d: "M4 7h16M4 12h10M4 17h13" }
                    }
                    "{raw_label}"
                }
            }
            if raw {
                div { class: "plan-body ps-scroll",
                    pre { class: "plan-raw mono", "{raw_text}" }
                }
            } else {
                div { class: "plan-body ps-scroll",
                    for (i, n) in nodes.iter().enumerate() {
                        {plan_node_card(n, i, analyze, max_ms)}
                    }
                }
            }
        }
    }
}

/// One operator card in the plan tree, indented by depth and coloured by kind.
fn plan_node_card(
    n: &crate::plan::PlanNode,
    idx: usize,
    analyze: bool,
    max_ms: f64,
) -> Element {
    let color = n.kind.color();
    let indent = n.depth * 22;
    let has_metrics = analyze && n.ms_val.is_some();
    let ms = n.ms_val.unwrap_or(0.0);
    let hot = analyze && ms >= max_ms * 0.6;
    let bar_pct = if has_metrics {
        ((ms / max_ms) * 100.0).round().max(3.0)
    } else {
        0.0
    };
    let rows_label = n.rows.map(fmt_int).unwrap_or_default();

    rsx! {
        div { key: "p{idx}", class: "plan-row", style: "padding-left:{indent}px;",
            div { class: "plan-card", style: "border-left-color:{color};",
                div { class: "plan-card-head",
                    span { class: "plan-sq", style: "background:{color};" }
                    span { class: "plan-name mono", style: "color:{color};", "{n.name}" }
                    if hot {
                        span { class: "plan-hot mono", "HOTSPOT" }
                    }
                }
                if !n.detail.is_empty() {
                    div { class: "plan-detail mono", "{n.detail}" }
                }
                if has_metrics {
                    div { class: "plan-metrics",
                        div { class: "plan-metrics-row",
                            span { class: "plan-rows mono", "{rows_label} rows" }
                            span { class: "plan-ms mono", "{n.ms_label}" }
                            if !n.extra.is_empty() {
                                span { class: "plan-extra mono", "{n.extra}" }
                            }
                        }
                        div { class: "plan-bar",
                            div { class: "plan-bar-fill", style: "width:{bar_pct}%;background:{color};" }
                        }
                    }
                }
            }
        }
    }
}

/// Group a non-negative integer with thin thousands separators (e.g. 48213 →
/// "48,213") for the plan's row counts.
fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Results area while a query is in flight — a centred spinner. (Cancel is S14.)
fn results_running(state: Signal<AppState>) -> Element {
    let target = {
        let s = state.read();
        s.project
            .workspaces
            .get(s.project.active_ws)
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "sources".into())
    };
    rsx! {
        div { class: "res-state res-running",
            {icons::spinner(30)}
            div { class: "res-title", "Running query…" }
            div { class: "res-sub mono", "scanning {target}" }
        }
    }
}

/// Results area for the last failed query — an error banner, the message, an
/// optional code frame with a caret, and an optional hint. Dismiss clears it.
fn results_error(state: Signal<AppState>) -> Element {
    let Some(err) = state.read().query_error.clone() else {
        return rsx! { div {} };
    };
    let loc = err.loc.clone().unwrap_or_default();
    rsx! {
        div { class: "res-error ps-scroll",
            div { class: "err-banner",
                span { class: "err-ico", {icons::err_circle(15)} }
                span { class: "err-type", "{err.etype}" }
                if !loc.is_empty() {
                    span { class: "err-loc", "{loc}" }
                }
                div { class: "spacer" }
                button { class: "err-dismiss", title: "Dismiss",
                    onclick: move |_| dispatch(state, Action::DismissQueryError),
                    svg { width: "12", height: "12", "viewBox": "0 0 24 24", fill: "none",
                        stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round",
                        path { d: "M6 6l12 12M18 6L6 18" }
                    }
                }
            }
            div { class: "err-body",
                {crate::ui::errview::error_detail(&err)}
            }
        }
    }
}

/// Results area before the active tab has produced any rows. An unrun EXPLAIN
/// gets a plan-specific hint.
fn results_empty(state: Signal<AppState>) -> Element {
    let is_explain = crate::plan::is_explain(&state.read().active_sql());
    let (title, sub) = if is_explain {
        ("No plan yet", "Run the EXPLAIN to see the query plan.")
    } else {
        (
            "No results yet",
            "Run the query to load rows from your sources into the grid.",
        )
    };
    rsx! {
        div { class: "res-state res-empty",
            div { class: "res-empty-ico", {icons::rows(22)} }
            div { class: "res-title", "{title}" }
            div { class: "res-sub", "{sub}" }
        }
    }
}

/// Center-pane placeholder shown when no query tab is open (all tabs closed).
fn empty_state(state: Signal<AppState>) -> Element {
    let has_closed = !state.read().closed_tabs.is_empty();
    let saved: Vec<String> = state
        .read()
        .project.saved_queries
        .iter()
        .take(4)
        .map(|q| q.name.clone())
        .collect();
    rsx! {
        div { class: "ws-empty",
            div { class: "ws-empty-ico", {icons::database(26)} }
            div { class: "ws-empty-title", "No query open" }
            div { class: "ws-empty-sub",
                "Open a new query tab to explore your data, or run "
                span { class: "mono hl", "SELECT *" }
                " on a table from the catalog."
            }
            div { class: "ws-empty-actions",
                button { class: "btn accent", style: "height:36px;",
                    onclick: move |_| dispatch(state, Action::NewTab),
                    {icons::plus(15)}
                    "New query"
                    span { class: "kbd", style: "margin-left:2px;", "⌘N" }
                }
                if has_closed {
                    button { class: "btn", style: "height:36px;",
                        onclick: move |_| dispatch(state, Action::ReopenTab),
                        {icons::reopen(14)}
                        "Reopen closed"
                    }
                }
            }
            if !saved.is_empty() {
                div { class: "ws-empty-saved",
                    div { class: "lbl", "SAVED QUERIES" }
                    for name in saved {
                        {
                            let nm = name.clone();
                            rsx! {
                                div { class: "ws-empty-q",
                                    onclick: move |_| dispatch(state, Action::OpenSavedQuery(nm.clone())),
                                    span { style: "color:var(--purple);display:flex;flex:none;", {icons::brackets(14)} }
                                    span { class: "nm", "{name}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn tabs(state: Signal<AppState>, mut tab_menu: Signal<Option<(usize, Point)>>) -> Element {
    let sidebar_open = state.read().sidebar_open;
    let active = state.read().project.active_ws;
    let renaming = state.read().renaming_ws;
    let rename_val = state.read().rename_val.clone();
    let ws: Vec<(usize, String)> = state
        .read()
        .project.workspaces
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
                    let is_rename = renaming == Some(i);
                    let rv = rename_val.clone();
                    rsx! {
                        div {
                            class: if i == active { "ws-tab active" } else { "ws-tab" },
                            onclick: move |_| dispatch(state, Action::SwitchTab(i)),
                            ondoubleclick: move |e| { e.stop_propagation(); dispatch(state, Action::StartRename(i)); },
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
                                    oninput: move |e| dispatch(state, Action::RenameInput(e.value())),
                                    onkeydown: move |e| match e.key() {
                                        Key::Enter => { e.prevent_default(); dispatch(state, Action::CommitRename); }
                                        Key::Escape => { e.prevent_default(); dispatch(state, Action::CancelRename); }
                                        _ => {}
                                    },
                                    onblur: move |_| dispatch(state, Action::CommitRename),
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
                    {tab_menu_items(state, tab_menu, idx)}
                }
            }
        }
    }
}

/// Rows for a workspace-tab context menu. Each dismisses the popup then dispatches.
fn tab_menu_items(
    state: Signal<AppState>,
    mut tab_menu: Signal<Option<(usize, Point)>>,
    idx: usize,
) -> Element {
    let can_reopen = !state.read().closed_tabs.is_empty();
    rsx! {
        MenuItem { label: "Rename".to_string(),
            onclick: move |_| { tab_menu.set(None); dispatch(state, Action::StartRename(idx)); } }
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

fn editor(mut state: Signal<AppState>) -> Element {
    let sql = state.read().active_sql();
    let editor_h = state.read().editor_h;
    let (ln, col, ws_id, epoch) = {
        let s = state.read();
        let ws_id = s
            .project
            .workspaces
            .get(s.project.active_ws)
            .map(|w| w.id)
            .unwrap_or(0);
        (s.caret_line, s.caret_col, ws_id, s.editor_epoch)
    };

    rsx! {
        section { style: "flex:none;background:var(--main);",
            div { style: "height:{editor_h}px;background:var(--main);border-bottom:1px solid var(--line);overflow:auto;",
                CodeEditor {
                    // Remount when the active content changes for a non-typing
                    // reason: the tab id covers switches, the epoch covers
                    // programmatic edits (Format/Clear/load). The editor seeds its
                    // textarea from `value` only on mount, so this re-seeds it.
                    key: "{ws_id}-{epoch}",
                    value: sql.clone(),
                    language: crate::ui::lang("sql"),
                    theme: crate::ui::code_theme(),
                    line_numbers: true,
                    spellcheck: false,
                    placeholder: "SELECT * FROM your_table LIMIT 100;",
                    class: "ps-sql",
                    oninput: move |v: String| state.write().set_active_sql(v),
                }
            }
        }
    }
}

fn results_toolbar(state: Signal<AppState>) -> Element {
    let q = state.read().result_search.clone();
    rsx! {
        div { class: "results-tb",
            div { class: "field", style: "width:320px;max-width:46%;",
                {icons::search(14)}
                input { class: "input mono", placeholder: "Find in results", value: "{q}",
                    oninput: move |e| dispatch(state, Action::SetResultSearch(e.value())) }
            }
            div { class: "spacer" }
            button { class: "btn", style: "height:28px;", onclick: move |_| crate::overlays::open_export(),
                {icons::download(13)} "Export" }
        }
    }
}

fn results_grid(state: Signal<AppState>, cell_view: Signal<Option<CellView>>) -> Element {
    let s = state.read();
    let zebra = s.zebra;
    let type_color = s.type_color_cells;
    let page = s.page;
    let page_size = s.page_size;
    let search = s.result_search.to_lowercase();
    let Some(result) = s.result.clone() else {
        return results_empty(state);
    };
    drop(s);

    // (name, type, type-text-class, cell-class, nested)
    let cols: Vec<(String, String, &'static str, &'static str, bool)> = result
        .columns
        .iter()
        .map(|c| (c.name.clone(), c.dtype.clone(), c.kind.text_class(), c.kind.cell_class(), c.kind.is_nested()))
        .collect();

    // `result.rows` is already the current page (server-side snapshot). Number
    // by global position; the find-box filters within the visible page.
    let base = page.saturating_sub(1) * page_size;
    let rows_page: Vec<(usize, Vec<Cell>)> = result
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| search.is_empty() || r.iter().any(|c| c.text.to_lowercase().contains(&search)))
        .map(|(i, r)| (base + i + 1, r.clone()))
        .collect();

    rsx! {
        div { class: "grid-scroll ps-scroll",
            div { class: "grid-inner",
                div { class: "grid-head",
                    div { class: "hnum", "#" }
                    for (cn, ct, tcls, _cc, _nested) in cols.iter().cloned() {
                        div { class: "hcol", style: "width:150px;",
                            span { class: "cn", "{cn}" }
                            span { class: "ct {tcls}", "{ct}" }
                        }
                    }
                }
                for (rownum, cells) in rows_page {
                    div { class: if zebra && rownum % 2 == 0 { "grid-row zebra" } else { "grid-row" },
                        div { class: "rnum", "{rownum}" }
                        for (ci, cell) in cells.iter().enumerate() {
                            {render_cell(cols.get(ci).cloned(), cell.clone(), cell_view, type_color)}
                        }
                    }
                }
            }
        }
    }
}

fn render_cell(
    col: Option<(String, String, &'static str, &'static str, bool)>,
    cell: Cell,
    mut cell_view: Signal<Option<CellView>>,
    type_color: bool,
) -> Element {
    let (name, ty, cell_cls, nested) = match col {
        Some((n, t, _tc, cc, nested)) => (n, t, cc, nested),
        None => (String::new(), String::new(), "", false),
    };
    let mut class = String::from("cell");
    if cell.null {
        class.push_str(" null");
    } else if type_color && !cell_cls.is_empty() {
        class.push(' ');
        class.push_str(cell_cls);
    }
    let text = cell.text.clone();

    rsx! {
        div {
            class: "{class}",
            style: "width:150px;",
            onclick: move |_| {
                if nested {
                    cell_view.set(Some(CellView {
                        name: name.clone(),
                        type_label: ty.clone(),
                        json: text.clone(),
                    }));
                }
            },
            "{cell.text}"
        }
    }
}

/// The nested-cell JSON view (struct/list/map cell) — a workspace-local `Dialog`
/// with a static highlighted `Code` body. The `cell_view` signal owns open/close.
fn cell_dialog(mut cell_view: Signal<Option<CellView>>, c: CellView) -> Element {
    rsx! {
        Dialog { on_close: move |_| cell_view.set(None), card_class: "modal cell-modal".to_string(), z: 64,
            div { class: "row", style: "gap:10px;padding:13px 16px;border-bottom:1px solid var(--line);",
                span { class: "mono", style: "font-weight:600;font-size:13px;", "{c.name}" }
                span { class: "mono", style: "font-size:10px;color:var(--t-list);background:var(--accent-soft);padding:2px 7px;border-radius:5px;", "{c.type_label}" }
                div { class: "spacer" }
                button { class: "icon-btn plain", style: "width:28px;height:28px;", onclick: move |_| cell_view.set(None), {icons::close(13)} }
            }
            div { style: "overflow:auto;max-height:70vh;",
                Code {
                    src: SourceCode::new(crate::ui::lang("json"), c.json.clone()),
                    theme: crate::ui::code_theme(),
                }
            }
        }
    }
}

fn pager(state: Signal<AppState>) -> Element {
    let (total, elapsed, page, page_size, page_size_open) = {
        let s = state.read();
        (
            s.result.as_ref().map(|r| r.total).unwrap_or(0),
            s.result.as_ref().map(|r| r.elapsed_ms).unwrap_or(0),
            s.page,
            s.page_size,
            s.page_size_open,
        )
    };
    let page_count = ((total as f64) / (page_size as f64)).ceil().max(1.0) as usize;

    rsx! {
        div { class: "pager",
            span { style: "width:7px;height:7px;border-radius:50%;background:var(--green);box-shadow:0 0 6px var(--green);" }
            span { class: "rows", "{total} rows" }
            span { class: "meta", "{elapsed} ms" }
            div { class: "spacer" }
            div { style: "position:relative;",
                button { class: "btn sm", style: "height:26px;",
                    onclick: move |_| dispatch(state, Action::TogglePageSizeMenu),
                    "{page_size} / page" {icons::chevron_down(11)}
                }
                if page_size_open {
                    div { class: "menu", style: "position:absolute;bottom:32px;right:0;width:120px;z-index:6;",
                        for sz in [50usize, 100, 500, 1000] {
                            button { class: "menu-item mono",
                                onclick: move |_| dispatch(state, Action::SetPageSize(sz)),
                                "{sz} / page" }
                        }
                    }
                }
            }
            div { style: "width:1px;height:18px;background:var(--line);" }
            div { class: "row", style: "gap:3px;",
                button { class: "pg-btn", title: "First", onclick: move |_| dispatch(state, Action::FetchPage(1)), {icons::first(15)} }
                button { class: "pg-btn", title: "Previous", onclick: move |_| { if page > 1 { dispatch(state, Action::FetchPage(page - 1)); } }, {icons::prev(15)} }
                div { class: "row", style: "gap:6px;padding:0 6px;",
                    input { class: "page-input", value: "{page}",
                        onchange: move |e| { if let Ok(p) = e.value().parse::<usize>() { dispatch(state, Action::FetchPage(p.clamp(1, page_count))); } } }
                    span { class: "meta", "of {page_count}" }
                }
                button { class: "pg-btn", title: "Next", onclick: move |_| { if page < page_count { dispatch(state, Action::FetchPage(page + 1)); } }, {icons::next(15)} }
                button { class: "pg-btn", title: "Last", onclick: move |_| dispatch(state, Action::FetchPage(page_count)), {icons::last(15)} }
            }
        }
    }
}
