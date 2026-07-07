//! The permanent 48px activity rail (S23, RustRover model) — always visible on the
//! far left. Top group = tool windows (**Catalog** toggle); bottom group =
//! **Problems / Events / History**, each toggling the bottom drawer to that view.
//! Replaces the old collapsed-sidebar rail (B9). The **Connections** pane joins the
//! top group with S21.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, LogKind, LogTab};
use crate::ui::icons;

#[component]
pub(crate) fn ActivityRail() -> Element {
    let state = use_context::<Signal<AppState>>();
    let (sidebar_open, log_open, log_tab, problem_count) = {
        let s = state.read();
        let n = s.log.iter().filter(|e| e.kind == LogKind::Error).count();
        (s.sidebar_open, s.log_open, s.log_tab, n)
    };
    let on = |t: LogTab| log_open && log_tab == t;

    rsx! {
        aside { class: "act-rail",
            // Top group: tool windows.
            {rail_btn(state, "Catalog", sidebar_open, None, icons::database(18), Action::ToggleSidebar)}

            div { class: "rail-spacer" }

            // Bottom group: diagnostics & activity.
            {rail_btn(state, "Problems", on(LogTab::Problems), Some(problem_count), icons::problems(18), Action::OpenProblems)}
            {rail_btn(state, "Events", on(LogTab::Events), None, icons::events(18), Action::OpenEvents)}
            {rail_btn(state, "History", on(LogTab::History), None, icons::clock(18), Action::OpenHistory)}
        }
    }
}

/// One rail button: active accent stripe + an optional red count badge (Problems).
fn rail_btn(
    state: Signal<AppState>,
    title: &str,
    active: bool,
    badge: Option<usize>,
    icon: Element,
    action: Action,
) -> Element {
    let cls = if active { "rail-btn on" } else { "rail-btn" };
    rsx! {
        button {
            class: "{cls}",
            title: "{title}",
            onclick: move |_| dispatch(state, action.clone()),
            if active {
                span { class: "rail-stripe" }
            }
            {icon}
            if let Some(n) = badge {
                if n > 0 {
                    span { class: "rail-badge", { if n > 99 { "99+".to_string() } else { n.to_string() } } }
                }
            }
        }
    }
}
