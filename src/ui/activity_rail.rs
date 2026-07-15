//! The permanent 48px activity rail (S23, RustRover model) — always visible on the
//! far left. Top group = tool windows (**Catalog** toggle); bottom group =
//! **Problems / Events / History**, each toggling the bottom drawer to that view.
//! Replaces the old collapsed-sidebar rail (B9). The **Connections** pane joins the
//! top group with S21.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, LogTab};
use crate::ui::components::{IconButton, IconButtonVariant};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub(crate) fn ActivityRail() -> Element {
    let state = use_context::<Signal<AppState>>();
    let sidebar_open = crate::layout::sidebar_open();
    let drawer_open = crate::layout::drawer_open();
    let drawer_tab = crate::layout::drawer_tab();
    // Live error-diagnostic count across all tabs (validation ∪ execution). Reads
    // the session + diagnostics + runs stores reactively, so the badge tracks
    // problems as they appear and clear — no query run required.
    let problem_count = crate::diagnostics::total_problems();
    let on = |t: LogTab| drawer_open && drawer_tab == t;

    rsx! {
        aside { class: "act-rail",
            // Top group: tool windows.
            {rail_btn(state, "Catalog", sidebar_open, None, IconName::Database, Action::ToggleSidebar)}

            div { class: "rail-spacer" }

            // Bottom group: diagnostics & activity.
            {rail_btn(state, "Problems", on(LogTab::Problems), Some(problem_count), IconName::Problems, Action::OpenProblems)}
            {rail_btn(state, "Events", on(LogTab::Events), None, IconName::Events, Action::OpenEvents)}
            {rail_btn(state, "History", on(LogTab::History), None, IconName::Clock, Action::OpenHistory)}
        }
    }
}

/// One rail button — the shared **toggle** `IconButton` at the rail size (active =
/// accent-soft tint + accent icon); an optional red count badge (Problems).
fn rail_btn(
    state: Signal<AppState>,
    title: &str,
    active: bool,
    badge: Option<usize>,
    icon: IconName,
    action: Action,
) -> Element {
    rsx! {
        div { class: "ds-badge-anchor",
            IconButton {
                variant: IconButtonVariant::Toggle,
                icon: icon,
                icon_size: IconSize::Lg,
                class: "act-rail",
                on: active,
                title: "{title}",
                onclick: move |_| dispatch(state, action.clone()),
            }
            if let Some(n) = badge {
                if n > 0 {
                    span { class: "ds-count-badge err", { if n > 99 { "99+".to_string() } else { n.to_string() } } }
                }
            }
        }
    }
}
