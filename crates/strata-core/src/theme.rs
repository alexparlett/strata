//! The Strata theme **data model** — the native JSON theme format (`themes/*.json`),
//! framework-agnostic.
//!
//! Midnight/Daylight are built-ins (embedded); custom themes load the same shape from a
//! plugin dir (roadmap). A theme file has: a `sheet` copied 1:1 into the frontend's colour
//! palette (Freya's `ColorsSheet`), a `components` map of per-component overrides keyed by
//! component key, `tokens` for our own not-yet-built components, `fonts`, and a top-level
//! `typography` type scale. Each component field is a tagged [`Pref`] — `{ "specific": … }`
//! or `{ "reference": "<sheet slot>" }`.
//!
//! This module owns the authored shapes, the [`ThemeRegistry`] (discovery over the embedded
//! built-ins + the user themes dir, with [`Source`] badges and id lookup), the resolved
//! [`Typography`] scale, the Sync-with-OS selection helpers, and the JSON-schema generator
//! ([`generate_schema`], parameterized over the frontend's component registries). Everything
//! Freya-specific — coercing [`Pref`]s into `Preference<Color>`s, the component registries
//! themselves, schema sync — lives in `strata-freya`'s `theme` module.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MIDNIGHT_JSON: &str = include_str!("../../../themes/midnight.json");
const DAYLIGHT_JSON: &str = include_str!("../../../themes/daylight.json");

/// The default theme id (used until Settings/prefs pick another).
pub const DEFAULT_THEME: &str = "midnight";

/// The 27 `ColorsSheet` slot names — reference targets + the required sheet keys.
pub const SLOTS: &[&str] = &[
    "primary", "secondary", "tertiary", "success", "warning", "error", "info",
    "background", "surface_primary", "surface_secondary", "surface_tertiary",
    "surface_inverse", "surface_inverse_secondary", "surface_inverse_tertiary",
    "border", "border_focus", "border_disabled",
    "text_primary", "text_secondary", "text_placeholder", "text_inverse", "text_highlight",
    "focus", "active", "disabled", "overlay", "shadow",
];

/// Light/dark grouping — picks the frontend's base theme and (later) the Sync-with-OS split.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

/// A theme file exactly as authored.
#[derive(Deserialize)]
pub struct StrataTheme {
    pub id: String,
    pub name: String,
    pub mode: Mode,
    pub sheet: SheetDef,
    #[serde(default)]
    pub components: BTreeMap<String, BTreeMap<String, Pref>>,
    #[serde(default)]
    pub tokens: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub fonts: BTreeMap<String, String>,
    /// The type scale — named roles (display · title · body · meta · …), each fixing a font
    /// family (`ui`/`mono`, resolved via `fonts`), weight and size (+ optional line-height /
    /// letter-spacing). A **top-level** section (not a `components` entry): its fields are
    /// `TypeRole` objects, not the colour `Pref`s every `components.*` map holds. Resolved
    /// into a [`Typography`] by [`resolve_typography`].
    #[serde(default)]
    pub typography: BTreeMap<String, TypeRole>,
}

/// One authored typography role from the theme file. `family` is a `fonts` key (`ui`/`mono`);
/// `weight`/`size` are required; `line_height`/`letter_spacing` are optional.
#[derive(Deserialize, Clone)]
pub struct TypeRole {
    pub family: String,
    pub weight: i32,
    pub size: f32,
    #[serde(default)]
    pub line_height: Option<f32>,
    #[serde(default)]
    pub letter_spacing: Option<f32>,
}

/// A component field override — the `specific` / `reference` discriminated union.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pref {
    Specific(SpecificValue),
    Reference(String),
}

/// The payload of a `specific` — a colour/font string, a scalar, or four gap sides (distinct
/// JSON types). Deserialized by hand through `serde_json::Value` rather than `#[serde(untagged)]`:
/// untagged buffering breaks on non-integer numbers when serde_json's `arbitrary_precision`
/// feature is enabled anywhere in the workspace (this crate enables it), and `Value` handles it
/// natively.
pub enum SpecificValue {
    Color(String),
    Scalar(f32),
    Sides([f32; 4]),
}

impl<'de> Deserialize<'de> for SpecificValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(s) => Ok(Self::Color(s)),
            serde_json::Value::Number(n) => {
                Ok(Self::Scalar(n.as_f64().ok_or_else(|| {
                    D::Error::custom("specific number out of range")
                })? as f32))
            }
            serde_json::Value::Array(a) => {
                let sides: Vec<f32> = a
                    .iter()
                    .map(|v| v.as_f64().map(|n| n as f32))
                    .collect::<Option<_>>()
                    .ok_or_else(|| D::Error::custom("specific sides must be numbers"))?;
                let sides: [f32; 4] = sides.try_into().map_err(|_| {
                    D::Error::custom("specific sides must have exactly 4 numbers")
                })?;
                Ok(Self::Sides(sides))
            }
            _ => Err(D::Error::custom(
                "specific must be a colour/font string, a number, or a 4-number array",
            )),
        }
    }
}

/// A component field's value type — drives both the frontend's runtime coercion and the schema.
#[derive(Clone, Copy)]
pub enum Kind {
    Color,
    F32,
    I32,
    Gaps,
    Corner,
    /// A font family: a `fonts` key (`ui`/`mono`) resolved to the real family name, or a
    /// literal family name.
    Font,
}

/// The 27 fields of Freya's `ColorsSheet`, as authored colour strings.
#[derive(Deserialize)]
pub struct SheetDef {
    pub primary: String,
    pub secondary: String,
    pub tertiary: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    pub info: String,
    pub background: String,
    pub surface_primary: String,
    pub surface_secondary: String,
    pub surface_tertiary: String,
    pub surface_inverse: String,
    pub surface_inverse_secondary: String,
    pub surface_inverse_tertiary: String,
    pub border: String,
    pub border_focus: String,
    pub border_disabled: String,
    pub text_primary: String,
    pub text_secondary: String,
    pub text_placeholder: String,
    pub text_inverse: String,
    pub text_highlight: String,
    pub focus: String,
    pub active: String,
    pub disabled: String,
    pub overlay: String,
    pub shadow: String,
}

/// Load an embedded **built-in** theme by id ("midnight" / "daylight"), defaulting to
/// Midnight. This is the always-available floor (used by [`typography`]'s defensive
/// fallback and the theme tests); real theme resolution goes through the [`ThemeRegistry`],
/// which also discovers user-authored themes.
pub fn load(id: &str) -> StrataTheme {
    let json = match id {
        "daylight" => DAYLIGHT_JSON,
        _ => MIDNIGHT_JSON,
    };
    serde_json::from_str(json).expect("strata theme json")
}

/// Where a theme was discovered — drives the Settings source badge. (Plugin-contributed
/// dirs are roadmap; the variant lands with them.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Builtin,
    User,
}

/// One discovered theme + where it came from.
pub struct ThemeEntry {
    pub theme: StrataTheme,
    pub source: Source,
}

/// Every discovered theme: the embedded built-ins plus any user-authored `*.json` in the
/// user themes dir. Discovered **once** at launch (see `strata-freya`'s `main`) and shared
/// by every window/app; entries keep discovery order (built-ins first, then user files by
/// filename), and a user theme whose `id` matches an existing entry **replaces** it in
/// place — that's how you retune a built-in by dropping a `midnight.json` in the dir.
pub struct ThemeRegistry {
    entries: Vec<ThemeEntry>,
}

impl ThemeRegistry {
    /// Discover the registry: built-ins + the user themes dir (created best-effort so
    /// there's always a place to drop themes).
    pub fn discover() -> Self {
        let dirs: Vec<PathBuf> = user_themes_dir().into_iter().collect();
        for dir in &dirs {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::with_dirs(&dirs)
    }

    /// Build from the built-ins plus the given theme dirs (the testable core of
    /// [`discover`](Self::discover)). Unreadable/invalid files are skipped with a warning —
    /// a broken user theme must never take the app down.
    pub fn with_dirs(dirs: &[PathBuf]) -> Self {
        let mut entries: Vec<ThemeEntry> = [MIDNIGHT_JSON, DAYLIGHT_JSON]
            .iter()
            .map(|raw| ThemeEntry {
                theme: serde_json::from_str(raw).expect("built-in theme json"),
                source: Source::Builtin,
            })
            .collect();
        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let mut paths: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .map(|e| e.eq_ignore_ascii_case("json"))
                        .unwrap_or(false)
                })
                .collect();
            paths.sort();
            for path in paths {
                match parse_theme_file(&path) {
                    Ok(theme) => upsert(&mut entries, theme, Source::User),
                    Err(e) => tracing::warn!("skipping theme {}: {e}", path.display()),
                }
            }
        }
        Self { entries }
    }

    /// Every discovered theme, in display order — for the Settings theme list.
    pub fn entries(&self) -> &[ThemeEntry] {
        &self.entries
    }

    /// The theme with this id, if discovered.
    pub fn get(&self, id: &str) -> Option<&StrataTheme> {
        self.entries
            .iter()
            .find(|e| e.theme.id == id)
            .map(|e| &e.theme)
    }

    /// The theme with this id, falling back to [`DEFAULT_THEME`] — a stale persisted id
    /// (e.g. a deleted user theme) must still paint a real theme.
    pub fn get_or_default(&self, id: &str) -> &StrataTheme {
        self.get(id)
            .or_else(|| self.get(DEFAULT_THEME))
            .expect("built-in default theme always present")
    }
}

fn parse_theme_file(path: &Path) -> Result<StrataTheme, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

/// Insert a discovered theme: same-id replaces in place (keeps display position), new ids
/// append.
fn upsert(entries: &mut Vec<ThemeEntry>, theme: StrataTheme, source: Source) {
    match entries.iter_mut().find(|e| e.theme.id == theme.id) {
        Some(e) => *e = ThemeEntry { theme, source },
        None => entries.push(ThemeEntry { theme, source }),
    }
}

/// The user themes directory (`<app-config>/Strata/themes`). Drop a `*.json` theme here to
/// add your own (or override a built-in by reusing its id).
pub fn user_themes_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let base = PathBuf::from(home);
    #[cfg(target_os = "macos")]
    let dir = base.join("Library/Application Support/Strata/themes");
    #[cfg(not(target_os = "macos"))]
    let dir = base.join(".config/Strata/themes");
    Some(dir)
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

/// A resolved typography role, ready to paint: the **actual** font family name (looked up from
/// `fonts`), plus the role's weight, size and optional line-height / letter-spacing.
#[derive(Clone, PartialEq)]
pub struct TextStyle {
    pub family: String,
    pub weight: i32,
    pub size: f32,
    pub line_height: Option<f32>,
    pub letter_spacing: Option<f32>,
}

/// The resolved type scale for a theme — one [`TextStyle`] per role. Field names mirror the
/// theme file's `typography.<role>` keys (and [`generate_schema`]'s `TYPE_ROLES`).
#[derive(Clone, PartialEq)]
pub struct Typography {
    pub display: TextStyle,
    pub title: TextStyle,
    pub strong_body: TextStyle,
    pub body_medium: TextStyle,
    pub control: TextStyle,
    pub body: TextStyle,
    pub caption: TextStyle,
    pub code_display: TextStyle,
    pub data_display: TextStyle,
    pub data_value: TextStyle,
    pub code_block: TextStyle,
    pub field_label: TextStyle,
    pub meta: TextStyle,
    pub mono_path: TextStyle,
}

/// Load + resolve the [`Typography`] scale for a theme id — [`load`] + [`resolve_typography`].
pub fn typography(id: &str) -> Typography {
    resolve_typography(&load(id))
}

/// Resolve the scale from an already-loaded theme — each role's `family` key (`ui`/`mono`) looked up
/// in `fonts` to the real family name. A role the file omits falls back to a neutral 13px UI style
/// so text still renders (the theme owns the scale).
pub fn resolve_typography(t: &StrataTheme) -> Typography {
    let fam = |key: &str| -> String {
        t.fonts
         .get(key)
         .cloned()
         .unwrap_or_else(|| "IBM Plex Sans".to_string())
    };
    let role = |name: &str| -> TextStyle {
        match t.typography.get(name) {
            Some(r) => TextStyle {
                family: fam(&r.family),
                weight: r.weight,
                size: r.size,
                line_height: r.line_height,
                letter_spacing: r.letter_spacing,
            },
            None => TextStyle {
                family: fam("ui"),
                weight: 400,
                size: 13.0,
                line_height: None,
                letter_spacing: None,
            },
        }
    };
    Typography {
        display: role("display"),
        title: role("title"),
        strong_body: role("strong_body"),
        body_medium: role("body_medium"),
        control: role("control"),
        body: role("body"),
        caption: role("caption"),
        code_display: role("code_display"),
        data_display: role("data_display"),
        data_value: role("data_value"),
        code_block: role("code_block"),
        field_label: role("field_label"),
        meta: role("meta"),
        mono_path: role("mono_path"),
    }
}

/// Build the JSON schema for the theme format: the fixed model (sheet slots, fonts, tokens,
/// typography roles) plus the frontend's themeable components — `component_registries` is a
/// set of `(component key, fields + kinds)` tables (e.g. Freya's builtin-override registry
/// and its custom-component registry). The frontend's `schema_in_sync` test keeps
/// `themes/theme.schema.json` equal to this.
pub fn generate_schema(
    component_registries: &[&[(&str, &[(&str, Kind)])]],
) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let ref_for = |k: &Kind| match k {
        Kind::Color => "#/$defs/colorPref",
        Kind::F32 | Kind::I32 | Kind::Corner => "#/$defs/numberPref",
        Kind::Gaps => "#/$defs/gapsPref",
        Kind::Font => "#/$defs/fontPref",
    };

    let mut components = Map::new();
    for (key, fields) in component_registries.iter().flat_map(|r| r.iter()) {
        let mut props = Map::new();
        for (name, kind) in *fields {
            props.insert((*name).to_string(), json!({ "$ref": ref_for(kind) }));
        }
        components.insert(
            (*key).to_string(),
            json!({ "type": "object", "additionalProperties": false, "properties": Value::Object(props) }),
        );
    }

    let mut sheet_props = Map::new();
    for s in SLOTS {
        sheet_props.insert((*s).to_string(), json!({ "$ref": "#/$defs/color" }));
    }
    let slots = serde_json::to_value(SLOTS).unwrap();

    // The type scale — a top-level `typography` section, one `typeRole` per named role.
    const TYPE_ROLES: &[&str] = &[
        "display", "title", "strong_body", "body_medium", "control", "body", "caption",
        "code_display", "data_display", "data_value", "code_block", "field_label", "meta",
        "mono_path",
    ];
    let mut typo_props = Map::new();
    for r in TYPE_ROLES {
        typo_props.insert((*r).to_string(), json!({ "$ref": "#/$defs/typeRole" }));
    }

    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "https://strata.dev/schemas/freya-theme.schema.json",
        "title": "Strata (Freya) theme",
        "type": "object",
        "required": ["id", "name", "mode", "sheet"],
        "additionalProperties": false,
        "properties": {
            "$schema": { "type": "string" },
            "id": { "type": "string" },
            "name": { "type": "string" },
            "author": { "type": "string" },
            "mode": { "enum": ["dark", "light"] },
            "sheet": { "$ref": "#/$defs/sheet" },
            "components": { "type": "object", "additionalProperties": false, "properties": Value::Object(components) },
            "tokens": { "type": "object", "additionalProperties": { "type": "object", "additionalProperties": { "$ref": "#/$defs/color" } } },
            "fonts": { "type": "object", "properties": { "ui": { "type": "string" }, "mono": { "type": "string" } }, "additionalProperties": { "type": "string" } },
            "typography": { "type": "object", "additionalProperties": false, "properties": Value::Object(typo_props) }
        },
        "$defs": {
            "color": { "type": "string", "pattern": "^(#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?|rgba\\([^)]*\\))$" },
            "slot": { "enum": slots.clone() },
            "colorPref": { "oneOf": [
                { "type": "object", "required": ["specific"], "additionalProperties": false, "properties": { "specific": { "$ref": "#/$defs/color" } } },
                { "type": "object", "required": ["reference"], "additionalProperties": false, "properties": { "reference": { "$ref": "#/$defs/slot" } } }
            ] },
            "numberPref": { "type": "object", "required": ["specific"], "additionalProperties": false, "properties": { "specific": { "type": "number" } } },
            "fontPref": { "type": "object", "required": ["specific"], "additionalProperties": false, "properties": { "specific": { "type": "string", "description": "A fonts key (ui/mono) or a literal family name" } } },
            "gapsPref": { "type": "object", "required": ["specific"], "additionalProperties": false, "properties": { "specific": { "oneOf": [ { "type": "number" }, { "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4 } ] } } },
            "sheet": { "type": "object", "additionalProperties": false, "required": slots, "properties": Value::Object(sheet_props) },
            "typeRole": { "type": "object", "required": ["family", "weight", "size"], "additionalProperties": false, "properties": {
                "family": { "type": "string", "enum": ["ui", "mono"] },
                "weight": { "type": "number" },
                "size": { "type": "number" },
                "line_height": { "type": "number" },
                "letter_spacing": { "type": "number" }
            } }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty scratch dir under the OS temp dir (no tempfile dep for two tests).
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "strata-theme-registry-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn registry_discovers_builtins_and_falls_back() {
        let reg = ThemeRegistry::with_dirs(&[]);
        let ids: Vec<&str> = reg.entries().iter().map(|e| e.theme.id.as_str()).collect();
        assert_eq!(ids, ["midnight", "daylight"]);
        assert!(reg.entries().iter().all(|e| e.source == Source::Builtin));
        assert_eq!(reg.get_or_default("no-such-theme").id, DEFAULT_THEME);
    }

    #[test]
    fn registry_user_dir_adds_overrides_and_skips_broken() {
        let dir = scratch_dir("user");
        // A new user theme: the midnight file under a fresh id.
        let custom = MIDNIGHT_JSON.replace(r#""id": "midnight""#, r#""id": "custom""#);
        assert_ne!(custom, MIDNIGHT_JSON, "id marker must match the fixture");
        std::fs::write(dir.join("custom.json"), custom).unwrap();
        // An override: a user file reusing the built-in id replaces it in place.
        let renamed = MIDNIGHT_JSON.replace(r#""name": "Midnight""#, r#""name": "My Midnight""#);
        assert_ne!(renamed, MIDNIGHT_JSON, "name marker must match the fixture");
        std::fs::write(dir.join("midnight-tweak.json"), renamed).unwrap();
        // Broken files are skipped, never fatal.
        std::fs::write(dir.join("broken.json"), "{ not json").unwrap();

        let reg = ThemeRegistry::with_dirs(std::slice::from_ref(&dir));
        let ids: Vec<&str> = reg.entries().iter().map(|e| e.theme.id.as_str()).collect();
        assert_eq!(ids, ["midnight", "daylight", "custom"]);
        assert_eq!(reg.get("midnight").unwrap().name, "My Midnight");
        assert_eq!(reg.entries()[0].source, Source::User, "override rebadges the entry");
        assert_eq!(reg.entries()[2].source, Source::User);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
