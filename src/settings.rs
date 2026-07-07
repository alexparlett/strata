//! Per-window **settings store** — the user's [`crate::config::Settings`] held in a
//! `dioxus-stores` `GlobalStore`, mirroring [`crate::overlays`].
//!
//! Deliberately *not* on `AppState`: settings are a cross-cutting concern read from
//! both components (the grid's zebra/density, the header/theme) and the
//! non-component action layer (`row_limit` in `query::select_star`, `open_pref` /
//! `default_project_dir` in `projects::open_dir`) — a per-window `GlobalStore` is
//! reachable from both; a context provider would not be. Each project window is its
//! own `VirtualDom`, so the store is per-window; mutations persist back to the
//! machine-global app config immediately.
//!
//! **Mutators write through field lenses** (`SETTINGS.resolve().theme().set(..)`),
//! never a coarse `.write()` — see [[workbench-and-runs]] for why a lens write is
//! what actually notifies lens subscribers.

use dioxus::prelude::*;
use dioxus_stores::*;

use crate::config::{Settings, SettingsStoreExt};

/// This window's live settings. Seeded from the app config on window startup
/// ([`load`]); every mutation below writes through to the config.
pub static SETTINGS: GlobalStore<Settings> = Global::new(|| Settings::default());

/// The OS appearance (dark), detected at startup and updated live by the window's
/// `ThemeChanged` handler. Runtime-only (never persisted); a bare bool, so it stays
/// a `GlobalSignal` (nothing to lens).
pub static OS_DARK: GlobalSignal<bool> = Signal::global(|| true);

/// Seed the store from the app config (called once per window at startup).
pub fn load() {
    *SETTINGS.resolve().write() = crate::config::load().settings;
}

/// Persist the current settings back to the app config, preserving recents.
fn persist() {
    let mut cfg = crate::config::load();
    cfg.settings = SETTINGS.resolve().peek().clone();
    crate::config::save(&cfg);
}

/// The theme id that should actually apply right now (honours Sync-with-OS).
pub fn effective_theme() -> String {
    let s = SETTINGS.resolve();
    let theme = s.theme().cloned();
    let sync = s.sync_os().cloned();
    crate::theme::effective_id(&theme, sync, *OS_DARK.read())
}

/// Record the OS appearance (from the window's `ThemeChanged` event / startup
/// detection). Reactive, so a Sync-with-OS window re-themes live.
pub fn set_os_dark(dark: bool) {
    if *OS_DARK.peek() != dark {
        *OS_DARK.write() = dark;
    }
}

// ---- mutators (each writes the store via a lens, then persists to the config) ----

pub fn set_theme(id: String) {
    SETTINGS.resolve().theme().set(id);
    persist();
}

pub fn toggle_sync_os() {
    let s = SETTINGS.resolve();
    let v = s.sync_os().cloned();
    s.sync_os().set(!v);
    persist();
}

pub fn set_density(compact: bool) {
    SETTINGS.resolve().density_compact().set(compact);
    persist();
}

pub fn toggle_zebra() {
    let s = SETTINGS.resolve();
    let v = s.zebra().cloned();
    s.zebra().set(!v);
    persist();
}

pub fn set_row_limit(limit: usize) {
    SETTINGS.resolve().row_limit().set(limit);
    persist();
}

pub fn toggle_reopen_startup() {
    let s = SETTINGS.resolve();
    let v = s.reopen_on_startup().cloned();
    s.reopen_on_startup().set(!v);
    persist();
}

pub fn set_default_project_dir(dir: String) {
    SETTINGS.resolve().default_project_dir().set(dir);
    persist();
}

pub fn set_open_pref(pref: crate::config::OpenPref) {
    SETTINGS.resolve().open_pref().set(pref);
    persist();
}

pub fn toggle_confirm_close() {
    let s = SETTINGS.resolve();
    let v = s.confirm_close_running().cloned();
    s.confirm_close_running().set(!v);
    persist();
}

/// Whether to confirm before closing a tab/window with a running query (S14).
pub fn confirm_close_running() -> bool {
    SETTINGS.resolve().peek().confirm_close_running
}

/// Set the confirm-close-running preference (a dialog's "don't ask again").
pub fn set_confirm_close_running(v: bool) {
    SETTINGS.resolve().confirm_close_running().set(v);
    persist();
}
