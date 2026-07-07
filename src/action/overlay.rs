//! Overlay / menu action handlers: the global overlay-close plus the drawer
//! handlers. Called from `action::dispatch`. (Settings prefs no longer live here —
//! the Settings modal writes the `crate::settings` store directly.)

use dioxus::prelude::*;

use crate::state::{AppState, LogKind, LogTab};

/// Close every remaining `AppState`-backed overlay (Esc, backdrop clicks). The
/// container-based overlays (menus, remove-confirm, cell view) and the
/// overlay-store windows (command palette, Settings, Export) own their own open
/// state and aren't touched here.
pub fn close_all(mut state: Signal<AppState>) {
    // The inline-rename mode is now `Tabs`-component-local (its input handles its
    // own Esc); only the pager dropdown remains an `AppState`-backed overlay here.
    state.write().page_size_open = false;
}

/// Open the bottom drawer on the **History** tab (status-bar History button).
/// Toggles closed if it's already open on History.
pub fn open_history(mut state: Signal<AppState>) {
    let mut s = state.write();
    if s.log_open && s.log_tab == LogTab::History {
        s.log_open = false;
    } else {
        s.log_open = true;
        s.log_tab = LogTab::History;
    }
}

/// Open the bottom drawer on the **Events** tab (status-bar Events button).
/// Toggles closed if it's already open on Events.
pub fn open_events(mut state: Signal<AppState>) {
    let mut s = state.write();
    if s.log_open && s.log_tab == LogTab::Events {
        s.log_open = false;
    } else {
        s.log_open = true;
        s.log_tab = LogTab::Events;
    }
}

/// Open the bottom drawer on the **Problems** tab (rail Problems button, S23).
/// Toggles closed if it's already open on Problems.
pub fn open_problems(mut state: Signal<AppState>) {
    let mut s = state.write();
    if s.log_open && s.log_tab == LogTab::Problems {
        s.log_open = false;
    } else {
        s.log_open = true;
        s.log_tab = LogTab::Problems;
    }
}

/// Switch the drawer's active tab (History / Events tab buttons).
pub fn set_log_tab(mut state: Signal<AppState>, tab: LogTab) {
    state.write().log_tab = tab;
}

/// Re-open a past query (from the History tab) into a tab, keeping the drawer
/// open. Idempotent (reuse-if-same) so a double-click — which fires `onclick`
/// twice before `ondoubleclick` — doesn't spawn duplicate tabs. (Double-click
/// also runs it — see `Action::RunHistoryQuery`.)
pub fn open_history_query(_state: Signal<AppState>, sql: String) {
    crate::session::open_or_focus_sql(sql);
}

/// Toggle the bottom drawer open/closed (drawer close button).
pub fn toggle_log(mut state: Signal<AppState>) {
    let mut s = state.write();
    s.log_open = !s.log_open;
}

/// Clear the active drawer tab: the events list, or the project's query history.
pub fn clear_drawer(mut state: Signal<AppState>) {
    let mut s = state.write();
    match s.log_tab {
        LogTab::Events => s.log.clear(),
        LogTab::History => s.project.history.clear(),
        // Problems is a filtered view of error events — clearing it removes them.
        LogTab::Problems => s.log.retain(|e| e.kind != LogKind::Error),
    }
}

/// Toggle an Events-tab error row's expanded detail (message + code frame +
/// hint). No-op for non-error rows, which carry no structured error.
pub fn toggle_log_row(mut state: Signal<AppState>, id: u64) {
    let mut s = state.write();
    if let Some(e) = s.log.iter_mut().find(|e| e.id == id) {
        e.open = !e.open;
    }
}

/// Toggle the drawer between its compact and expanded heights.
pub fn toggle_log_height(mut state: Signal<AppState>) {
    let mut s = state.write();
    s.log_h = if s.log_h > 250.0 { 168.0 } else { 360.0 };
}
