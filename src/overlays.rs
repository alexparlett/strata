//! Per-window **overlay store** — visibility state for the app-global overlays
//! (Settings, the command palette, Export, table Config, close-confirm).
//!
//! Deliberately kept *out* of `AppState` (the domain/project state): overlay
//! visibility is a pure UI concern. Held in a `dioxus-stores` `GlobalStore` — a
//! small, focused store read reactively inside the always-mounted *host* components
//! and written via plain helpers from anywhere, including the non-component
//! action/engine layer (e.g. an export runner closing its own window).
//!
//! Each project window is its own `VirtualDom` (see [`crate::window`]), and a
//! `GlobalStore` is scoped to its `VirtualDom`, so this store is **per-window** —
//! overlays never cross-trigger between windows. **Mutators write through field
//! lenses** (`OVERLAYS.resolve().cmdk().set(..)`), never a coarse `.write()` — a
//! lens write is what notifies lens subscribers (see [[workbench-and-runs]]).

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

/// What a running-query close confirm (S14) is guarding: a single tab (by id) or
/// the whole window (Close Project).
#[derive(Clone, Copy, PartialEq)]
pub enum RunningCloseTarget {
    Tab(crate::session::WorkspaceId),
    Window,
}

/// Which overlays are currently open in this window.
#[derive(Clone, Default, Store)]
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
    /// A tab or the window awaiting a running-query close confirm (S14). `None` = none.
    pub close_running_confirm: Option<RunningCloseTarget>,
    /// A picked project path awaiting a This-Window / New-Window choice (B10).
    /// `Some` when `open_pref == "ask"` and a project is being opened from a
    /// project window; the prompt host reads it.
    pub open_prompt: Option<std::path::PathBuf>,
}

/// The per-window overlay store. Hosts read it (`OVERLAYS.resolve().read().settings`)
/// and re-render when it changes; triggers mutate it through the lens helpers below
/// (callable from components *and* plain functions).
pub static OVERLAYS: GlobalStore<OverlayState> = Global::new(|| OverlayState::default());

/// Whether any dismissible overlay is currently open. Drives Esc priority (S14):
/// Esc closes an open overlay first, and only cancels a running query when none is up.
pub fn any_open() -> bool {
    let store = OVERLAYS.resolve();
    let s = store.read();
    s.settings
        || s.cmdk
        || s.export
        || s.config.is_some()
        || s.close_confirm.is_some()
        || s.close_running_confirm.is_some()
        || s.open_prompt.is_some()
}

pub fn toggle_settings() {
    let s = OVERLAYS.resolve();
    let open = s.settings().cloned();
    s.settings().set(!open);
}

pub fn set_settings(open: bool) {
    OVERLAYS.resolve().settings().set(open);
}

pub fn toggle_cmdk() {
    let s = OVERLAYS.resolve();
    let open = s.cmdk().cloned();
    s.cmdk().set(!open);
}

pub fn set_cmdk(open: bool) {
    OVERLAYS.resolve().cmdk().set(open);
}

pub fn open_export() {
    OVERLAYS.resolve().export().set(true);
}

/// Close the export window. Callable from the non-component action/engine layer —
/// `run_export` uses it to dismiss the window when the export is under way.
pub fn close_export() {
    OVERLAYS.resolve().export().set(false);
}

/// Open the table-config window for `target`; the modal seeds its local draft from
/// it (blank for `New`, a copy of the project table for `Edit`).
pub fn open_config(target: ConfigTarget) {
    let s = OVERLAYS.resolve();
    s.config().set(Some(target));
    s.config_err().set(None);
    s.pending_register().set(None);
}

/// Close the table-config window and clear its transient state.
pub fn close_config() {
    let s = OVERLAYS.resolve();
    s.config().set(None);
    s.config_err().set(None);
    s.pending_register().set(None);
}

/// Show an inline register error in the (still-open) config window.
pub fn set_config_err(msg: String) {
    OVERLAYS.resolve().config_err().set(Some(msg));
}

/// Stash the row data for an in-flight config register (clears any prior error).
pub fn begin_register(pending: PendingTable) {
    let s = OVERLAYS.resolve();
    s.pending_register().set(Some(pending));
    s.config_err().set(None);
}

/// Take the pending register **iff** it's for `name` — i.e. a config-originated
/// register. Returns `None` for load-time registers (which stash nothing).
pub fn take_pending_register(name: &str) -> Option<PendingTable> {
    let s = OVERLAYS.resolve();
    match s.pending_register().cloned() {
        Some(p) if p.name == name => {
            s.pending_register().set(None);
            Some(p)
        }
        _ => None,
    }
}

/// Ask to confirm closing workspace `id` (it has unsaved changes). Callable from
/// the non-component action layer (`tab::close`).
pub fn open_close_confirm(id: crate::session::WorkspaceId) {
    OVERLAYS.resolve().close_confirm().set(Some(id));
}

/// Dismiss the close-confirm dialog.
pub fn close_close_confirm() {
    OVERLAYS.resolve().close_confirm().set(None);
}

/// Ask to confirm closing a tab / the window that has a running query (S14).
pub fn open_running_close(target: RunningCloseTarget) {
    OVERLAYS.resolve().close_running_confirm().set(Some(target));
}

/// Dismiss the running-query close confirm.
pub fn close_running_close() {
    OVERLAYS.resolve().close_running_confirm().set(None);
}

/// Ask where to open `path` — This Window vs New Window (B10, when the open
/// preference is "ask"). Callable from the non-component action layer.
pub fn open_open_prompt(path: std::path::PathBuf) {
    OVERLAYS.resolve().open_prompt().set(Some(path));
}

/// Dismiss the open-target prompt.
pub fn close_open_prompt() {
    OVERLAYS.resolve().open_prompt().set(None);
}
