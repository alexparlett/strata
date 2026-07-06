//! Overlay / menu action handlers: command palette, the global overlay-close,
//! and the nested-cell JSON popover. Called from `action::dispatch`.

use dioxus::prelude::*;

use crate::state::{AppState, LogTab, SettingsCat};

/// Toggle the command palette (⌘K), resetting its query + selection.
pub fn toggle_cmdk(mut state: Signal<AppState>) {
    let mut s = state.write();
    s.cmdk_open = !s.cmdk_open;
    s.cmdk_query.clear();
    s.cmdk_active = 0;
}

/// Close every overlay / menu / dialog (Esc, backdrop clicks).
pub fn close_all(mut state: Signal<AppState>) {
    let mut s = state.write();
    s.cmdk_open = false;
    s.export_open = false;
    s.config_open = false;
    s.cell_open = false;
    s.settings_open = false;
    s.project_menu_open = false;
    s.page_size_open = false;
    s.remove_open = false;
    s.remove_target = None;
    s.ctx_menu = None;
    s.tab_menu = None;
    s.renaming_ws = None;
}

/// Open the Settings modal (⌘, or the header gear).
pub fn open_settings(mut state: Signal<AppState>) {
    state.write().settings_open = true;
}

/// Switch the active theme and persist it to the machine-global app config, so
/// the choice survives a restart and applies to new windows + the launcher.
pub fn set_theme(mut state: Signal<AppState>, id: String) {
    state.write().theme_id = id.clone();
    let mut cfg = crate::config::load();
    cfg.theme = id;
    crate::config::save(&cfg);
}

/// Persist all Settings-page prefs to the machine-global app config (recent
/// projects are preserved). Called by every settings-pref handler below.
pub fn save_prefs(state: Signal<AppState>) {
    let s = state.read();
    let mut cfg = crate::config::load();
    cfg.theme = s.theme_id.clone();
    cfg.sync_os = s.sync_os;
    cfg.density_compact = s.density_compact;
    cfg.zebra = s.zebra;
    cfg.row_limit = s.row_limit;
    cfg.reopen_on_startup = s.reopen_on_startup;
    cfg.default_project_dir = s.default_project_dir.clone();
    cfg.open_pref = s.open_pref.clone();
    cfg.confirm_close_running = s.confirm_close_running;
    crate::config::save(&cfg);
}

// ---- settings-page prefs (each mutates state, then persists) ----

/// Switch the Settings modal's left-nav category (ephemeral — not persisted).
pub fn set_settings_cat(mut state: Signal<AppState>, cat: SettingsCat) {
    state.write().settings_cat = cat;
}

pub fn toggle_sync_os(mut state: Signal<AppState>) {
    {
        let mut s = state.write();
        s.sync_os = !s.sync_os;
    }
    save_prefs(state);
}

pub fn set_density(mut state: Signal<AppState>, compact: bool) {
    state.write().density_compact = compact;
    save_prefs(state);
}

pub fn toggle_zebra(mut state: Signal<AppState>) {
    {
        let mut s = state.write();
        s.zebra = !s.zebra;
    }
    save_prefs(state);
}

pub fn set_row_limit(mut state: Signal<AppState>, limit: usize) {
    state.write().row_limit = limit;
    save_prefs(state);
}

pub fn toggle_reopen_startup(mut state: Signal<AppState>) {
    {
        let mut s = state.write();
        s.reopen_on_startup = !s.reopen_on_startup;
    }
    save_prefs(state);
}

pub fn set_default_project_dir(mut state: Signal<AppState>, dir: String) {
    state.write().default_project_dir = dir;
    save_prefs(state);
}

pub fn set_open_pref(mut state: Signal<AppState>, pref: String) {
    state.write().open_pref = pref;
    save_prefs(state);
}

pub fn toggle_confirm_close(mut state: Signal<AppState>) {
    {
        let mut s = state.write();
        s.confirm_close_running = !s.confirm_close_running;
    }
    save_prefs(state);
}

/// Open the export modal.
pub fn open_export(mut state: Signal<AppState>) {
    state.write().export_open = true;
}

/// Open the nested-cell JSON popover for a struct/list/map cell.
pub fn open_cell(mut state: Signal<AppState>, name: String, type_label: String, json: String) {
    let mut s = state.write();
    s.cell.name = name;
    s.cell.type_label = type_label;
    s.cell.json = json;
    s.cell_open = true;
}

/// Toggle the header's project switcher menu.
pub fn toggle_project_menu(mut state: Signal<AppState>) {
    let mut s = state.write();
    s.project_menu_open = !s.project_menu_open;
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

/// Switch the drawer's active tab (History / Events tab buttons).
pub fn set_log_tab(mut state: Signal<AppState>, tab: LogTab) {
    state.write().log_tab = tab;
}

/// Re-open a past query (from the History tab) into a tab, keeping the drawer
/// open. Idempotent (reuse-if-same) so a double-click — which fires `onclick`
/// twice before `ondoubleclick` — doesn't spawn duplicate tabs. (Double-click
/// also runs it — see `Action::RunHistoryQuery`.)
pub fn open_history_query(mut state: Signal<AppState>, sql: String) {
    state.write().open_or_focus_sql(sql);
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
