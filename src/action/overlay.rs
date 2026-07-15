//! Bottom-drawer action handlers (open / toggle / clear + history-query re-open).
//! Called from `action::dispatch`. (Settings prefs no longer live here — the
//! Settings modal writes the `crate::settings` store directly.)

use dioxus::prelude::*;

use crate::state::{AppState, LogTab};

/// Open the bottom drawer on the **History** tab (status-bar History button).
/// Toggles closed if it's already open on History.
pub fn open_history() {
    crate::layout::toggle_drawer(LogTab::History);
}

/// Open the bottom drawer on the **Events** tab (status-bar Events button).
/// Toggles closed if it's already open on Events.
pub fn open_events() {
    crate::layout::toggle_drawer(LogTab::Events);
}

/// Open the bottom drawer on the **Problems** tab (rail Problems button, S23).
/// Toggles closed if it's already open on Problems.
pub fn open_problems() {
    crate::layout::toggle_drawer(LogTab::Problems);
}

/// Switch the drawer's active tab (History / Events tab buttons).
pub fn set_log_tab(tab: LogTab) {
    crate::layout::set_drawer_tab(tab);
}

/// Re-open a past query (from the History tab) into a tab, keeping the drawer
/// open. Idempotent (reuse-if-same) so a double-click — which fires `onclick`
/// twice before `ondoubleclick` — doesn't spawn duplicate tabs. (Double-click
/// also runs it — see `Action::RunHistoryQuery`.)
pub fn open_history_query(sql: String) {
    crate::session::open_or_focus_sql(sql);
}

/// Toggle the bottom drawer open/closed (drawer close button).
pub fn toggle_log() {
    crate::layout::toggle_drawer_open();
}

/// Clear the active drawer tab (Events → the events store; History → project history).
pub fn clear_drawer(mut state: Signal<AppState>) {
    match crate::layout::drawer_tab() {
        LogTab::Events => crate::events::clear(),
        LogTab::History => state.write().project.history.clear(),
        // Problems has no Clear button (they're live diagnostics — a fixed problem
        // clears itself). Kept as an exhaustive no-op arm.
        LogTab::Problems => {}
    }
}

/// Toggle an Events-tab error row's expanded detail (message + code frame +
/// hint). No-op for non-error rows, which carry no structured error.
pub fn toggle_log_row(id: u64) {
    crate::events::toggle_row(id);
}

