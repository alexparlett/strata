//! The Strata theme **data model** — the native JSON theme format (`themes/*.json`),
//! framework-agnostic.
//!
//! Midnight/Daylight are built-ins (embedded); custom themes load the same shape from a
//! plugin dir (roadmap). A theme file has: a `sheet` copied 1:1 into the frontend's core colour
//! slots (Freya's `ColorsSheet`), a `palette` of app-named slots extending it, a `components`
//! map of per-component overrides keyed by component key, `fonts`, and a top-level `typography`
//! type scale. Each component field is a tagged [`Pref`] — `{ "specific": … }` or
//! `{ "reference": "<slot>" }`, where a slot is a sheet slot **or** a `palette` key.
//!
//! This module owns the authored shapes, the [`ThemeRegistry`] (discovery over the embedded
//! built-ins + the user themes dir, with [`Source`] badges and id lookup), the resolved
//! [`Typography`] scale, the Sync-with-OS selection helpers, and the JSON-schema generator
//! ([`generate_schema`], parameterized over the frontend's component registries). Everything
//! Freya-specific — coercing [`Pref`]s into `Preference<Color>`s, the component registries
//! themselves, schema sync — lives in `strata-freya`'s `theme` module.

use serde::{Deserialize, Deserializer};
use serde_json::from_str;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MIDNIGHT_JSON: &str = include_str!("../../../themes/midnight.json");
const DAYLIGHT_JSON: &str = include_str!("../../../themes/daylight.json");

/// The default theme id (used until Settings/prefs pick another).
pub const DEFAULT_THEME: &str = "midnight";

/// What an omitted role resolves to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fallback {
    /// The role is required: a file that omits it is warned at discovery and paints magenta.
    Required,
    /// Omitted ⇒ read this role's value instead.
    Role(Role),
    /// Omitted ⇒ fully transparent.
    Transparent,
}

macro_rules! role_fallback {
    () => {
        Fallback::Required
    };
    (transparent) => {
        Fallback::Transparent
    };
    ($role:ident) => {
        Fallback::Role(Role::$role)
    };
}

/// One table generates the enum, the dotted names, the lookup and the fallback rules — so a role
/// cannot exist without a name, nor a fallback point at a role that doesn't.
macro_rules! roles {
    ($( $(#[$doc:meta])* $variant:ident => $name:literal $(( or $fb:tt ))? ),* $(,)?) => {
        /// One role of the theme vocabulary — the closed set of names a theme file's `roles` map
        /// may author, and the only names the frontend's component mapping may reference.
        /// Ordinals are stable within a build (the frontend indexes a resolved array by them).
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Role {
            $( $(#[$doc])* $variant ),*
        }

        impl Role {
            /// Every role, in declaration (family) order.
            pub const ALL: &'static [Role] = &[ $( Role::$variant ),* ];
            pub const COUNT: usize = Role::ALL.len();

            /// The dotted name this role has in a theme file's `roles` map.
            pub fn name(self) -> &'static str {
                match self { $( Role::$variant => $name ),* }
            }

            /// The role a dotted name denotes, if it is one.
            pub fn from_name(name: &str) -> Option<Role> {
                match name { $( $name => Some(Role::$variant), )* _ => None }
            }

            /// What an authored file may omit for this role, and what the omission reads as.
            /// Fallbacks are always "read this other named role" — never a computed tint, which
            /// no shipping theme format does either.
            pub fn fallback(self) -> Fallback {
                match self { $( Role::$variant => role_fallback!($($fb)?) ),* }
            }
        }
    };
}

roles! {
    // ---- Surfaces: elevation tiers, not widget names --------------------------------------
    /// The app base coat: the window body, the active tab's well behind everything.
    Background => "background",
    /// The standard panel surface: sidebars, tab bar, status bar, grid body, input wells, cards.
    SurfaceBackground => "surface.background",
    /// One step up: drawer, inspector, chart canvas, settings/launcher body, title bar.
    SurfaceRaised => "surface.raised",
    /// Below base: the EXPLAIN plan canvas.
    SurfaceSunken => "surface.sunken" (or Background),
    /// A barely-raised quiet box: insight panels, the faintest tint washes.
    SurfaceSubtle => "surface.subtle",
    /// The grid's zebra-row tint, painted translucent over `surface.background`.
    SurfaceStripe => "surface.stripe" (or transparent),
    /// Floating chrome: menus, popups, tooltips, the command palette, modal cards.
    ElevatedSurface => "elevated_surface.background",
    /// The scrim behind modals and the palette.
    Backdrop => "backdrop",
    /// Drop shadow of floating chrome.
    Shadow => "shadow",

    // ---- Location refinements: a place, not a widget, that may leave its tier --------------
    /// Drawer / inspector body, when a theme wants panels off the raised tier.
    PanelBackground => "panel.background" (or SurfaceRaised),
    /// The editor tab strip's container.
    TabBarBackground => "tab_bar.background" (or SurfaceBackground),
    /// The results-pane footer.
    StatusBarBackground => "status_bar.background" (or SurfaceBackground),
    /// The header bar.
    TitleBarBackground => "title_bar.background" (or SurfaceRaised),

    // ---- Interactive elements: filled controls, flush controls, and the odd fills ----------
    /// Filled-control rest fill: buttons, select triggers, segmented containers, grid headers.
    ElementBackground => "element.background",
    /// Filled-control hover fill (also the strong hover of icon flat-buttons).
    ElementHover => "element.hover",
    /// Filled-control pressed fill.
    ElementActive => "element.active",
    /// Neutral selected fill: a menu's checked item, the active grid gutter/header.
    ElementSelected => "element.selected",
    /// Disabled filled-control fill.
    ElementDisabled => "element.disabled",
    /// Flush-control rest fill (transparent in both built-ins, themeable).
    GhostElementBackground => "ghost_element.background",
    /// Flush hover wash: tabs, sidebar rows, drawer rows, segments.
    GhostElementHover => "ghost_element.hover",
    /// Flush pressed fill.
    GhostElementActive => "ghost_element.active",
    /// Flush neutral selected fill: the active tab pill, the selected sidebar row.
    GhostElementSelected => "ghost_element.selected",
    /// Hover wash for items on elevated/filled bases (menu items, select options, completion
    /// rows, card hover) — authored translucent so one value works on every base.
    ElevatedElementHover => "elevated_element.hover",
    /// Data-row hover (grid cells, table rows) — hue-distinct from control hover in Daylight.
    ListHover => "list.hover",
    /// Drag-and-drop placeholder fill (the tab drag slot).
    DropTarget => "drop_target.background",
    /// Progress/slider channel fill.
    Track => "track",
    /// The light control knob: switch thumbs, the checkbox check mark.
    Knob => "knob",

    // ---- Borders ---------------------------------------------------------------------------
    /// Standard hairline: panel dividers, pane rules.
    Border => "border",
    /// Fainter hairline: in-list row rules, grid row dividers, tree guides.
    BorderVariant => "border.variant",
    /// Control outline: buttons, inputs, chips, boxed tables.
    BorderControl => "border.control",
    /// Emphasized outline: hovered cards, the keymap's dashed empty slot.
    BorderStrong => "border.strong",
    /// Focus ring.
    BorderFocused => "border.focused",
    /// Selected-card/chip outline.
    BorderSelected => "border.selected" (or BorderFocused),
    /// Disabled control outline.
    BorderDisabled => "border.disabled" (or BorderVariant),
    /// Edge of floating chrome; also checkbox/radio rest outline and the switch track.
    BorderOverlay => "border.overlay",

    // ---- Text (icons read these too; an `icon.*` family is the named escape if one ever
    //      needs to differ) ------------------------------------------------------------------
    /// Primary content and headings.
    Text => "text",
    /// Secondary body: labels, values, legends, row text.
    TextMuted => "text.muted",
    /// Control labels at rest (buttons, segment items, status-bar controls).
    TextControl => "text.control",
    /// Recessive chrome text: status bar, flat buttons at rest, tab labels, empty states.
    TextDim => "text.dim",
    /// Uppercase eyebrow/field labels.
    TextLabel => "text.label",
    /// Placeholders, hints, chevrons, line numbers.
    TextPlaceholder => "text.placeholder",
    /// Disabled/meta text: timestamps, tallies, null tiles.
    TextDisabled => "text.disabled",
    /// Emphasized/link text.
    TextAccent => "text.accent" (or Accent),
    /// Text and glyphs on an accent or status fill.
    TextOnAccent => "text.on_accent",

    // ---- Accent ----------------------------------------------------------------------------
    /// The brand accent: filled buttons, selection marks, links, cursors.
    Accent => "accent",
    /// Filled-accent hover.
    AccentHover => "accent.hover",
    /// Focus ring on an accent-filled control.
    AccentRing => "accent.ring" (or Accent),
    /// The ~12% accent wash: selected rows/pills/cards, nav pills, the palette's active row.
    AccentSelection => "accent.selection",
    /// The stronger ~22% wash: toggle-button active, the form reveal flash.
    AccentMuted => "accent.muted",
    /// The ~12% badge/icon-chip fill.
    AccentBadge => "accent.badge",

    // ---- Status: one global triad per semantic (error also carries a hover, for the two
    //      live controls dressed in it) -------------------------------------------------------
    /// The error tone.
    Error => "error",
    /// The tinted error fill.
    ErrorBackground => "error.background",
    /// The tinted error fill, hovered (Cancel, Run-while-running).
    ErrorBackgroundHover => "error.background.hover",
    /// The tinted error outline.
    ErrorBorder => "error.border",
    /// The warning tone.
    Warning => "warning",
    /// The tinted warning fill.
    WarningBackground => "warning.background",
    /// The tinted warning outline.
    WarningBorder => "warning.border",
    /// The success tone.
    Success => "success",
    /// The tinted success fill.
    SuccessBackground => "success.background",
    /// The tinted success outline.
    SuccessBorder => "success.border",
    /// The info tone.
    Info => "info",
    /// The tinted info fill.
    InfoBackground => "info.background",
    /// The tinted info outline.
    InfoBorder => "info.border",

    // ---- Editor chrome (syntax is the separate `syntax` section) ---------------------------
    /// The code editor well — its own role because the built-ins genuinely put it on
    /// different tiers.
    EditorBackground => "editor.background",
    /// Gutter numbers at rest.
    EditorLineNumber => "editor.line_number",
    /// The cursor line's gutter number.
    EditorActiveLineNumber => "editor.active_line_number",
    /// Text-selection wash.
    EditorSelection => "editor.selection",
    /// The caret.
    EditorCursor => "editor.cursor" (or Accent),

    // ---- Scrollbar -------------------------------------------------------------------------
    /// The track.
    ScrollbarTrack => "scrollbar.track",
    /// The thumb at rest.
    ScrollbarThumb => "scrollbar.thumb",
    /// The thumb, hovered.
    ScrollbarThumbHover => "scrollbar.thumb.hover",
    /// The thumb, grabbed.
    ScrollbarThumbActive => "scrollbar.thumb.active",

    // ---- The categorical data-type ramp (Strata's display taxonomy — see `Kind`) -----------
    /// Strings.
    DataTypeString => "data_type.string",
    /// Numbers.
    DataTypeNumber => "data_type.number",
    /// Booleans.
    DataTypeBoolean => "data_type.boolean",
    /// Timestamps/dates.
    DataTypeTimestamp => "data_type.timestamp",
    /// Structs.
    DataTypeStruct => "data_type.struct",
    /// Lists.
    DataTypeList => "data_type.list",
    /// Maps.
    DataTypeMap => "data_type.map",

    // ---- The ordered chart series ramp ------------------------------------------------------
    /// Series 1.
    Chart1 => "chart.1",
    /// Series 2.
    Chart2 => "chart.2",
    /// Series 3.
    Chart3 => "chart.3",
    /// Series 4.
    Chart4 => "chart.4",
    /// Series 5.
    Chart5 => "chart.5",
    /// Series 6.
    Chart6 => "chart.6",
    /// Series 7.
    Chart7 => "chart.7",
    /// Series 8.
    Chart8 => "chart.8",
    /// Series 9.
    Chart9 => "chart.9",
    /// Series 10.
    Chart10 => "chart.10",

    // ---- Entity kinds: catalog icons + completion kinds, one reconciled set -----------------
    /// A table.
    EntityTable => "entity.table",
    /// A view.
    EntityView => "entity.view",
    /// A saved query.
    EntityQuery => "entity.query",
    /// A column.
    EntityColumn => "entity.column",
    /// A function.
    EntityFunction => "entity.function",
    /// A keyword (completion), aligned with the syntax keyword hue.
    EntityKeyword => "entity.keyword",

    // ---- Source-format badges: a closed set, deliberately NOT the data-type ramp —
    //      retinting strings must not repaint file badges ------------------------------------
    /// Parquet.
    FormatParquet => "format.parquet",
    /// CSV.
    FormatCsv => "format.csv",
    /// JSON.
    FormatJson => "format.json",
    /// Arrow.
    FormatArrow => "format.arrow",
    /// A view badge.
    FormatView => "format.view",
}

/// The 27 `ColorsSheet` slot names — reference targets + the required sheet keys.
pub const SLOTS: &[&str] = &[
    "primary",
    "secondary",
    "tertiary",
    "success",
    "warning",
    "error",
    "info",
    "background",
    "surface_primary",
    "surface_secondary",
    "surface_tertiary",
    "surface_inverse",
    "surface_inverse_secondary",
    "surface_inverse_tertiary",
    "border",
    "border_focus",
    "border_disabled",
    "text_primary",
    "text_secondary",
    "text_placeholder",
    "text_inverse",
    "text_highlight",
    "focus",
    "active",
    "disabled",
    "overlay",
    "shadow",
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
    /// App-named colour slots, extending the 27 [`SLOTS`] the frontend palette carries. A
    /// `reference` in `components` resolves against the sheet first, then this map — so a tone
    /// the sheet has no name for (a muted meta text, a hairline, an accent) is stated **once**
    /// here and referenced everywhere, instead of being repeated as a `specific` per field.
    /// Names are free-form; the frontend paints an unresolvable one magenta.
    #[serde(default)]
    pub palette: BTreeMap<String, String>,
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
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(s) => Ok(Self::Color(s)),
            serde_json::Value::Number(n) => Ok(Self::Scalar(
                n.as_f64()
                    .ok_or_else(|| D::Error::custom("specific number out of range"))?
                    as f32,
            )),
            serde_json::Value::Array(a) => {
                let sides: Vec<f32> = a
                    .iter()
                    .map(|v| v.as_f64().map(|n| n as f32))
                    .collect::<Option<_>>()
                    .ok_or_else(|| D::Error::custom("specific sides must be numbers"))?;
                let sides: [f32; 4] = sides
                    .try_into()
                    .map_err(|_| D::Error::custom("specific sides must have exactly 4 numbers"))?;
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

impl StrataTheme {
    /// Every `reference` in `components` naming neither a core [`SLOTS`] entry nor a
    /// [`palette`](Self::palette) key, formatted `"<component>.<field> -> <name>"`.
    ///
    /// `reference` is an open namespace (a theme names its own palette slots), so the JSON
    /// schema can no longer enumerate the valid targets the way a closed enum did. This is what
    /// replaces that check: the theme still renders — an unresolved reference paints magenta —
    /// but a typo becomes a warning at load instead of a colour nobody looks at twice.
    pub fn unresolved_references(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (component, fields) in &self.components {
            for (field, pref) in fields {
                if let Pref::Reference(name) = pref {
                    if !SLOTS.contains(&name.as_str()) && !self.palette.contains_key(name) {
                        out.push(format!("{component}.{field} -> {name}"));
                    }
                }
            }
        }
        out
    }
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
    from_str(json).expect("strata theme json")
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
            let _ = fs::create_dir_all(dir);
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
                theme: from_str(raw).expect("built-in theme json"),
                source: Source::Builtin,
            })
            .collect();
        for dir in dirs {
            let Ok(rd) = fs::read_dir(dir) else {
                continue;
            };
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
                    Ok(theme) => {
                        for r in theme.unresolved_references() {
                            tracing::warn!("theme {}: unresolved reference {r}", path.display());
                        }
                        upsert(&mut entries, theme, Source::User)
                    }
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
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    from_str(&raw).map_err(|e| e.to_string())
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
    let home = env::var_os("HOME")?;
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
        let _ = fs::create_dir_all(&dir);
        #[cfg(target_os = "macos")]
        let _ = Command::new("open").arg(&dir).spawn();
        #[cfg(target_os = "windows")]
        let _ = Command::new("explorer").arg(&dir).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let _ = Command::new("xdg-open").arg(&dir).spawn();
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
        Command::new("defaults")
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

/// Build the JSON schema for the theme format: the fixed model (sheet slots, palette, fonts,
/// typography roles) plus the frontend's themeable components — `component_registries` is a
/// set of `(component key, fields + kinds)` tables (e.g. Freya's builtin-override registry
/// and its custom-component registry). The frontend's `schema_in_sync` test keeps
/// `themes/theme.schema.json` equal to this.
pub fn generate_schema(component_registries: &[&[(&str, &[(&str, Kind)])]]) -> serde_json::Value {
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
        "display",
        "title",
        "strong_body",
        "body_medium",
        "control",
        "body",
        "caption",
        "code_display",
        "data_display",
        "data_value",
        "code_block",
        "field_label",
        "meta",
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
            "palette": { "type": "object", "additionalProperties": { "$ref": "#/$defs/color" } },
            "scale": { "type": "object", "additionalProperties": { "type": "object", "additionalProperties": { "type": "number" } } },
            "fonts": { "type": "object", "properties": { "ui": { "type": "string" }, "mono": { "type": "string" } }, "additionalProperties": { "type": "string" } },
            "typography": { "type": "object", "additionalProperties": false, "properties": Value::Object(typo_props) }
        },
        "$defs": {
            "color": { "type": "string", "pattern": "^(#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?|rgba\\([^)]*\\))$" },
            "slot": { "type": "string", "description": "A core sheet slot, or a key of this theme's `palette`", "examples": slots.clone() },
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
    use std::process;

    /// A fresh, empty scratch dir under the OS temp dir (no tempfile dep for two tests).
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata-theme-registry-{}-{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The vocabulary's internal coherence: every dotted name is unique and round-trips through
    /// `from_name`, and every fallback chain terminates at a required role in bounded steps —
    /// a cycle would hang resolution, and nothing else checks for one.
    #[test]
    fn role_table_is_coherent() {
        let mut seen = std::collections::BTreeSet::new();
        for role in Role::ALL {
            assert!(
                seen.insert(role.name()),
                "duplicate role name {}",
                role.name()
            );
            assert_eq!(Role::from_name(role.name()), Some(*role), "{}", role.name());
            assert!(
                role.name()
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._".contains(c)),
                "role name '{}' is not lowercase dotted",
                role.name()
            );
            let mut current = *role;
            let mut steps = 0;
            loop {
                match current.fallback() {
                    Fallback::Required | Fallback::Transparent => break,
                    Fallback::Role(next) => {
                        current = next;
                        steps += 1;
                        assert!(steps <= Role::COUNT, "fallback cycle at {}", role.name());
                    }
                }
            }
        }
        assert_eq!(
            Role::COUNT,
            100,
            "the vocabulary is a deliberate, counted set"
        );
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
        fs::write(dir.join("custom.json"), custom).unwrap();
        // An override: a user file reusing the built-in id replaces it in place.
        let renamed = MIDNIGHT_JSON.replace(r#""name": "Midnight""#, r#""name": "My Midnight""#);
        assert_ne!(renamed, MIDNIGHT_JSON, "name marker must match the fixture");
        fs::write(dir.join("midnight-tweak.json"), renamed).unwrap();
        // Broken files are skipped, never fatal.
        fs::write(dir.join("broken.json"), "{ not json").unwrap();

        let reg = ThemeRegistry::with_dirs(std::slice::from_ref(&dir));
        let ids: Vec<&str> = reg.entries().iter().map(|e| e.theme.id.as_str()).collect();
        assert_eq!(ids, ["midnight", "daylight", "custom"]);
        assert_eq!(reg.get("midnight").unwrap().name, "My Midnight");
        assert_eq!(
            reg.entries()[0].source,
            Source::User,
            "override rebadges the entry"
        );
        assert_eq!(reg.entries()[2].source, Source::User);

        let _ = fs::remove_dir_all(&dir);
    }
}
