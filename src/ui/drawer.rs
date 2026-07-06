//! Bottom drawer (S5) — one panel above the status bar with **History** and
//! **Events** tabs. Replaces the old event-log panel + the query-history
//! slide-over. Events are fed by `app::apply_event` via `AppState::push_log`;
//! history rows come from the project. Opened from the status bar (Events /
//! History buttons); a tab-aware Clear, expand/restore, and close in the header.

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

    let (tab, log_h, log_count, hist_count) = {
        let s = state.read();
        (s.log_tab, s.log_h, s.log.len(), s.project.history.len())
    };
    let expanded = log_h > 250.0;
    let expand_icon = if expanded {
        "M8 3v4H4M16 3v4h4M8 21v-4H4M16 21v-4h4"
    } else {
        "M4 8V4h4M20 8V4h-4M4 16v4h4M20 16v4h-4"
    };

    // Full events (cloned) so error rows can carry their structured error and
    // expand in place (S6). `event_colors` is applied per-row at render.
    let events: Vec<LogEvent> = state.read().log.clone();
    let history: Vec<(u64, String, String, u128, usize, &'static str, &'static str)> = state
        .read()
        .project
        .history
        .iter()
        .map(|h| {
            let dot = if h.ok { "var(--green)" } else { "var(--red2)" };
            let meta = if h.ok { "var(--dim)" } else { "var(--red)" };
            (
                h.id,
                h.sql.clone(),
                h.ts_label.clone(),
                h.ms,
                h.rows,
                dot,
                meta,
            )
        })
        .collect();

    let hist_tab_cls = if tab == LogTab::History {
        "drawer-tab on"
    } else {
        "drawer-tab"
    };
    let evt_tab_cls = if tab == LogTab::Events {
        "drawer-tab on"
    } else {
        "drawer-tab"
    };

    rsx! {
        div { class: "ps-log", style: "height:{log_h}px;",
            {crate::action::panel::resize_handle(state, crate::state::ResizeTarget::Log)}
            div { class: "ps-log-head",
                button { class: "{hist_tab_cls}", onclick: move |_| dispatch(state, Action::SetLogTab(LogTab::History)),
                    {icons::clock(13)}
                    span { "History" }
                    span { class: "drawer-count", "{hist_count}" }
                }
                button { class: "{evt_tab_cls}", onclick: move |_| dispatch(state, Action::SetLogTab(LogTab::Events)),
                    {icons::format(13)}
                    span { "Events" }
                    span { class: "drawer-count", "{log_count}" }
                }
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
            if tab == LogTab::History {
                div { class: "ps-log-body ps-scroll", style: "padding:8px;",
                    if history.is_empty() {
                        div { class: "ps-log-empty", "No queries run yet" }
                    }
                    for (id, sql, ts, ms, rows, dot, meta) in history {
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
                                        div { class: "spacer" }
                                        span { class: "mono", style: "font-size:10px;color:var(--faint);", "{ts}" }
                                    }
                                    div { class: "hist-sql", "{sql}" }
                                }
                            }
                        }
                    }
                }
            } else {
                div { class: "ps-log-body ps-scroll",
                    if events.is_empty() {
                        div { class: "ps-log-empty", "No events yet" }
                    }
                    for e in events {
                        {
                            let (dot, fg) = event_colors(e.kind);
                            let LogEvent { id, msg, ts, err, open, .. } = e;
                            let expandable = err.is_some();
                            // Down-chevron when open, right-chevron when collapsed.
                            let chevron = if open { "M6 9l6 6 6-6" } else { "M9 6l6 6-6 6" };
                            let row_cls = if expandable { "ps-log-row expandable" } else { "ps-log-row" };
                            rsx! {
                                div { key: "e{id}", class: "evt-item",
                                    div {
                                        class: "{row_cls}",
                                        onclick: move |_| { if expandable { dispatch(state, Action::ToggleLogRow(id)); } },
                                        span { class: "dot", style: "background:{dot};" }
                                        span { class: "msg", style: "color:{fg};", "{msg}" }
                                        if expandable {
                                            svg {
                                                class: "evt-chev", width: "13", height: "13", "viewBox": "0 0 24 24",
                                                fill: "none", stroke: "currentColor", "stroke-width": "2",
                                                "stroke-linecap": "round", "stroke-linejoin": "round",
                                                path { d: "{chevron}" }
                                            }
                                        }
                                        span { class: "ts", "{ts}" }
                                    }
                                    {
                                        match err {
                                            Some(err) if open => rsx! {
                                                div { class: "evt-detail",
                                                    div { class: "evt-errbox",
                                                        {crate::ui::errview::error_detail(&err)}
                                                    }
                                                }
                                            },
                                            _ => rsx! {},
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
