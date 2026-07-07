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

/// What the table-config window is doing, if open: creating a new table or editing
/// an existing one (by name). `None` on the store = closed.
#[derive(Clone, PartialEq)]
pub enum ConfigTarget {
    New,
    Edit(String),
}

/// Row data for an **in-flight** config register, held here — *not* in the
/// project — so `project.tables` stays untouched until the engine confirms. The
/// `Registered` success handler builds the real catalog row from this + the
/// returned columns; a failure just clears it (there is no placeholder to clean up).
#[derive(Clone)]
pub struct PendingTable {
    pub name: String,
    pub format: String,
    pub sources: Vec<String>,
    pub partition_cols: Vec<(String, String)>,
}

/// Which overlays are currently open in this window.
#[derive(Clone, Default)]
pub struct OverlayState {
    pub settings: bool,
    pub cmdk: bool,
    pub export: bool,
    /// The table-config window's target (`Some` = open).
    pub config: Option<ConfigTarget>,
    /// Inline register-failure message for the config window (which stays open).
    pub config_err: Option<String>,
    /// Row data for a config register awaiting the engine's `Registered` event.
    pub pending_register: Option<PendingTable>,
    /// A workspace id awaiting a discard-confirm before it closes (A6). `None` = none.
    pub close_confirm: Option<crate::session::WorkspaceId>,
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

/// Open the table-config window for `target`; the modal seeds its local draft from
/// it (blank for `New`, a copy of the project table for `Edit`).
pub fn open_config(target: ConfigTarget) {
    let mut o = OVERLAYS.write();
    o.config = Some(target);
    o.config_err = None;
    o.pending_register = None;
}

/// Close the table-config window and clear its transient state.
pub fn close_config() {
    let mut o = OVERLAYS.write();
    o.config = None;
    o.config_err = None;
    o.pending_register = None;
}

/// Show an inline register error in the (still-open) config window.
pub fn set_config_err(msg: String) {
    OVERLAYS.write().config_err = Some(msg);
}

/// Stash the row data for an in-flight config register (clears any prior error).
pub fn begin_register(pending: PendingTable) {
    let mut o = OVERLAYS.write();
    o.pending_register = Some(pending);
    o.config_err = None;
}

/// Take the pending register **iff** it's for `name` — i.e. a config-originated
/// register. Returns `None` for load-time registers (which stash nothing).
pub fn take_pending_register(name: &str) -> Option<PendingTable> {
    let mut o = OVERLAYS.write();
    if o.pending_register
        .as_ref()
        .map_or(false, |p| p.name == name)
    {
        o.pending_register.take()
    } else {
        None
    }
}

/// Ask to confirm closing workspace `id` (it has unsaved changes). Callable from
/// the non-component action layer (`tab::close`).
pub fn open_close_confirm(id: crate::session::WorkspaceId) {
    OVERLAYS.write().close_confirm = Some(id);
}

/// Dismiss the close-confirm dialog.
pub fn close_close_confirm() {
    OVERLAYS.write().close_confirm = None;
}
