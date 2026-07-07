//! Bottom drawer (S5 + S23) — one panel showing **History / Events / Problems**,
//! chosen by the activity rail (no in-drawer tab strip). Header is just
//! `title · count` + Clear / expand / close. Events is a flat log; Problems is the
//! `error`-kind subset **grouped by owning query tab** (click a row → switch to it);
//! history rows come from the project. Drag the top edge to resize.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, LogEvent, LogKind, LogTab};
use crate::ui::icons;

/// Colour for an event row's dot + message, keyed by severity.
fn event_colors(kind: LogKind) -> (&'static str, &'static str) {
    match kind {
        LogKind::Ok => ("var(--green)", "var(--text2)"),
        LogKind::Info => ("var(--accent)", "var(--text2)"),
        LogKind::Run => ("var(--accent)", "var(--text2)"),
        LogKind::Warn => ("var(--orange)", "var(--text2)"),
        LogKind::Error => ("var(--red2)", "var(--red)"),
    }
}

#[component]
pub fn Drawer() -> Element {
    let state = use_context::<Signal<AppState>>();
    let (tab, log_h) = {
        let s = state.read();
        (s.log_tab, s.log_h)
    };
    let expanded = log_h > 250.0;
    let expand_icon = if expanded {
        "M8 3v4H4M16 3v4h4M8 21v-4H4M16 21v-4h4"
    } else {
        "M4 8V4h4M20 8V4h-4M4 16v4h4M20 16v4h-4"
    };
    let (title, count): (&str, usize) = {
        let s = state.read();
        match tab {
            LogTab::History => ("History", s.project.history.len()),
            LogTab::Events => ("Events", s.log.len()),
            LogTab::Problems => (
                "Problems",
                s.log.iter().filter(|e| e.kind == LogKind::Error).count(),
            ),
        }
    };

    rsx! {
        div { class: "ps-log", style: "height:{log_h}px;",
            {crate::action::panel::resize_handle(state, crate::state::ResizeTarget::Log)}
            div { class: "ps-log-head",
                span { class: "title", "{title}" }
                span { class: "count", "{count}" }
                div { class: "spacer" }
                button { class: "txtbtn", onclick: move |_| dispatch(state, Action::ClearDrawer), "Clear" }
                button { class: "iconbtn", title: "Expand / collapse", onclick: move |_| dispatch(state, Action::ToggleLogHeight),
                    svg {
                        width: "14", height: "14", "viewBox": "0 0 24 24", fill: "none",
                        stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round", "stroke-linejoin": "round",
                        path { d: "{expand_icon}" }
                    }
                }
                button { class: "iconbtn", title: "Close", onclick: move |_| dispatch(state, Action::ToggleLog),
                    svg {
                        width: "13", height: "13", "viewBox": "0 0 24 24", fill: "none",
                        stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round",
                        path { d: "M6 6l12 12M18 6L6 18" }
                    }
                }
            }
            {
                match tab {
                    LogTab::History => history_body(state),
                    LogTab::Events => events_body(state),
                    LogTab::Problems => problems_body(state),
                }
            }
        }
    }
}

/// History tab: one row per past run (single-line preview + a `N lines` chip for
/// multi-line SQL). Click loads, double-click loads & runs.
fn history_body(state: Signal<AppState>) -> Element {
    let history: Vec<(u64, String, String, u128, usize, usize, &'static str, &'static str)> = state
        .read()
        .project
        .history
        .iter()
        .map(|h| {
            let dot = if h.ok { "var(--green)" } else { "var(--red2)" };
            let meta = if h.ok { "var(--dim)" } else { "var(--red)" };
            let lines = h.sql.lines().count();
            (h.id, h.sql.clone(), h.ts_label.clone(), h.ms, h.rows, lines, dot, meta)
        })
        .collect();
    rsx! {
        div { class: "ps-log-body ps-scroll", style: "padding:8px;",
            if history.is_empty() {
                div { class: "ps-log-empty", "No queries run yet" }
            }
            for (id, sql, ts, ms, rows, lines, dot, meta) in history {
                {
                    let sql_load = sql.clone();
                    let sql_run = sql.clone();
                    rsx! {
                        div {
                            key: "h{id}",
                            class: "hist-item",
                            title: "Click to load · double-click to load & run",
                            onclick: move |_| dispatch(state, Action::OpenHistoryQuery(sql_load.clone())),
                            ondoubleclick: move |_| dispatch(state, Action::RunHistoryQuery(sql_run.clone())),
                            div { class: "row", style: "gap:8px;margin-bottom:6px;",
                                span { style: "width:6px;height:6px;border-radius:50%;flex:none;background:{dot};" }
                                span { class: "mono", style: "font-size:10px;color:{meta};", "{rows} rows · {ms} ms" }
                                if lines > 1 {
                                    span { class: "hist-lines", "{lines} lines" }
                                }
                                div { class: "spacer" }
                                span { class: "mono", style: "font-size:10px;color:var(--faint);", "{ts}" }
                            }
                            div { class: "hist-sql", "{sql}" }
                        }
                    }
                }
            }
        }
    }
}

/// Events tab: a flat log (dot + message + timestamp). No expand — the rich
/// diagnostics live in Problems (S23).
fn events_body(state: Signal<AppState>) -> Element {
    let events: Vec<LogEvent> = state.read().log.clone();
    rsx! {
        div { class: "ps-log-body ps-scroll",
            if events.is_empty() {
                div { class: "ps-log-empty", "No events yet" }
            }
            for e in events {
                {
                    let (dot, fg) = event_colors(e.kind);
                    rsx! {
                        div { key: "e{e.id}", class: "evt-item",
                            div { class: "ps-log-row",
                                span { class: "dot", style: "background:{dot};" }
                                span { class: "msg", style: "color:{fg};", "{e.msg}" }
                                span { class: "ts", "{e.ts}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Problems tab: `error`-kind events grouped by owning query tab (sticky headers),
/// flat one-liner rows with a `line:col` ref; click a row → switch to its tab.
fn problems_body(state: Signal<AppState>) -> Element {
    let snap = crate::session::snapshot();
    let name_of = |ws: Option<u64>| -> String {
        match ws {
            Some(id) => snap
                .workspaces
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.name.clone())
                .unwrap_or_else(|| "closed tab".into()),
            None => "General".into(),
        }
    };
    let errors: Vec<(u64, String, Option<String>, Option<u64>)> = state
        .read()
        .log
        .iter()
        .filter(|e| e.kind == LogKind::Error)
        .map(|e| {
            (
                e.id,
                e.msg.clone(),
                e.err.as_ref().and_then(|q| q.loc.clone()),
                e.ws,
            )
        })
        .collect();

    if errors.is_empty() {
        return rsx! {
            div { class: "prob-empty",
                {icons::check(26)}
                div { "No problems — queries are clean" }
            }
        };
    }

    // Group by owning tab, first-seen order.
    let mut groups: Vec<(Option<u64>, Vec<(u64, String, Option<String>)>)> = Vec::new();
    for (id, msg, loc, ws) in errors {
        if let Some(g) = groups.iter_mut().find(|(gws, _)| *gws == ws) {
            g.1.push((id, msg, loc));
        } else {
            groups.push((ws, vec![(id, msg, loc)]));
        }
    }

    rsx! {
        div { class: "ps-log-body ps-scroll",
            for (ws, rows) in groups {
                {
                    let gname = name_of(ws);
                    let n = rows.len();
                    let gcount = format!("{n} problem{}", if n == 1 { "" } else { "s" });
                    rsx! {
                        div { class: "prob-group",
                            {icons::file(14)}
                            span { class: "prob-gname", "{gname}" }
                            span { class: "prob-gcount", "{gcount}" }
                        }
                        for (id, msg, loc) in rows {
                            div {
                                key: "p{id}",
                                class: "prob-row",
                                title: "Go to source",
                                onclick: move |_| { if let Some(w) = ws { dispatch(state, Action::SwitchTab(w)); } },
                                {icons::problems(15)}
                                span { class: "prob-msg", "{msg}" }
                                if let Some(l) = loc {
                                    span { class: "prob-loc", "{l}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
