//! Machine-global app config: the recent-projects list (+ future global prefs),
//! persisted as JSON in the OS user-config dir via the `preferences` crate.
//! Distinct from a `Project` — this is per-machine, never inside a `.psproj`.

use crate::util;
use preferences::{AppInfo, Preferences};
use serde::{Deserialize, Serialize};

const APP_INFO: AppInfo = AppInfo {
    name: "Strata",
    author: "Strata",
};
/// Key under the config dir (the `preferences` crate maps it to a file path).
const KEY: &str = "config";

/// One entry in the recent-projects list.
#[derive(Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub name: String,
    /// Absolute path to the project's `.strata` dir.
    pub path: String,
    /// Unix epoch seconds of the last open (for display / ordering).
    pub last_opened: u64,
    /// Pinned to the top of the launcher list (B11).
    #[serde(default)]
    pub pinned: bool,
}

/// The user's settings. A plain nested field in [`AppConfig`] (a `"settings"` object in the
/// config JSON — deliberately **not** `#[serde(flatten)]`, see [`AppConfig::settings`]), held at
/// runtime in the per-window [`crate::settings`] store.
/// Where "Open Project" opens a project when invoked from a window that already
/// has one: ask each time (the This/New prompt — B10), reuse this window, or a new
/// window. Serialized lowercase (`"ask"` / `"this"` / `"new"`) — matches older configs.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OpenPref {
    #[default]
    Ask,
    This,
    New,
}

/// A logical, rebindable command — the target of a key chord. The *what* (dispatch /
/// direct call / context handler) lives in `crate::keymap`; this is just the stable,
/// serializable id a binding points at. Serialized by variant name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Command {
    /// Find — context-dependent (results find today). Routed via the keymap registry.
    Find,
    NewTab,
    ReopenTab,
    CloseActiveTab,
    SaveQuery,
    RunQuery,
    CommandPalette,
    OpenSettings,
    CycleWindow,
    /// Esc — dismiss an open overlay, else cancel a running query.
    Cancel,
}

/// A normalized key chord. `primary` folds the platform primary modifier (⌘ on macOS /
/// Ctrl elsewhere), matching how `handle_key` already treats `meta || ctrl`. `key` is a
/// normalized key name (lowercased character, or `"Enter"` / `"Escape"`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyChord {
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
    pub key: String,
}

/// A user override for a [`Command`] (persisted in [`Settings::keybinds`]). A command with
/// no entry falls back to its built-in default chord; an entry with `chord: None` is an
/// **explicit unbind** (the command has no shortcut — e.g. its chord was reassigned away).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyBind {
    pub command: Command,
    #[serde(default)]
    pub chord: Option<KeyChord>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Active theme id (see `crate::theme`). Persists across sessions/windows.
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub sync_os: bool,
    #[serde(default)]
    pub density_compact: bool,
    #[serde(default = "default_true")]
    pub zebra: bool,
    /// Default results-grid column width in px (V20). Per-column overrides live on the run
    /// (session-scoped); this is the starting width. No UI control yet — struct-only.
    #[serde(default = "default_col_width")]
    pub default_col_width: f64,
    #[serde(default = "default_row_limit")]
    pub row_limit: usize,
    /// Query-history cap (design24 System ▸ History): oldest runs drop off once the count
    /// exceeds this. Surfaced as a 25/50/100/200 segmented control.
    #[serde(default = "default_max_history")]
    pub max_history: usize,
    #[serde(default = "default_true")]
    pub reopen_on_startup: bool,
    #[serde(default)]
    pub default_project_dir: String,
    #[serde(default)]
    pub open_pref: OpenPref,
    #[serde(default = "default_true")]
    pub confirm_close_running: bool,
    /// User key-binding overrides (empty = all defaults). Read by `crate::keymap`.
    #[serde(default)]
    pub keybinds: Vec<KeyBind>,
    /// Curated DataFusion engine option overrides (only non-default keys), applied to
    /// each window's `SessionContext` (W2). Keyed by `datafusion.*` option name; see
    /// [`crate::engine::config`].
    #[serde(default)]
    pub engine: std::collections::BTreeMap<String, String>,
}

/// The live theme selection projected out of the persisted settings — the input to
/// [`crate::theme::ThemeSel::effective`]. Lives here (not in `theme`) so `theme` needn't
/// know about `Settings`; every frontend derives it the same way.
impl From<&Settings> for crate::theme::ThemeSel {
    fn from(s: &Settings) -> Self {
        Self {
            id: s.theme.clone(),
            sync_os: s.sync_os,
        }
    }
}

fn default_theme() -> String {
    crate::theme::DEFAULT_THEME.to_string()
}
fn default_row_limit() -> usize {
    100
}
fn default_max_history() -> usize {
    100
}
fn default_col_width() -> f64 {
    150.0
}
fn default_true() -> bool {
    true
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            sync_os: false,
            density_compact: false,
            zebra: true,
            default_col_width: default_col_width(),
            row_limit: default_row_limit(),
            max_history: default_max_history(),
            reopen_on_startup: true,
            default_project_dir: String::new(),
            open_pref: OpenPref::Ask,
            confirm_close_running: true,
            keybinds: Vec::new(),
            engine: std::collections::BTreeMap::new(),
        }
    }
}

/// Machine-global configuration: the recent-projects list + the user [`Settings`].
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub recent_projects: Vec<RecentProject>,
    /// Paths (`.strata` dirs) of the projects with an open window right now, so
    /// "Reopen projects on startup" can restore the whole set. Maintained live —
    /// added on open, removed on any window close.
    #[serde(default)]
    pub open_projects: Vec<String>,
    /// A plain nested field — **not** `#[serde(flatten)]`: flatten is incompatible with
    /// serde_json's `arbitrary_precision` (which we enable for exact decimals in JSON copies),
    /// and a broken flatten deserialize silently reset recents + settings to defaults on load.
    #[serde(default)]
    pub settings: Settings,
}

impl AppConfig {
    /// Add or promote a project in the recents list (most-recent first, cap 12).
    pub fn push_recent(&mut self, name: &str, path: &str) {
        // Preserve the pin across a re-open (retain-then-insert would drop it).
        let pinned = self
            .recent_projects
            .iter()
            .find(|r| r.path == path)
            .map(|r| r.pinned)
            .unwrap_or(false);
        self.recent_projects.retain(|r| r.path != path);
        self.recent_projects.insert(
            0,
            RecentProject {
                name: name.to_string(),
                path: path.to_string(),
                last_opened: util::now_secs(),
                pinned,
            },
        );
        self.recent_projects.truncate(12);
    }

    /// Pin or unpin the recent at `path` (B11).
    pub fn set_pinned(&mut self, path: &str, pinned: bool) {
        if let Some(r) = self.recent_projects.iter_mut().find(|r| r.path == path) {
            r.pinned = pinned;
        }
    }

    /// Drop the recent at `path` from the list (B11 — doesn't touch the project).
    pub fn remove_recent(&mut self, path: &str) {
        self.recent_projects.retain(|r| r.path != path);
    }

    /// Record that `path` has an open window (dedup).
    pub fn add_open(&mut self, path: &str) {
        if !self.open_projects.iter().any(|p| p == path) {
            self.open_projects.push(path.to_string());
        }
    }

    /// Record that `path`'s window has closed.
    pub fn remove_open(&mut self, path: &str) {
        self.open_projects.retain(|p| p != path);
    }

    /// The most-recently-opened project, if any (used to reopen on launch).
    pub fn most_recent(&self) -> Option<&RecentProject> {
        self.recent_projects.first()
    }
}

/// Load the app config (empty default if missing or unreadable).
pub fn load() -> AppConfig {
    AppConfig::load(&APP_INFO, KEY).unwrap_or_default()
}

/// Persist the app config (best-effort; logged on failure).
pub fn save(cfg: &AppConfig) {
    if let Err(e) = cfg.save(&APP_INFO, KEY) {
        tracing::error!("save config: {e}");
    }
}

/// Record a project window opening in the persisted open-set (drives "Reopen
/// projects on startup"). Load-mutate-save so it works from any window.
pub fn mark_open(path: &str) {
    let mut cfg = load();
    cfg.add_open(path);
    save(&cfg);
}

/// Record a project window closing in the persisted open-set.
pub fn mark_closed(path: &str) {
    let mut cfg = load();
    cfg.remove_open(path);
    save(&cfg);
}