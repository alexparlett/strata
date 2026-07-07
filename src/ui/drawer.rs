//! Bottom drawer (S5 + S23) — one panel showing **History / Events / Problems**,
//! chosen by the activity rail (no in-drawer tab strip). Header is just
//! `title · count` + Clear (History/Events only) / expand / close. Events is a flat
//! log; **Problems reads
//! live per-tab diagnostics from `crate::diagnostics`** (validation ∪ execution),
//! grouped by owning tab (click a row → switch to it); history rows come from the
//! project. Drag the top edge to resize.

use dioxus::prelude::*;
// `.iter()` over the session store's workspaces collection.
use dioxus_stores::*;

use crate::action::{dispatch, Action};
// Lens accessors (`.workspaces()`, `.id()`, `.name()`) for the Problems grouping.
use crate::session::{SessionStoreExt, WorkspaceStoreExt};
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
    let (title, count): (&str, usize) = match tab {
        LogTab::History => ("History", state.read().project.history.len()),
        LogTab::Events => ("Events", state.read().log.len()),
        // Problems counts live error diagnostics (validation ∪ execution), not log rows.
        LogTab::Problems => ("Problems", crate::diagnostics::total_errors()),
    };

    rsx! {
        div { class: "ps-log", style: "height:{log_h}px;",
            {crate::action::panel::resize_handle(state, crate::state::ResizeTarget::Log)}
            div { class: "ps-log-head",
                span { class: "title", "{title}" }
                span { class: "count", "{count}" }
                div { class: "spacer" }
                // No Clear on Problems — they're live diagnostics that clear
                // themselves when the SQL is fixed (or the query re-run).
                if tab != LogTab::Problems {
                    button { class: "txtbtn", onclick: move |_| dispatch(state, Action::ClearDrawer), "Clear" }
                }
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

/// Problems tab: each tab's live diagnostics (validation + execution) grouped
/// under a sticky per-tab header, flat one-liner rows with an optional class chip
/// and `line:col` ref; click a row → switch to that tab. Sourced from
/// `crate::diagnostics` (NOT the event log), so a fixed problem clears itself.
fn problems_body(state: Signal<AppState>) -> Element {
    use crate::diagnostics::{Diagnostic, Severity};

    // Iterate the reactive session store so the view tracks the tab set, and read
    // each tab's diagnostics reactively (validation slice ∪ execution error).
    let sess = crate::session::store();
    let mut groups: Vec<(u64, String, Vec<Diagnostic>)> = Vec::new();
    for w in sess.workspaces().iter() {
        let id = w.id().cloned();
        let diags = crate::diagnostics::problems_for(id);
        if !diags.is_empty() {
            groups.push((id, w.name().cloned(), diags));
        }
    }

    if groups.is_empty() {
        return rsx! {
            div { class: "prob-empty",
                {icons::check(26)}
                div { "No problems detected" }
            }
        };
    }

    rsx! {
        div { class: "ps-log-body ps-scroll",
            for (id, name, diags) in groups {
                {
                    let n = diags.len();
                    let gcount = format!("{n} problem{}", if n == 1 { "" } else { "s" });
                    rsx! {
                        div { class: "prob-group",
                            {icons::file(14)}
                            span { class: "prob-gname", "{name}" }
                            span { class: "prob-gcount", "{gcount}" }
                        }
                        for (i, d) in diags.into_iter().enumerate() {
                            {
                                let (row_cls, sev_icon) = match d.severity {
                                    Severity::Error => ("prob-row err", icons::problems(15)),
                                    Severity::Warning => ("prob-row warn", icons::warning(15)),
                                    Severity::Info => ("prob-row info", icons::events(15)),
                                };
                                rsx! {
                                    div {
                                        key: "p{id}-{i}",
                                        class: "{row_cls}",
                                        title: "Go to source",
                                        onclick: move |_| dispatch(state, Action::SwitchTab(id)),
                                        {sev_icon}
                                        span { class: "prob-msg", "{d.message}" }
                                        if let Some(code) = d.code.clone() {
                                            span { class: "prob-code", "{code}" }
                                        }
                                        if let Some(l) = d.loc.clone() {
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
    }
}
