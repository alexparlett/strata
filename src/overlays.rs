//! Per-window **overlay store** — visibility state for the app-global overlays
//! (Settings, the command palette; Export/Config join as they migrate).
//!
//! Deliberately kept *out* of `AppState` (the domain/project state): overlay
//! visibility is a pure UI concern, and `AppState` is being decomposed rather than
//! grown. This mirrors the React/Zustand shape — a small, focused store read
//! reactively inside the always-mounted *host* components and written via plain
//! helpers from anywhere, including the non-component action/engine layer (e.g. an
//! export runner closing its own window).
//!
//! Each project window is its own `VirtualDom` (see [`crate::window`]), and a
//! `GlobalSignal` is scoped to its `VirtualDom`, so this store is **per-window** —
//! overlays never cross-trigger between windows.

use dioxus::prelude::*;

/// Which overlays are currently open in this window.
#[derive(Clone, Default)]
pub struct OverlayState {
    pub settings: bool,
    pub cmdk: bool,
    pub export: bool,
    pub config: bool,
}

/// The per-window overlay store. Hosts read it reactively
/// (`OVERLAYS.read().settings`) and re-render when it changes; triggers mutate it
/// through the helpers below (callable from components *and* plain functions).
pub static OVERLAYS: GlobalSignal<OverlayState> = Signal::global(OverlayState::default);

pub fn toggle_settings() {
    let open = OVERLAYS.peek().settings;
    OVERLAYS.write().settings = !open;
}

pub fn set_settings(open: bool) {
    OVERLAYS.write().settings = open;
}

pub fn toggle_cmdk() {
    let open = OVERLAYS.peek().cmdk;
    OVERLAYS.write().cmdk = !open;
}

pub fn set_cmdk(open: bool) {
    OVERLAYS.write().cmdk = open;
}

pub fn open_export() {
    OVERLAYS.write().export = true;
}

/// Close the export window. Callable from the non-component action/engine layer —
/// `run_export` uses it to dismiss the window when the export is under way.
pub fn close_export() {
    OVERLAYS.write().export = false;
}

/// Open the table-config window. The `OpenConfigNew`/`OpenConfigEdit` handlers set
/// up the form (`AppState.cfg`) first, then call this.
pub fn open_config() {
    OVERLAYS.write().config = true;
}

/// Close the table-config window. Callable from the engine-event layer — the
/// `Registered` handler uses it to dismiss the window on a successful register.
pub fn close_config() {
    OVERLAYS.write().config = false;
}
