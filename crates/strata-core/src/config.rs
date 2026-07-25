//! Machine-global app config: the recent-projects list (+ future global prefs),
//! persisted as JSON in the OS user-config dir via the `preferences` crate.
//! Distinct from a `Project` — this is per-machine, never inside a `.psproj`.

use crate::project;
use crate::theme::DEFAULT_THEME;
use crate::util;
use preferences::{AppInfo, Preferences};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::Path;

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
    /// Absolute path to the **project folder** — the parent of its `.strata/` dir, i.e.
    /// the same root [`crate::project`] takes and `ProjectState.root` holds. What the
    /// launcher hands back to open the project.
    pub path: String,
    /// Unix epoch seconds of the last open (for display / ordering).
    pub last_opened: u64,
    /// Pinned to the top of the launcher list (B11).
    #[serde(default)]
    pub pinned: bool,
}

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

/// A logical, rebindable command — the target of a key chord. The *what* (which listener
/// acts on it) is distributed: each feature listens for its own command through
/// `crate::keymap::resolve`; this is just the stable, serializable id a binding points at.
/// Serialized by variant name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Command {
    /// Find — context-dependent (results find today).
    Find,
    NewTab,
    ReopenTab,
    CloseActiveTab,
    /// Pick a project folder and open it (File ▸ Open…).
    OpenProject,
    /// Close **this** window (red button / File ▸ Close Project). The launcher takes its
    /// place when it was the app's last window — never a quit.
    CloseProject,
    /// Quit Strata: close **every** window (⌘Q / menu Quit / dock quit). The projects with
    /// a window at that moment stay in [`AppConfig::open_projects`], so "Reopen projects on
    /// startup" restores them — which is exactly what closing them by hand does not.
    Quit,
    SaveQuery,
    RunQuery,
    /// Undo the last edit in the focused editor. Like every editing command (see
    /// [`Command::is_edit`]) it is rebindable: the effective chords are synced into the
    /// text layer's `EditBindings`, which replaced its hardcoded ⌘A/⌘C/⌘X/⌘V/⌘Z/⌘Y.
    Undo,
    /// Redo the last undone edit in the focused editor.
    Redo,
    /// Cut the selection in the focused editor.
    Cut,
    /// Copy the selection in the focused editor.
    Copy,
    /// Paste the clipboard into the focused editor.
    Paste,
    /// Select the focused editor's whole buffer.
    SelectAll,
    CommandPalette,
    OpenSettings,
    CycleWindow,
    /// Esc — dismiss an open overlay, else cancel a running query. Fixed (not rebindable).
    Cancel,
}

impl Command {
    /// Whether this command is a text-editing action — one the *focused editor* consumes
    /// (via its synced `EditBindings`) rather than a global listener. The editor's key
    /// gate lets exactly these chords through to the buffer.
    pub fn is_edit(self) -> bool {
        matches!(
            self,
            Self::Undo | Self::Redo | Self::Cut | Self::Copy | Self::Paste | Self::SelectAll
        )
    }
}

/// A normalized key chord. `primary` folds the platform primary modifier (⌘ on macOS /
/// Ctrl elsewhere), matching how `handle_key` already treats `meta || ctrl`. `key` is a
/// normalized key name (lowercased character, or `"Enter"` / `"Escape"`).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
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
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct KeyBind {
    pub command: Command,
    #[serde(default)]
    pub chord: Option<KeyChord>,
}

/// The user's settings. A plain nested field in [`AppConfig`] (a `"settings"` object in
/// the config JSON — deliberately **not** `#[serde(flatten)]`, see [`AppConfig::settings`]),
/// so it is reached through the one app-global config store rather than living in a store
/// of its own.
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
    pub engine: BTreeMap<String, String>,
}

fn default_theme() -> String {
    DEFAULT_THEME.to_string()
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
            engine: BTreeMap::new(),
        }
    }
}

/// Machine-global configuration: the recent-projects list + the user [`Settings`].
///
/// The UI holds **one reactive instance of this whole struct** for the process (the Freya
/// app's app-global config store) and persists it through [`save`]; nothing re-reads the
/// file to answer a question. [`load`] is a startup input, not a live source.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub recent_projects: Vec<RecentProject>,
    /// Project folders (see [`RecentProject::path`]) with an open window right now, so
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

    /// Rewrite project paths written by the pre-Freya app, which stored the project's
    /// `.strata` dir where we now store the project folder ([`RecentProject::path`]).
    /// Left alone, those entries name a folder that can't be opened as a project *and*
    /// shadow the same project under a second identity. Entries that collapse onto one
    /// already in the list are dropped, keeping the earlier (more recent) one.
    fn migrate_paths(&mut self) {
        for r in &mut self.recent_projects {
            r.path = project_folder(&r.path);
        }
        let mut seen = HashSet::new();
        self.recent_projects.retain(|r| seen.insert(r.path.clone()));

        for p in &mut self.open_projects {
            *p = project_folder(p);
        }
        let mut seen = HashSet::new();
        self.open_projects.retain(|p| seen.insert(p.clone()));
    }
}

/// The project folder for a stored path — the pre-Freya format's trailing `.strata`
/// segment removed. Any other path is already a project folder and passes through.
fn project_folder(path: &str) -> String {
    let p = Path::new(path);
    match (p.file_name(), p.parent()) {
        (Some(name), Some(parent)) if name == OsStr::new(project::STRATA_DIR) => {
            parent.to_string_lossy().into_owned()
        }
        _ => path.to_string(),
    }
}

/// Load the app config (empty default if missing or unreadable), with legacy project
/// paths migrated ([`AppConfig::migrate_paths`]).
pub fn load() -> AppConfig {
    let mut cfg = AppConfig::load(&APP_INFO, KEY).unwrap_or_default();
    cfg.migrate_paths();
    cfg
}

/// Persist the app config (best-effort; logged on failure). The caller holds the whole
/// [`AppConfig`], so this is a plain write — never a load-mutate-save round trip, which
/// would race the in-memory store it is supposed to mirror.
pub fn save(cfg: &AppConfig) {
    if let Err(e) = cfg.save(&APP_INFO, KEY) {
        tracing::error!("save config: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recent(name: &str, path: &str) -> RecentProject {
        RecentProject {
            name: name.into(),
            path: path.into(),
            last_opened: 0,
            pinned: false,
        }
    }

    #[test]
    fn legacy_strata_dir_paths_migrate_to_project_folders() {
        let mut cfg = AppConfig {
            recent_projects: vec![
                recent("sample", "/data/sample/.strata"),
                recent("events", "/data/sample/events"),
            ],
            open_projects: vec!["/data/sample/.strata".into()],
            settings: Settings::default(),
        };
        cfg.migrate_paths();

        let paths: Vec<&str> = cfg.recent_projects.iter().map(|r| &*r.path).collect();
        // The legacy entry loses its `.strata`; the already-migrated one is untouched.
        assert_eq!(paths, ["/data/sample", "/data/sample/events"]);
        assert_eq!(cfg.open_projects, ["/data/sample"]);
    }

    #[test]
    fn migration_collapses_both_spellings_of_one_project() {
        let mut cfg = AppConfig {
            // The same project under both spellings — the newer (first) entry wins, so a
            // re-open through the new path doesn't resurrect the stale row behind it.
            recent_projects: vec![
                recent("sample", "/data/sample"),
                recent("sample", "/data/sample/.strata"),
            ],
            open_projects: vec!["/data/sample".into(), "/data/sample/.strata".into()],
            settings: Settings::default(),
        };
        cfg.migrate_paths();

        assert_eq!(cfg.recent_projects.len(), 1);
        assert_eq!(cfg.recent_projects[0].path, "/data/sample");
        assert_eq!(cfg.open_projects, ["/data/sample"]);
    }
}
