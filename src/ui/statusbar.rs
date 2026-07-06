//! Bottom status bar. The left status text doubles as the **Events** entry
//! (opens the bottom drawer on the Events tab); a **History** button opens it on
//! the History tab.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, LogKind};
use crate::ui::icons;

#[component]
pub fn StatusBar() -> Element {
    let state = use_context::<Signal<AppState>>();
    // An unresolved query error wins the status line: a later background event
    // (e.g. an async "Registered table…" completing) must not turn the dot green
    // while the failed query is still on screen. Cleared on re-run / dismiss.
    let (status, kind) = {
        let s = state.read();
        (s.status_text.clone(), s.status_kind)
    };
    // The dot reflects the current status severity (kept in step with the text).
    let dot = match kind {
        LogKind::Ok => "var(--green)",
        LogKind::Info | LogKind::Run => "var(--accent)",
        LogKind::Warn => "var(--orange)",
        LogKind::Error => "var(--red2)",
    };

    rsx! {
        footer { class: "ps-status",
            button {
                class: "item log-toggle",
                title: "Event log",
                onclick: move |_| dispatch(state, Action::OpenEvents),
                span { class: "stat-dot", background: dot }
                "{status}"
            }
            div { style: "width:1px;height:13px;background:var(--line);flex:none;" }
            button {
                class: "item log-toggle",
                title: "Query history",
                onclick: move |_| dispatch(state, Action::OpenHistory),
                {icons::clock(12)}
                span { "History" }
            }
            div { class: "spacer" }
        }
    }
}
