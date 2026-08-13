//! Machine-global app config: the recent-projects list (+ future global prefs),
//! persisted as JSON in the OS user-config dir via the `preferences` crate.
//! Distinct from a `Project` — this is per-machine, never inside a `.psproj`.

use crate::ai::Ai;
use crate::project;
use crate::theme::DEFAULT_THEME;
use crate::util;
use preferences::AppInfo;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{fs, io};

/// The app's identity in the OS config dir. `pub(crate)` because the model-listings satellite
/// ([`crate::models`]) is a second key **under the same app**, and inventing a second `AppInfo`
/// for it would put it in a directory of its own.
pub(crate) const APP_INFO: AppInfo = AppInfo {
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
// `CommandPalette` repeats the enum's name, and stays that way: the variant name *is* the wire
// format for a saved binding (the line above), so renaming it silently drops every user's
// override of that chord — and "command palette" is the surface's own name, not a stutter.
#[allow(clippy::enum_variant_names)]
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
/// `PartialEq` because the Settings window edits a **draft** copy: comparing it against the
/// seed it was made from is what "is there anything to apply?" means, and committing it is a
/// per-field diff against that same seed ([`Settings::merge_onto`], which is also why the
/// individual fields need to compare — see `strata-freya`'s `SettingsCtx`).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
    /// (session-scoped); this is the starting width the grid seeds every column from
    /// (`DataGrid::render`, clamped to [`COL_WIDTH_MIN`]/[`COL_WIDTH_MAX`]). Edited by
    /// Settings ▸ Data display.
    ///
    /// Its default is the grid's own `DEFAULT_COL_W` and must stay that way: the setting
    /// took over a hardcoded width, so a default that differs would silently re-render
    /// every user's grid at a size they never chose and have no control to undo.
    #[serde(default = "default_col_width")]
    pub default_col_width: f64,
    /// The `LIMIT` clause **generated** queries carry, so a stray `SELECT *` can't pull a
    /// whole file into memory; `0` = no limit (design24 Data-display ▸ "Default row
    /// limit"). Deliberately *not* the results page size — that is per-run and lives on
    /// `QuerySpec`. Read by the catalog's View-table action; edited by Settings ▸ Data
    /// display.
    #[serde(default = "default_row_limit")]
    pub row_limit: usize,
    /// Query-history cap (design24 System ▸ History): the newest runs kept, both in the
    /// window's satellite and in `.strata/history.jsonl`, which the load rotates down to
    /// it. The control is P4-06 (Settings ▸ System) — a numeric input, as W3 shipped it.
    ///
    /// Its default is the `HISTORY_CAP` this setting took over from and must stay that way,
    /// with more force than the others: the cap drives the **rotation**, so a lower default
    /// doesn't just show less history, it rewrites `history.jsonl` down to the smaller
    /// window on the next open — the entries in between are gone for good.
    #[serde(default = "default_max_history")]
    pub max_history: usize,
    /// Reopen the projects that had a window at last exit ([`AppConfig::open_projects`]).
    /// Read by the Freya app's startup routing, one window per project.
    #[serde(default = "default_true")]
    pub reopen_on_startup: bool,
    /// Where the folder picker starts when opening or creating a project (empty = the OS
    /// default). Read by every Open… path (`platform::pick_project_folder`).
    #[serde(default)]
    pub default_project_dir: String,
    /// Where "Open Project" opens when this window already has one. Read by the project
    /// window's open path (`platform::open`), which resolves it to that window, a new one,
    /// or the This/New prompt. The launcher never asks: it has nothing to displace.
    #[serde(default)]
    pub open_pref: OpenPref,
    #[serde(default = "default_true")]
    pub confirm_close_running: bool,
    /// Ask GitHub for a newer release when Strata starts (UP-02). Read by the Freya app's
    /// `state::updates` startup check and by nothing else: it gates only the **automatic**
    /// check, so a manual one still works with this off, and it is irrelevant outside an
    /// installed bundle, where the updater is inert whatever it says.
    ///
    /// Its Settings row and search entry are UP-03's; the field is here because it is the gate
    /// the mechanism reads.
    #[serde(default = "default_true")]
    pub check_updates: bool,
    /// User key-binding overrides (empty = all defaults). Read by `crate::keymap`.
    #[serde(default)]
    pub keybinds: Vec<KeyBind>,
    /// Curated DataFusion engine option overrides (only non-default keys), applied to
    /// each window's `SessionContext` (W2). Keyed by `datafusion.*` option name; see
    /// [`crate::engine::config`].
    #[serde(default)]
    pub engine: BTreeMap<String, String>,
    /// Agent access (AA-03): whether the in-app MCP server listens, and on what. Off by
    /// default — the capability ships dark until the user turns it on.
    #[serde(default)]
    pub agent_access: AgentAccess,
    /// The assistant (AS-03): which brains are set up, and what a new chat starts with. Empty
    /// by default — an empty roster *is* the unconfigured state, which the chat pane renders
    /// honestly rather than being told a second time by a flag.
    #[serde(default)]
    pub ai: Ai,
}

/// The agent-access server's settings (`docs/AGENT_ACCESS_SPEC.md`, "The in-app server"): one app-wide
/// Streamable-HTTP server on loopback, opt-in, bearer-authenticated.
///
/// One nested struct rather than three flat fields because they are read and written as a
/// unit — the running server is started from exactly this triple, so "does the live server
/// match the settings?" is one comparison rather than three (`agent::use_agent_server`).
///
/// The **token is empty by default and minted on first use**, not by this `Default`. A serde
/// default that minted one would mint a *fresh* one on every load of a config file that
/// lacks the field, and nothing would ever write it back — so every launch would invalidate
/// the client configuration the user had just pasted. Minting is a deliberate, persisted act;
/// see `strata_agent::server::mint_token`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AgentAccess {
    /// Whether the server listens at all. Off by default — the capability ships dark until
    /// the user turns it on.
    #[serde(default)]
    pub enabled: bool,
    /// The loopback port to bind. Fixed rather than ephemeral so a client configuration
    /// (`claude mcp add --transport http strata http://127.0.0.1:<port>/mcp`) keeps working
    /// across launches.
    #[serde(default = "default_agent_port")]
    pub port: u16,
    /// The bearer token every request must present. Empty means "not minted yet" — the
    /// server refuses an empty token outright, because the guard is a byte compare and an
    /// empty secret would match a bare `Authorization: Bearer `.
    #[serde(default)]
    pub token: String,
}

impl Default for AgentAccess {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_agent_port(),
            token: String::new(),
        }
    }
}

/// The default loopback port for agent access. Above the registered range and not a port
/// anything common claims; changeable in Settings ▸ Agent access (AA-04).
fn default_agent_port() -> u16 {
    47821
}

/// The ports Settings ▸ Agent access will accept, for the same reason the column-width and
/// history bounds are named beside their settings: the field has to offer exactly what its
/// consumer can honour.
///
/// The floor is the privileged range's ceiling plus one — binding below 1024 needs root on
/// Unix, so a field that accepted 80 would be a field whose value can only ever fail to bind.
/// The ceiling is the port number's own, stated rather than left to `u16` because the control
/// is a [`u32`] field (`NumberField`) and would otherwise offer numbers no port can be.
pub const AGENT_PORT_MIN: u16 = 1024;
pub const AGENT_PORT_MAX: u16 = u16::MAX;

/// Generates [`Settings::merge_onto`] from one list of the struct's fields.
///
/// The list is checked against the struct by the compiler: the `let Settings { … } = self`
/// pattern names every field, so adding a setting without listing it here is a **build
/// error** rather than a field the Settings window can edit and never commit. That's the
/// whole reason this is a macro and not a hand-written sequence of `if`s.
macro_rules! settings_merge {
    ($( $field:ident ),* $(,)?) => {
        impl Settings {
            /// Commit this **draft** onto the live settings, field by field: a field the draft
            /// left alone (still equal to `base`, the settings it was seeded from) keeps
            /// whatever `live` holds *now*.
            ///
            /// Why not `*live = draft.clone()`: the Settings window's draft is seeded when it
            /// opens and committed when Apply is pressed, and in between another window can
            /// commit a setting of its own — the close confirm's "Don't ask again" writes
            /// [`Settings::confirm_close_running`] from a window that never showed it. A
            /// whole-struct write would carry the draft's stale copy of that field back over
            /// the top, silently undoing a change the user did make. A per-field diff against
            /// the seed only ever commits what this draft actually changed.
            pub fn merge_onto(&self, base: &Settings, live: &mut Settings) {
                // The draft's fields, destructured rather than read through `self` — the
                // pattern is what makes the list exhaustive (see the macro's docs), and
                // binding each field means a missing one is reported by name.
                let Settings { $( $field ),* } = self;
                $(
                    if $field != &base.$field {
                        live.$field = $field.clone();
                    }
                )*
            }
        }
    };
}

settings_merge!(
    theme,
    sync_os,
    density_compact,
    zebra,
    default_col_width,
    row_limit,
    max_history,
    reopen_on_startup,
    default_project_dir,
    open_pref,
    confirm_close_running,
    check_updates,
    keybinds,
    engine,
    agent_access,
    ai,
);

/// The legal range for [`Settings::default_col_width`], in px — the bounds the results grid
/// holds every column width to, whether seeded from this setting or set by a resize drag.
///
/// Named beside the setting rather than inside the grid because the Settings ▸ Data display
/// input has to offer **exactly** the range the grid will honour: a field that accepts a width
/// the grid then silently clamps is a field that lies. The grid's own `MIN_COL_W`/`MAX_COL_W`
/// are these, so there is one definition and not two to drift apart.
pub const COL_WIDTH_MIN: f64 = 56.;
pub const COL_WIDTH_MAX: f64 = 2000.;

/// The floor for [`Settings::max_history`] — the smallest history a project can keep.
///
/// Named here for the same reason as the column-width bounds: the Settings ▸ System field has
/// to offer exactly the range its consumer honours, and the app's `history_cap` floors at this
/// number. `0` is not among them, and the reason is stronger than a lower bound usually is —
/// the cap drives the **rotation**, so a zero would have the next open rewrite
/// `history.jsonl` down to nothing.
pub const HISTORY_MIN: usize = 1;

fn default_theme() -> String {
    DEFAULT_THEME.to_string()
}
fn default_row_limit() -> usize {
    100
}
/// Matches the `HISTORY_CAP` the history satellite used before the setting was wired to it
/// (see [`Settings::max_history`] — the mismatch is destructive, not cosmetic).
fn default_max_history() -> usize {
    200
}
/// Matches the results grid's `DEFAULT_COL_W`, the width it hardcoded before the setting
/// was wired to it (see [`Settings::default_col_width`]).
fn default_col_width() -> f64 {
    168.0
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
            check_updates: true,
            keybinds: Vec::new(),
            engine: BTreeMap::new(),
            agent_access: AgentAccess::default(),
            ai: Ai::default(),
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
    /// Schema version of the file this was loaded from — the gate [`AppConfig::migrate`]
    /// dispatches its one-shot repairs on. Absent (pre-versioning) files read as `0`;
    /// [`load`] stamps [`CONFIG_VERSION`] after migrating, and the next [`save`] persists it.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub recent_projects: Vec<RecentProject>,
    /// Project folders (see [`RecentProject::path`]) with an open window right now, so
    /// "Reopen projects on startup" can restore the whole set. Maintained live —
    /// added on open, removed on any window close.
    #[serde(default)]
    pub open_projects: Vec<String>,
    /// A plain nested field — **not** `#[serde(flatten)]`: flatten is incompatible with
    /// `serde_json`'s `arbitrary_precision` (which we enable for exact decimals in JSON copies),
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

    /// Drop the recents whose project folder no longer exists on disk. Called by [`load`]
    /// on every launch — an environmental check, not a one-shot [`AppConfig::migrate`]
    /// repair, because a project can be deleted between any two runs.
    ///
    /// The test is deliberately "is a directory", not [`crate::project::exists_at`]: a
    /// folder that merely lost its `.strata/` still exists and can be opened again (and
    /// re-scaffolded), while a folder that is gone can never open — only the latter
    /// forfeits its entry. `open_projects` is left alone: it is the reopen set, which the
    /// app's startup filter checks against the stricter is-a-project test and reports
    /// what it skips.
    pub fn prune_missing(&mut self) {
        self.recent_projects.retain(|r| {
            let keep = Path::new(&r.path).is_dir();
            if !keep {
                tracing::info!("dropping recent `{}`: folder no longer exists", r.path);
            }
            keep
        });
    }

    /// Bring a loaded config up to [`CONFIG_VERSION`], then stamp it.
    ///
    /// Each repair is gated on the version of the file it came *from*, so it runs once and
    /// never again — `if self.version < N { … }`, then bump [`CONFIG_VERSION`] to `N`. Steps
    /// must stay idempotent anyway: nothing persists until the next [`save`], so a launch
    /// that never saves replays them.
    ///
    /// **The case this exists for.** `#[serde(default = "…")]` fires only when a key is
    /// *absent*, and [`Settings`] serializes every field unconditionally while `write_config`
    /// persists on every project open — so any config the app has ever written pins every
    /// setting at whatever the default was *then*. Changing a `default_*` fn therefore does
    /// not reach existing files. When a setting gains its first real consumer and that
    /// consumer previously hardcoded a different constant (`max_history` vs the history
    /// satellite's old `HISTORY_CAP`, `default_col_width` vs the grid's `DEFAULT_COL_W`),
    /// the value repair belongs *here*, not in the default — otherwise the wiring silently
    /// changes behaviour for anyone already running the app. Nothing needs that repair today
    /// (pre-release, no installs to preserve), which is the only reason v1 carries just the
    /// path rewrite.
    fn migrate(&mut self) {
        if self.version < 1 {
            self.migrate_paths();
        }
        self.version = CONFIG_VERSION;
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

/// The schema version this build writes. Bump it when you add a repair to
/// [`AppConfig::migrate`]; the new step is gated on the version it repairs *from*.
const CONFIG_VERSION: u32 = 1;

/// Where the config file lives — the path `preferences` itself would compute, resolved here
/// because the crate keeps its own version of this private and this module does its own IO
/// (see [`save`]). `AppDataType::UserConfig` plus the `.prefs.json` suffix is the whole rule,
/// and the round trip is pinned by a test.
fn config_path() -> Result<PathBuf, String> {
    let dir = app_dirs2::app_root(app_dirs2::AppDataType::UserConfig, &APP_INFO)
        .map_err(|e| format!("config dir: {e}"))?;
    Ok(dir.join(format!("{KEY}.prefs.json")))
}

/// Whether the config file may be written over.
///
/// Cleared by [`load`] when the file is there and could not be *read* — a permission the user
/// changed, a network home directory that stopped answering. Absent is not that, and neither is
/// unparseable: both of those are handled by [`load`] itself and leave writing safe.
///
/// **Why a latch and not a `Result` threaded to the nine call sites.** The rule is about the
/// file, not about any one write: once this process has failed to read it, *every* later write is
/// the same mistake — blindly replacing settings that are still on disk with the defaults this
/// process started from. A per-call answer would have to be recomputed identically at nine sites
/// and would say the same thing at all of them. It is only ever cleared, never set, so nothing
/// can turn writing back on inside a session; a restart is what re-asks the question.
static WRITABLE: AtomicBool = AtomicBool::new(true);

/// Load the app config, brought up to [`CONFIG_VERSION`] by [`AppConfig::migrate`], with the
/// recents whose folder is gone pruned ([`AppConfig::prune_missing`] — after the migration, so
/// legacy `.strata` paths are checked in their repaired shape).
///
/// **Three outcomes, not one**, which is the difference between this and the
/// `unwrap_or_default()` it replaces. That one line read *absent*, *unparseable* and *unreadable*
/// as the same thing — and because a write follows within seconds of launch (every window mount
/// claims its project through `write_config`), the defaults it returned were persisted over the
/// user's real file before they could notice. One transient read failure cost every keybind,
/// every engine override, the AI provider roster (whose `SecretRef`s then name keystore entries
/// nothing can reach again), the agent token and the recents list. `session.json` has had the
/// corrupt-vs-unreadable split and a kept-aside copy for exactly this reason; the app config,
/// which is the file that cannot be regenerated by re-running anything, had neither.
///
/// - **Absent** is the ordinary first launch: defaults, and writing is normal.
/// - **Unparseable** keeps the bytes aside as `<config>.corrupt` before returning defaults, so
///   the settings are recoverable by hand and the next write has nothing left to destroy. A
///   version this build predates lands here too, which is why the copy is taken rather than the
///   file simply replaced.
/// - **Unreadable** returns defaults for this session and latches [`WRITABLE`] off, so the file
///   on disk survives to be read by a build, or a run, that can reach it.
pub fn load() -> AppConfig {
    let mut cfg = match read_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!("{e}");
            AppConfig::default()
        }
    };
    cfg.migrate();
    cfg.prune_missing();
    cfg
}

/// Where an unparseable config is kept — the file's own name with `.corrupt` **appended**.
///
/// Appended, not `with_extension`: the file is `config.prefs.json`, whose extension is `json`, so
/// replacing it yields `config.prefs.prefs.json.corrupt` — a name that works by accident and reads
/// like a bug. `session.json.corrupt` is the shape this is following.
fn corrupt_config_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".corrupt");
    PathBuf::from(name)
}

/// [`load`]'s three-way read, split out so the outcome is a value rather than a control-flow
/// comment. `Err` is only ever the unreadable case; the other two return a config.
fn read_config() -> Result<AppConfig, String> {
    // No config dir at all: nothing to read and nowhere to write. Left writable — `save` will
    // report its own failure, which is the honest place for it.
    let path = config_path()?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(AppConfig::default()),
        Err(e) => {
            WRITABLE.store(false, Ordering::Relaxed);
            return Err(format!(
                "read config '{}': {e}. Settings are the defaults for this session and will not \
                 be saved over the file on disk",
                path.display()
            ));
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            let aside = corrupt_config_path(&path);
            // **Writing is only safe once the bytes are somewhere else.** A rename that fails
            // leaves the unparseable file as the user's only copy of their settings — and a
            // truncated config usually holds most of them verbatim — sitting at the path the
            // first window mount is about to write. So the latch goes off exactly as it does for
            // an unreadable file: this session runs on defaults and does not replace what it
            // could not preserve. `keep_corrupt_session` takes the same position for
            // `session.json`, where it can afford to fail the open outright.
            if fs::rename(&path, &aside).is_ok() {
                tracing::error!(
                    "config '{}' did not parse: {e}. Kept aside as '{}'",
                    path.display(),
                    aside.display()
                );
            } else {
                WRITABLE.store(false, Ordering::Relaxed);
                tracing::error!(
                    "config '{}' did not parse: {e}, and could not be kept aside. Settings are \
                     the defaults for this session and will not be saved over it",
                    path.display()
                );
            }
            Ok(AppConfig::default())
        }
    }
}

/// Persist the app config. The caller holds the whole [`AppConfig`], so this is a plain
/// write — never a load-mutate-save round trip, which would race the in-memory store it is
/// supposed to mirror.
///
/// **Returns its `Result`** (P4-15). It used to swallow the error into a `tracing` line, which
/// made the app's documented sole write path — `strata_freya::state::write_config` — structurally
/// incapable of knowing it had failed: not a silence any caller could fix, but one no caller could
/// even see. Reporting it is the caller's, because this crate has no user surface.
///
/// **Atomic**, through the same [`write_atomic`](crate::util::write_atomic) every `.strata/` write
/// uses — temp beside the file, fsync, rename. `preferences`' own `save` is `File::create`
/// followed by `to_writer`: the file is truncated first and the bytes land unsynced, so a kill or
/// a power loss mid-write leaves a config that parses as nothing. Two app instances writing at
/// once race the same way. Neither is hypothetical on a file written on every window mount.
pub fn save(cfg: &AppConfig) -> Result<(), String> {
    if !WRITABLE.load(Ordering::Relaxed) {
        return Err(
            "not saving config: this session could not read the file it would replace".to_string(),
        );
    }
    let path = config_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("save config: {e}"))?;
    }
    let bytes = serde_json::to_vec(cfg).map_err(|e| format!("save config: {e}"))?;
    util::write_atomic(&path, &bytes).map_err(|e| format!("save config: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::{env, fs, process};

    fn recent(name: &str, path: &str) -> RecentProject {
        RecentProject {
            name: name.into(),
            path: path.into(),
            last_opened: 0,
            pinned: false,
        }
    }

    /// A fresh temp folder standing in for projects on disk, cleaned up on drop.
    struct TempRoot(PathBuf);
    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = env::temp_dir().join(format!("strata-config-test-{tag}-{}", process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// **The file this module reads and writes is the one `preferences` used to.** Its own
    /// `compute_file_path` is private, so [`config_path`] restates the rule — and a restated rule
    /// is one that can drift. Replicated here exactly as the crate computes it (`get_app_dir`
    /// with the key, then the file name set to key + `.prefs.json`) and compared, so a change to
    /// either half fails here rather than by silently orphaning every existing user's settings.
    ///
    /// Reads no file and writes none: both sides are pure path arithmetic.
    #[test]
    fn the_config_path_is_the_one_preferences_computed() {
        let theirs = {
            let mut p = app_dirs2::get_app_dir(app_dirs2::AppDataType::UserConfig, &APP_INFO, KEY)
                .expect("a config dir");
            p.set_file_name(format!("{KEY}.prefs.json"));
            p
        };
        assert_eq!(config_path().expect("a config path"), theirs);
    }

    /// The kept-aside name is the config's own with `.corrupt` on the end — not the extension
    /// swapped, which on a `.prefs.json` gives `config.prefs.prefs.json.corrupt`.
    #[test]
    fn a_corrupt_config_is_kept_beside_the_file_it_came_from() {
        let path = PathBuf::from("/tmp/Strata/config.prefs.json");
        assert_eq!(
            corrupt_config_path(&path),
            PathBuf::from("/tmp/Strata/config.prefs.json.corrupt")
        );
    }

    #[test]
    fn recents_whose_folder_is_gone_are_pruned() {
        let root = TempRoot::new("prune");
        let alive = root.0.join("alive");
        fs::create_dir_all(&alive).unwrap();
        let gone = root.0.join("gone");
        let file = root.0.join("plain.txt");
        fs::write(&file, "not a folder").unwrap();

        let mut cfg = AppConfig {
            version: CONFIG_VERSION,
            recent_projects: vec![
                recent("alive", alive.to_str().unwrap()),
                // Deleted outright, and replaced by a plain file — neither can open, and a
                // pin doesn't save an entry whose folder is gone.
                recent("gone", gone.to_str().unwrap()),
                recent("file", file.to_str().unwrap()),
            ],
            open_projects: vec![gone.to_string_lossy().into_owned()],
            settings: Settings::default(),
        };
        cfg.recent_projects[1].pinned = true;
        cfg.prune_missing();

        let names: Vec<&str> = cfg.recent_projects.iter().map(|r| &*r.name).collect();
        assert_eq!(names, ["alive"]);
        // The reopen set is the startup filter's to validate (and report) — not pruned here.
        assert_eq!(cfg.open_projects.len(), 1);
    }

    /// Wiring a setting to a consumer that had a hardcoded constant must be
    /// behaviour-preserving: the default *is* what the app already did. Both of these took
    /// over a constant in `strata-freya` (which this crate can't name from here, hence the
    /// literals): the results grid's `DEFAULT_COL_W` and the history satellite's old
    /// `HISTORY_CAP`. Changing either of these numbers without changing the constant it
    /// mirrors silently changes behaviour for every existing user — and for `max_history`
    /// it also truncates their `history.jsonl` on the next open.
    #[test]
    fn defaults_match_the_constants_the_settings_took_over() {
        let d = Settings::default();
        assert_eq!(d.default_col_width, 168.0, "datagrid DEFAULT_COL_W");
        assert_eq!(
            d.max_history, 200,
            "the history satellite's old HISTORY_CAP"
        );
    }

    #[test]
    fn legacy_strata_dir_paths_migrate_to_project_folders() {
        let mut cfg = AppConfig {
            // A pre-versioning file: no `version` key, so it reads as 0 and the v1 gate fires.
            version: 0,
            recent_projects: vec![
                recent("sample", "/data/sample/.strata"),
                recent("events", "/data/sample/events"),
            ],
            open_projects: vec!["/data/sample/.strata".into()],
            settings: Settings::default(),
        };
        cfg.migrate();

        let paths: Vec<&str> = cfg.recent_projects.iter().map(|r| &*r.path).collect();
        // The legacy entry loses its `.strata`; the already-migrated one is untouched.
        assert_eq!(paths, ["/data/sample", "/data/sample/events"]);
        assert_eq!(cfg.open_projects, ["/data/sample"]);
        assert_eq!(cfg.version, CONFIG_VERSION, "migrating stamps the version");
    }

    /// The gate is what makes a repair one-shot: a file already at [`CONFIG_VERSION`] must not
    /// have v1 replayed over it. Proven with a path that the v1 rewrite *would* rewrite — a
    /// project legitimately named `.strata` would lose its own folder name if the step ran again.
    #[test]
    fn a_current_version_config_is_left_alone() {
        let mut cfg = AppConfig {
            version: CONFIG_VERSION,
            recent_projects: vec![recent("odd", "/data/.strata")],
            open_projects: vec!["/data/.strata".into()],
            settings: Settings::default(),
        };
        cfg.migrate();

        assert_eq!(cfg.recent_projects[0].path, "/data/.strata");
        assert_eq!(cfg.open_projects, ["/data/.strata"]);
    }

    #[test]
    fn migration_collapses_both_spellings_of_one_project() {
        let mut cfg = AppConfig {
            version: 0,
            // The same project under both spellings — the newer (first) entry wins, so a
            // re-open through the new path doesn't resurrect the stale row behind it.
            recent_projects: vec![
                recent("sample", "/data/sample"),
                recent("sample", "/data/sample/.strata"),
            ],
            open_projects: vec!["/data/sample".into(), "/data/sample/.strata".into()],
            settings: Settings::default(),
        };
        cfg.migrate();

        assert_eq!(cfg.recent_projects.len(), 1);
        assert_eq!(cfg.recent_projects[0].path, "/data/sample");
        assert_eq!(cfg.open_projects, ["/data/sample"]);
    }

    /// The reason Apply is a per-field merge and not a whole-struct write: the Settings
    /// window's draft is seeded when it opens, and another window can commit a setting of
    /// its own before Apply is pressed. Both edits have to survive.
    #[test]
    fn applying_a_draft_keeps_what_another_window_committed_meanwhile() {
        let seed = Settings::default();
        // The Settings window's draft: the user picks a theme.
        let mut draft = seed.clone();
        draft.theme = "daylight".to_string();
        // Meanwhile, the close confirm's "Don't ask again" writes this from a window that
        // never showed the setting — so the draft still holds the old value.
        let mut live = seed.clone();
        live.confirm_close_running = false;

        draft.merge_onto(&seed, &mut live);

        assert_eq!(live.theme, "daylight", "the draft's own edit commits");
        assert!(
            !live.confirm_close_running,
            "a field the draft never touched keeps the value the other window committed"
        );
    }

    /// The seed is the baseline, not the live value: a field the user edited commits even
    /// when another window happened to write that same field too. Last Apply wins, which is
    /// the only answer that doesn't silently discard what the user is looking at.
    #[test]
    fn a_field_the_draft_changed_wins_over_a_concurrent_write() {
        let seed = Settings::default();
        let mut draft = seed.clone();
        draft.row_limit = 500;
        let mut live = seed.clone();
        live.row_limit = 50;

        draft.merge_onto(&seed, &mut live);

        assert_eq!(live.row_limit, 500);
    }

    /// An untouched draft commits nothing at all — which is what makes Apply's disabled state
    /// ("the draft matches its seed") honest rather than just cosmetic.
    #[test]
    fn an_untouched_draft_commits_nothing() {
        let seed = Settings::default();
        let mut live = seed.clone();
        live.theme = "daylight".to_string();
        live.max_history = 12;

        seed.clone().merge_onto(&seed, &mut live);

        assert_eq!(live.theme, "daylight");
        assert_eq!(live.max_history, 12);
    }
}
