//! The results area: a four-way state switch (running spinner / structured error /
//! EXPLAIN plan / grid) or the "no results yet" placeholder, plus the results
//! toolbar, the pager, and the no-tabs center-pane empty state. Each is its own
//! context-component; the toolbar and pager only appear alongside a grid.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::AppState;
use crate::ui::icons;

/// The results area is one of four mutually-exclusive states: a spinner while a
/// query runs, the structured error view, the grid (with its search/export
/// toolbar + pager), or the "no results yet" empty state.
#[component]
pub(crate) fn Results() -> Element {
    let state = use_context::<Signal<AppState>>();
    let (running, has_err, has_plan, has_result) = {
        let id = state.read().active_tab_id();
        let runs = crate::runs::RUNS.read();
        id.and_then(|id| runs.get(&id))
            .map(|r| {
                (
                    r.running,
                    r.query_error.is_some(),
                    r.plan.is_some(),
                    r.result.is_some(),
                )
            })
            .unwrap_or((false, false, false, false))
    };
    if running {
        rsx! { Running {} }
    } else if has_err {
        rsx! { ErrorView {} }
    } else if has_plan {
        rsx! { super::plan_view::PlanView {} }
    } else if has_result {
        rsx! {
            ResultsToolbar {}
            super::grid::ResultsGrid {}
            Pager {}
        }
    } else {
        rsx! { Empty {} }
    }
}

/// Results area while a query is in flight — a centred spinner. (Cancel is S14.)
#[component]
fn Running() -> Element {
    let state = use_context::<Signal<AppState>>();
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
#[component]
fn ErrorView() -> Element {
    let state = use_context::<Signal<AppState>>();
    let err = {
        let id = state.read().active_tab_id();
        let runs = crate::runs::RUNS.read();
        id.and_then(|id| runs.get(&id))
            .and_then(|r| r.query_error.clone())
    };
    let Some(err) = err else {
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

/// Results area before the active tab has produced any rows. An unrun EXPLAIN gets
/// a plan-specific hint. Also the grid's defensive fallback (see `grid`).
#[component]
pub(crate) fn Empty() -> Element {
    let state = use_context::<Signal<AppState>>();
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
#[component]
pub(crate) fn EmptyState() -> Element {
    let state = use_context::<Signal<AppState>>();
    let has_closed = !state.read().closed_tabs.is_empty();
    let saved: Vec<String> = state
        .read()
        .project
        .saved_queries
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

#[component]
fn ResultsToolbar() -> Element {
    let state = use_context::<Signal<AppState>>();
    let q = {
        let id = state.read().active_tab_id();
        let runs = crate::runs::RUNS.read();
        id.and_then(|id| runs.get(&id))
            .map(|r| r.result_search.clone())
            .unwrap_or_default()
    };
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

#[component]
fn Pager() -> Element {
    let state = use_context::<Signal<AppState>>();
    let (total, elapsed, page, page_size, page_size_open) = {
        let (id, page_size_open) = {
            let s = state.read();
            (s.active_tab_id(), s.page_size_open)
        };
        let runs = crate::runs::RUNS.read();
        let run = id.and_then(|id| runs.get(&id));
        (
            run.and_then(|r| r.result.as_ref())
                .map(|r| r.total)
                .unwrap_or(0),
            run.and_then(|r| r.result.as_ref())
                .map(|r| r.elapsed_ms)
                .unwrap_or(0),
            run.map(|r| r.page).unwrap_or(1),
            run.map(|r| r.page_size).unwrap_or(100),
            page_size_open,
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
