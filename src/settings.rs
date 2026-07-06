//! Per-window **settings store** — the user's [`crate::config::Settings`] held in a
//! `GlobalSignal`, mirroring the shape of [`crate::overlays`].
//!
//! Deliberately *not* on `AppState`: settings are a cross-cutting concern read from
//! both components (the grid's zebra/density, the header/theme) and the
//! non-component action layer (`row_limit` in `query::select_star`, `open_pref` /
//! `default_project_dir` in `projects::open_dir`) — a `GlobalSignal` is reachable
//! from both; a context provider would not be. Each project window is its own
//! `VirtualDom`, so the store is per-window (like the app it mirrors); mutations
//! persist back to the machine-global app config immediately.

use dioxus::prelude::*;

use crate::config::Settings;

/// This window's live settings. Seeded from the app config on window startup
/// ([`load`]); every mutation below writes through to the config.
pub static SETTINGS: GlobalSignal<Settings> = Signal::global(Settings::default);

/// The OS appearance (dark), detected at startup and updated live by the window's
/// `ThemeChanged` handler. Runtime-only (never persisted); drives the effective
/// theme while Sync-with-OS is on.
pub static OS_DARK: GlobalSignal<bool> = Signal::global(|| true);

/// Seed the store from the app config (called once per window at startup).
pub fn load() {
    *SETTINGS.write() = crate::config::load().settings;
}

/// Persist the current settings back to the app config, preserving recents.
fn persist() {
    let mut cfg = crate::config::load();
    cfg.settings = SETTINGS.peek().clone();
    crate::config::save(&cfg);
}

/// The theme id that should actually apply right now (honours Sync-with-OS).
pub fn effective_theme() -> String {
    let s = SETTINGS.read();
    crate::theme::effective_id(&s.theme, s.sync_os, *OS_DARK.read())
}

/// Record the OS appearance (from the window's `ThemeChanged` event / startup
/// detection). Reactive, so a Sync-with-OS window re-themes live.
pub fn set_os_dark(dark: bool) {
    if *OS_DARK.peek() != dark {
        *OS_DARK.write() = dark;
    }
}

// ---- mutators (each writes the store, then persists to the app config) ----

pub fn set_theme(id: String) {
    SETTINGS.write().theme = id;
    persist();
}

pub fn toggle_sync_os() {
    let v = SETTINGS.peek().sync_os;
    SETTINGS.write().sync_os = !v;
    persist();
}

pub fn set_density(compact: bool) {
    SETTINGS.write().density_compact = compact;
    persist();
}

pub fn toggle_zebra() {
    let v = SETTINGS.peek().zebra;
    SETTINGS.write().zebra = !v;
    persist();
}

pub fn set_row_limit(limit: usize) {
    SETTINGS.write().row_limit = limit;
    persist();
}

pub fn toggle_reopen_startup() {
    let v = SETTINGS.peek().reopen_on_startup;
    SETTINGS.write().reopen_on_startup = !v;
    persist();
}

pub fn set_default_project_dir(dir: String) {
    SETTINGS.write().default_project_dir = dir;
    persist();
}

pub fn set_open_pref(pref: String) {
    SETTINGS.write().open_pref = pref;
    persist();
}

pub fn toggle_confirm_close() {
    let v = SETTINGS.peek().confirm_close_running;
    SETTINGS.write().confirm_close_running = !v;
    persist();
}
