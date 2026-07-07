//! Machine-global app config: the recent-projects list (+ future global prefs),
//! persisted as JSON in the OS user-config dir via the `preferences` crate.
//! Distinct from a `Project` — this is per-machine, never inside a `.psproj`.

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

/// The user's settings. Persisted **flat** inside [`AppConfig`] via
/// `#[serde(flatten)]` (so existing config files stay compatible), and held at
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

#[derive(Clone, Serialize, Deserialize, dioxus_stores::Store)]
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
    #[serde(default = "default_row_limit")]
    pub row_limit: usize,
    #[serde(default = "default_true")]
    pub reopen_on_startup: bool,
    #[serde(default)]
    pub default_project_dir: String,
    #[serde(default)]
    pub open_pref: OpenPref,
    #[serde(default = "default_true")]
    pub confirm_close_running: bool,
}

fn default_theme() -> String {
    crate::theme::DEFAULT_THEME.to_string()
}
fn default_row_limit() -> usize {
    100
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
            row_limit: default_row_limit(),
            reopen_on_startup: true,
            default_project_dir: String::new(),
            open_pref: OpenPref::Ask,
            confirm_close_running: true,
        }
    }
}

/// Machine-global configuration: the recent-projects list + the user [`Settings`].
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub recent_projects: Vec<RecentProject>,
    #[serde(flatten)]
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
                last_opened: now_secs(),
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

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
