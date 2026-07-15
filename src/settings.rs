//! Shared, cross-window **settings** — the user's [`crate::config::Settings`], held
//! in owner-independent (leaked) `Signal`s in a thread-local.
//!
//! Every window runs on the one UI thread, so a `thread_local!` is effectively
//! process-global: the *same* signals are shared by all windows. Reads inside a
//! component subscribe reactively (a change re-renders every window); the
//! non-component action layer (`row_limit` in `query::saved`, `open_pref` in
//! `projects::open_dir`, `keybinds` in `keymap`) reads the very same signals — a
//! plain context provider would not reach it, which is why this stays a global.
//!
//! The state is split into **three** shared signals by *when* a change lands in the
//! other windows and *whether* it persists:
//!
//! - [`Shared::theme`] — the **live** display theme. The Settings window writes it
//!   the instant a theme is picked, so every window re-themes immediately (a live
//!   preview). It is **never** written to disk here; Save commits it, Cancel
//!   reverts it.
//! - [`Shared::applied`] — the **committed** settings (behaviour: density / zebra /
//!   row-limit / open-pref / …), read by every window. Changes only on the Settings
//!   window's Save, or via an explicit immediate mutator (a dialog "don't ask
//!   again").
//! - [`Shared::os_dark`] — the **OS appearance**, a programme-wide *runtime* fact
//!   (never persisted). It feeds the same [`effective_theme`] as `theme`, so it
//!   shares this tier rather than living in a per-window global; any window's
//!   `ThemeChanged` updates it once and every window re-themes.
//!
//! Persistence to the machine-global app config happens **only** on Save (all
//! fields, theme included) or from those immediate mutators — `os_dark` is never
//! persisted. See [[settings-store]].

use std::cell::Cell;

use dioxus::prelude::*;

use crate::config::{KeyBind, OpenPref, Settings};

/// The live display-theme selection: the chosen theme id + whether to follow the
/// OS. The *effective* theme (honouring Sync-with-OS and the OS appearance) is
/// derived in [`effective_theme`].
#[derive(Clone, PartialEq)]
pub struct ThemeSel {
    pub id: String,
    pub sync_os: bool,
}

impl ThemeSel {
    fn of(s: &Settings) -> Self {
        Self {
            id: s.theme.clone(),
            sync_os: s.sync_os,
        }
    }
}

/// The shared, cross-window settings handles. All **leaked** signals (no owner
/// scope), so they survive any window closing and are safe to share by value.
#[derive(Clone, Copy)]
struct Shared {
    /// Committed settings — the behaviour source of truth for every window.
    applied: Signal<Settings>,
    /// Live display theme — previews across all windows; committed on Save.
    theme: Signal<ThemeSel>,
    /// OS appearance (`true` = dark). Programme-wide + **runtime-only** (never
    /// persisted): it's a system fact, the same in every window, and feeds the same
    /// [`effective_theme`] as `theme` — so it lives in this shared tier, not a
    /// per-window global (F7). Any window's `ThemeChanged` writes it once and every
    /// window re-themes.
    os_dark: Signal<bool>,
}

thread_local! {
    /// The one shared settings context. Created lazily on first access (see
    /// [`shared`]) and then reused by every window on this (the only) UI thread.
    static SHARED: Cell<Option<Shared>> = Cell::new(None);
}

/// The current OS appearance (`true` = dark). Reactive — reading it in a component
/// subscribes, so a Sync-with-OS window re-themes on an OS light/dark switch. Backed
/// by the cross-window [`Shared::os_dark`] signal.
pub fn os_dark() -> bool {
    *shared().os_dark.read()
}

/// Get — creating on first call — the shared context. The first caller leaks the
/// two signals from the current app config; every later caller, including other
/// windows, receives the same handles. Reading a leaked signal needs no runtime,
/// but the first call happens from a window render (the root reads the theme on
/// mount), so a reactive scope is live to subscribe.
fn shared() -> Shared {
    SHARED.with(|c| {
        if let Some(s) = c.get() {
            return s;
        }
        let cfg = crate::config::load().settings;
        let loc = std::panic::Location::caller();
        let s = Shared {
            applied: Signal::leak_with_caller(cfg.clone(), loc),
            theme: Signal::leak_with_caller(ThemeSel::of(&cfg), loc),
            // Detect the OS appearance once, here — every window then reads this same
            // value (and its `ThemeChanged` handler keeps it current via `set_os_dark`).
            os_dark: Signal::leak_with_caller(crate::theme::os_is_dark(), loc),
        };
        c.set(Some(s));
        s
    })
}

/// Ensure the shared context exists — called once from each window root's mount so
/// a later non-component read never races a cold init. Idempotent.
pub fn init() {
    let _ = shared();
}

/// Component hook: the single place a window root wires itself into the shared
/// settings. Seeds the shared context + current OS appearance **once**, then returns
/// the effective theme's CSS-variable string — read **reactively** (subscribes), so
/// a theme preview or OS light/dark switch re-themes this window. Called by every
/// window root (`ProjectRoot`, `SettingsRoot`, `LauncherRoot`); the returned string
/// is injected as the root element's `style`.
pub fn use_settings() -> String {
    use_hook(|| {
        init();
        set_os_dark(crate::theme::os_is_dark());
    });
    crate::theme::css_for(&effective_theme())
}

// ---- reactive reads (subscribe in a component, plain read elsewhere) ----------

/// The theme id that should actually apply right now (honours Sync-with-OS).
/// Subscribes to the live theme and the OS appearance, so a preview or an OS switch
/// re-themes the reader.
pub fn effective_theme() -> String {
    let s = shared();
    let t = s.theme.read();
    let x = crate::theme::effective_id(&t.id, t.sync_os, *s.os_dark.read());
    x
}

pub fn density_compact() -> bool {
    shared().applied.read().density_compact
}

pub fn zebra() -> bool {
    shared().applied.read().zebra
}

pub fn default_col_width() -> f64 {
    shared().applied.read().default_col_width
}

pub fn row_limit() -> usize {
    shared().applied.read().row_limit
}
pub fn max_history() -> usize {
    shared().applied.read().max_history
}

pub fn open_pref() -> OpenPref {
    shared().applied.read().open_pref
}

pub fn default_project_dir() -> String {
    shared().applied.read().default_project_dir.clone()
}

/// Whether to confirm before closing a tab/window with a running query (S14).
pub fn confirm_close_running() -> bool {
    shared().applied.read().confirm_close_running
}

/// The user's key-binding overrides (empty = all defaults). Read by `crate::keymap`.
pub fn keybinds() -> Vec<KeyBind> {
    shared().applied.read().keybinds.clone()
}

/// The engine config overrides (only non-default `datafusion.*` keys). Read by
/// `crate::engine` at spawn and re-sent live on change (W2). Reactive.
pub fn engine_overrides() -> std::collections::BTreeMap<String, String> {
    shared().applied.read().engine.clone()
}

/// A one-shot snapshot of the committed settings (a clone), used to seed the
/// Settings window's local draft. Peeks — the draft owns its copy from there on.
pub fn snapshot() -> Settings {
    shared().applied.peek().clone()
}

// ---- live theme preview (Settings window; immediate across windows, no persist) -

/// Preview `id` as the live theme across **every** window. Not persisted — the
/// Settings window's Save commits it; Cancel reverts via [`revert_theme_preview`].
pub fn preview_theme(id: String) {
    let mut theme = shared().theme;
    theme.write().id = id;
}

/// Preview the Sync-with-OS toggle live across every window (no persist).
pub fn preview_sync_os(on: bool) {
    let mut theme = shared().theme;
    theme.write().sync_os = on;
}

// ---- save / cancel (Settings window) -----------------------------------------

/// Commit the Settings window's `draft`: publish it as the applied settings (all
/// windows pick up the new behaviour), keep the live theme in lockstep, and persist
/// to the app config. This is the **only** place the Settings window writes to disk.
pub fn save_draft(draft: Settings) {
    let sh = shared();
    let sel = ThemeSel::of(&draft);
    let mut applied = sh.applied;
    let mut theme = sh.theme;
    applied.set(draft.clone());
    if *theme.peek() != sel {
        theme.set(sel);
    }
    persist(draft);
}

/// Discard a live theme preview, reverting to the committed theme — called when the
/// Settings window is cancelled/closed without saving.
pub fn revert_theme_preview() {
    let sh = shared();
    let saved = ThemeSel::of(&sh.applied.peek());
    let mut theme = sh.theme;
    if *theme.peek() != saved {
        theme.set(saved);
    }
}

// ---- immediate mutators (dialogs; write applied + persist now) -----------------

/// Set + persist the confirm-close-running preference immediately (a running-close
/// dialog's "don't ask again" — not part of the Settings window draft).
pub fn set_confirm_close_running(v: bool) {
    let mut applied = shared().applied;
    applied.write().confirm_close_running = v;
    persist(applied.peek().clone());
}

/// Set + persist the open preference immediately (the open-target prompt's
/// "remember my choice").
pub fn set_open_pref(pref: OpenPref) {
    let mut applied = shared().applied;
    applied.write().open_pref = pref;
    persist(applied.peek().clone());
}

// ---- OS appearance ------------------------------------------------------------

/// Record the OS appearance (from a window's `ThemeChanged` event / startup
/// detection). Reactive, so a Sync-with-OS window re-themes live.
pub fn set_os_dark(dark: bool) {
    let mut sig = shared().os_dark;
    if *sig.peek() != dark {
        *sig.write() = dark;
    }
}

// ---- persistence --------------------------------------------------------------

/// Write `s` back to the machine-global app config, preserving recents / open-set.
fn persist(s: Settings) {
    let mut cfg = crate::config::load();
    cfg.settings = s;
    crate::config::save(&cfg);
}
