//! Theme system — JSON theme files → CSS-variable injection.
//!
//! A **theme is a JSON file** ([`Theme`]) whose colours are authored in **named
//! groups** (`surface`, `border`, `text`, `accent`, `status`, `dataType`,
//! `syntax`, `grid`) plus a `fonts` block — an author-friendly layout validated
//! by `themes/theme.schema.json`. The group names are presentational only: the
//! loader **flattens** every leaf into one token map keyed by the same names the
//! stylesheet uses as CSS variables (`bg`, `accent`, `t-str`, `syn-keyword`, …).
//! Themes come from **bundled built-ins**, a **user themes dir**, and
//! **plugin-contributed** dirs, and may `extends` a base theme (flattened token
//! maps merge onto the base). At apply time the resolved tokens are rendered to a
//! `--name: value;` string set on the app root element, overriding the stylesheet
//! `:root` defaults.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use dioxus::html::completions::CompleteWithBraces::base;

/// Light/dark grouping — used for the "Sync with OS" split and per-mode fallback.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

/// Where a theme was discovered — drives the Settings source badge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Builtin,
    User,
    Plugin,
}

/// A theme file exactly as authored: colours in named groups plus a `fonts`
/// block. Any group may be partial when `extends` is set (only the overrides need
/// listing); anything still missing after resolution is filled from the mode's
/// default built-in. `#[serde(rename = "$schema")]` lets files point their editor
/// at the schema without the loader caring.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Theme {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub extends: Option<String>,
    pub mode: Mode,
    /// Colour tokens grouped for readability — `{ "surface": { "bg": "#…" }, … }`.
    /// Group names are cosmetic; the loader flattens every leaf into the token map.
    #[serde(default)]
    pub colors: BTreeMap<String, BTreeMap<String, String>>,
    /// Font stacks (`ui`, `mono`) — flattened alongside the colours.
    #[serde(default)]
    pub fonts: BTreeMap<String, String>,
    /// Accepted + ignored so a file can carry `"$schema": "./theme.schema.json"`.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
}

impl Theme {
    /// Flatten the authored groups into the single flat token map the rest of the
    /// pipeline (`extends`, fallback, CSS render) uses. Group names are dropped;
    /// every colour leaf plus each font stack becomes a `name → value` entry.
    fn flatten(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for group in self.colors.values() {
            for (k, v) in group {
                out.insert(k.clone(), v.clone());
            }
        }
        for (k, v) in &self.fonts {
            out.insert(k.clone(), v.clone());
        }
        out
    }
}

/// A fully-resolved theme ready to apply: every required token present, and the
/// tokens pre-rendered to a CSS-variable string.
#[derive(Clone, Debug)]
pub struct ResolvedTheme {
    pub id: String,
    pub name: String,
    pub mode: Mode,
    pub source: Source,
    /// The resolved tokens (name → colour) — for anything that needs a colour as
    /// a Rust value (settings swatches, or an inline style that can't use `var`).
    pub tokens: BTreeMap<String, String>,
    /// `"--bg:#0b0e13;--panel:#0e121a;…"` — set as the root element's `style`.
    pub css: String,
}

impl ResolvedTheme {
    /// A resolved token value (`""` if absent — always present for the
    /// [`REQUIRED_TOKENS`] after resolution).
    pub fn color(&self, key: &str) -> &str {
        self.tokens.get(key).map(String::as_str).unwrap_or("")
    }
}

/// Every token the stylesheet expects. A theme missing (or mis-typing) any of
/// these has it filled from its mode's default built-in, so the app is never
/// left painting a blank variable.
pub const REQUIRED_TOKENS: &[&str] = &[
    // surfaces
    "bg",
    "panel",
    "main",
    "elev",
    "elev2",
    "elev3",
    // borders
    "line",
    "line2",
    "line3",
    "line-hi",
    // text
    "text",
    "text2",
    "text3",
    "dim",
    "dim2",
    "dim3",
    "faint",
    "faint2",
    // accents / status
    "accent",
    "accent-ink",
    "green",
    "purple",
    "red",
    "red2",
    "orange",
    "warm",
    // arrow-type colours
    "t-str",
    "t-num",
    "t-bool",
    "t-ts",
    "t-struct",
    "t-list",
    "t-map",
    // editor syntax
    "syn-keyword",
    "syn-function",
    "syn-string",
    "syn-number",
    "syn-comment",
    "syn-identifier",
    "syn-punct",
    // results grid / cells
    "cell",
    "cell-num",
    "cell-ts",
    "grid-line",
    "row-hover",
    // elevated surfaces (modals / popovers / footers) + accent tint + zebra
    "surface",
    "surface-sunk",
    "accent-soft",
    "zebra",
    // fonts (map to the `--ui` / `--mono` CSS variables)
    "ui",
    "mono",
];

/// Tokens that carry a font stack rather than a colour (skip the hex check).
fn is_font_token(key: &str) -> bool {
    matches!(key, "ui" | "mono")
}

const MIDNIGHT_JSON: &str = include_str!("../themes/midnight.json");
const DAYLIGHT_JSON: &str = include_str!("../themes/daylight.json");

static REGISTRY: OnceLock<Vec<ResolvedTheme>> = OnceLock::new();

/// The default theme id (used until Settings/prefs pick another).
pub const DEFAULT_THEME: &str = "midnight";

/// All discovered themes, resolved and ready to apply. Built-ins for now; user
/// (`<app-config>/themes/*.json`) and plugin dirs land in [`load_all`] next.
pub fn registry() -> &'static [ResolvedTheme] {
    REGISTRY.get_or_init(build_registry)
}

/// The pre-rendered CSS-variable string for a theme `id`. Empty when the id is
/// unknown, so the stylesheet `:root` defaults keep applying (never a blank UI).
pub fn css_for(id: &str) -> String {
    registry()
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.css.clone())
        .unwrap_or_default()
}

/// The user themes directory (`<app-config>/Strata/themes`). Drop a
/// `*.json` theme here to add your own.
pub fn user_themes_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let base = PathBuf::from(home);
    #[cfg(target_os = "macos")]
    let dir = base.join("Library/Application Support/Strata/themes");
    #[cfg(not(target_os = "macos"))]
    let dir = base.join(".config/Strata/themes");
    Some(dir)
}

/// The default theme id for a given system appearance (used by Sync-with-OS).
pub fn default_for(dark: bool) -> &'static str {
    if dark {
        "midnight"
    } else {
        "daylight"
    }
}

/// The theme id that should actually apply — honours Sync-with-OS.
pub fn effective_id(theme_id: &str, sync_os: bool, os_dark: bool) -> String {
    if sync_os {
        default_for(os_dark).to_string()
    } else {
        theme_id.to_string()
    }
}

/// Detect the OS dark-mode setting. macOS: `defaults read -g AppleInterfaceStyle`
/// prints `Dark` in dark mode and errors otherwise. Non-macOS defaults to dark.
pub fn os_is_dark() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("Dark"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Reveal the user themes folder in the OS file manager (creating it first).
pub fn open_user_themes_dir() {
    if let Some(dir) = user_themes_dir() {
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(&dir).spawn();
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
    }
}

/// Parse the authored theme files: bundled built-ins, then any user-authored
/// `*.json` in the user themes dir. (Plugin-contributed dirs are the next step.)
fn load_all() -> Vec<(Theme, Source)> {
    let mut out = Vec::new();
    for raw in [MIDNIGHT_JSON, DAYLIGHT_JSON] {
        match serde_json::from_str::<Theme>(raw) {
            Ok(t) => out.push((t, Source::Builtin)),
            Err(e) => tracing::error!("built-in theme parse error: {e}"),
        }
    }

    if let Some(dir) = user_themes_dir() {
        // Best-effort: make the folder so there's a place to drop themes.
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("json"))
                    .unwrap_or(false)
                {
                    match std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<Theme>(&s).ok())
                    {
                        Some(t) => out.push((t, Source::User)),
                        None => tracing::warn!("skipping unreadable theme {}", path.display()),
                    }
                }
            }
        }
    }
    out
}

fn build_registry() -> Vec<ResolvedTheme> {
    let authored = load_all();

    // Flattened token maps by id — for `extends` resolution.
    let by_id: BTreeMap<String, BTreeMap<String, String>> = authored
        .iter()
        .map(|(t, _)| (t.id.clone(), t.flatten()))
        .collect();

    // Per-mode fallback = the matching built-in's flattened tokens.
    let fallback = |mode: Mode| -> BTreeMap<String, String> {
        authored
            .iter()
            .find(|(t, s)| *s == Source::Builtin && t.mode == mode)
            .map(|(t, _)| t.flatten())
            .unwrap_or_default()
    };

    authored
        .iter()
        .map(|(t, source)| {
            // Start from the `extends` base (if any), then overlay own tokens.
            let mut tokens: BTreeMap<String, String> = t
                .extends
                .as_ref()
                .and_then(|base| by_id.get(base).cloned())
                .unwrap_or_default();
            for (k, v) in t.flatten() {
                tokens.insert(k, v);
            }
            // Fill any missing / invalid required token from the mode default.
            let fb = fallback(t.mode);
            for key in REQUIRED_TOKENS {
                let ok = tokens
                    .get(*key)
                    .map(|v| {
                        if is_font_token(key) {
                            !v.trim().is_empty()
                        } else {
                            is_color(v)
                        }
                    })
                    .unwrap_or(false);
                if !ok {
                    if let Some(v) = fb.get(*key) {
                        tokens.insert((*key).to_string(), v.clone());
                    } else {
                        tracing::warn!("theme '{}' missing token '{key}'", t.id);
                    }
                }
            }
            let css = REQUIRED_TOKENS
                .iter()
                .filter_map(|k| tokens.get(*k).map(|v| format!("--{k}:{v};")))
                .collect::<String>();
            ResolvedTheme {
                id: t.id.clone(),
                name: t.name.clone(),
                mode: t.mode,
                source: *source,
                tokens,
                css,
            }
        })
        .collect()
}

/// Loose colour check — accepts `#rgb` / `#rgba` / `#rrggbb` / `#rrggbbaa` and
/// lets `rgb()/rgba()/hsl()` through (the webview validates at paint time).
fn is_color(v: &str) -> bool {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix('#') {
        matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
    } else {
        v.starts_with("rgb") || v.starts_with("hsl")
    }
}
