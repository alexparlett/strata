//! The Strata theme **data model** — the native JSON theme format (`themes/*.json`),
//! framework-agnostic.
//!
//! Midnight/Daylight are built-ins (embedded); custom themes load the same shape from the user
//! themes dir. A theme file authors **roles**: a closed, schema-enumerated vocabulary of dotted
//! colour names ([`Role`]) covering surfaces, element states, borders, text, accent, the status
//! triads and the data ramps — plus a `syntax` map for the editor's scopes, `fonts`, and the
//! `typography` type scale. Components are **not** in the file: the frontend maps every
//! component field onto a role in one static table, so a theme retunes the app by retuning
//! roles alone.
//!
//! This module owns the authored shape, the [`Role`] table (names + fallback rules), the
//! [`ThemeRegistry`] (discovery over the embedded built-ins + the user themes dir, with
//! [`Source`] badges and id lookup), the resolved [`Typography`] scale, the Sync-with-OS
//! selection helpers, and the JSON-schema generator ([`generate_schema`], parameterized over
//! the editor's syntax-scope list). Everything Freya-specific — resolving roles into `Color`s,
//! the component mapping table, schema sync — lives in `strata-freya`'s `theme` module.

use serde::Deserialize;
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
    /// The standard panel surface: tab bar, status bar, grid body, input wells, cards.
    SurfaceBackground => "surface.background",
    /// One step up: the sidebar, drawer, inspector, chart canvas, settings/launcher body,
    /// title bar.
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
    /// Filled-control pressed fill. No control paints a distinct pressed fill yet, so this is
    /// optional until one does — authoring it is how a theme splits press from hover.
    ElementActive => "element.active" (or ElementHover),
    /// Neutral selected fill: a menu's checked item, the active grid gutter/header.
    ElementSelected => "element.selected",
    /// Disabled filled-control fill.
    ElementDisabled => "element.disabled",
    /// Flush-control rest fill (transparent in both built-ins, themeable).
    GhostElementBackground => "ghost_element.background",
    /// Flush hover wash: tabs, sidebar rows, drawer rows, segments.
    GhostElementHover => "ghost_element.hover",
    /// Flush pressed fill. As with `element.active`: optional until a flush control paints a
    /// distinct pressed state.
    GhostElementActive => "ghost_element.active" (or GhostElementSelected),
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

/// The authored form of "fully transparent" — what a [`Fallback::Transparent`] role reads as.
pub const TRANSPARENT: &str = "rgba(0,0,0,0)";

/// Light/dark grouping — picks the frontend's base theme and (later) the Sync-with-OS split.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

/// A theme file exactly as authored.
///
/// `roles` and `syntax` are deliberately **required** (no serde default): a pre-roles file —
/// one with a `sheet`/`components` section — fails to parse and takes discovery's warn-and-skip
/// path, with a legacy-specific message, rather than loading as an all-magenta theme.
#[derive(Deserialize)]
pub struct StrataTheme {
    pub id: String,
    pub name: String,
    pub mode: Mode,
    /// The authored vocabulary: dotted [`Role`] name → colour string. The closed set of names
    /// is [`Role::ALL`]; an unknown name is warned at discovery and ignored, a missing required
    /// one is warned and paints magenta. Values are always literal colours — no aliasing inside
    /// the file, because per-theme aliasing is exactly how the old palette rotted.
    pub roles: BTreeMap<String, String>,
    /// The editor's syntax colours: author-facing scope name (`keyword`,
    /// `punctuation.bracket`, …) → colour string. The scope list is the editor crate's
    /// `SYNTAX_SCOPES`; validation is parameterized over it because this crate is Freya-free.
    pub syntax: BTreeMap<String, String>,
    #[serde(default)]
    pub fonts: BTreeMap<String, String>,
    /// The type scale — named roles (title · body · meta · …), each fixing a font
    /// family (`ui`/`mono`, resolved via `fonts`), weight and size (+ optional line-height /
    /// letter-spacing). A **top-level** section (not a `components` entry): its fields are
    /// `TypeRole` objects, not colour strings like `roles` and `syntax` hold. Resolved
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

impl StrataTheme {
    /// The effective authored value for a role: its own `roles` entry, or the first authored
    /// value along its fallback chain. `None` means a **required** role is missing — the
    /// frontend paints magenta and [`missing_roles`](Self::missing_roles) names it at discovery.
    pub fn role_value(&self, role: Role) -> Option<&str> {
        let mut current = role;
        loop {
            if let Some(v) = self.roles.get(current.name()) {
                return Some(v);
            }
            match current.fallback() {
                Fallback::Role(next) => current = next,
                Fallback::Transparent => return Some(TRANSPARENT),
                Fallback::Required => return None,
            }
        }
    }

    /// Authored role names the vocabulary doesn't know — typos, or roles from a newer build.
    /// Warned at discovery; the values are ignored.
    pub fn unknown_roles(&self) -> Vec<&str> {
        self.roles
            .keys()
            .filter(|k| Role::from_name(k).is_none())
            .map(String::as_str)
            .collect()
    }

    /// Roles with no effective value — required, unauthored, and with no authored fallback.
    /// Warned at discovery; each paints magenta until the file names it.
    pub fn missing_roles(&self) -> Vec<&'static str> {
        Role::ALL
            .iter()
            .filter(|r| self.role_value(**r).is_none())
            .map(|r| r.name())
            .collect()
    }

    /// Authored syntax scopes the editor doesn't know, against its `scopes` list.
    pub fn unknown_syntax<'a>(&'a self, scopes: &[&str]) -> Vec<&'a str> {
        self.syntax
            .keys()
            .filter(|k| !scopes.contains(&k.as_str()))
            .map(String::as_str)
            .collect()
    }

    /// Editor scopes the file doesn't author. Warned at discovery; each paints magenta.
    pub fn missing_syntax<'a>(&self, scopes: &[&'a str]) -> Vec<&'a str> {
        scopes
            .iter()
            .filter(|s| !self.syntax.contains_key(**s))
            .copied()
            .collect()
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
    /// there's always a place to drop themes). `syntax_scopes` is the editor's scope list,
    /// threaded in because this crate is Freya-free — it gates the `syntax` warnings only.
    pub fn discover(syntax_scopes: &[&str]) -> Self {
        let dirs: Vec<PathBuf> = user_themes_dir().into_iter().collect();
        for dir in &dirs {
            let _ = fs::create_dir_all(dir);
        }
        Self::with_dirs(&dirs, syntax_scopes)
    }

    /// Build from the built-ins plus the given theme dirs (the testable core of
    /// [`discover`](Self::discover)). Unreadable/invalid files are skipped with a warning —
    /// a broken user theme must never take the app down.
    pub fn with_dirs(dirs: &[PathBuf], syntax_scopes: &[&str]) -> Self {
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
                        // Warn, never reject: every one of these still renders (magenta where a
                        // value is missing), and a loud wrong colour beats a theme that
                        // silently vanished from the list.
                        for r in theme.unknown_roles() {
                            tracing::warn!("theme {}: unknown role '{r}'", path.display());
                        }
                        for r in theme.missing_roles() {
                            tracing::warn!("theme {}: missing role '{r}'", path.display());
                        }
                        for s in theme.unknown_syntax(syntax_scopes) {
                            tracing::warn!("theme {}: unknown syntax scope '{s}'", path.display());
                        }
                        for s in theme.missing_syntax(syntax_scopes) {
                            tracing::warn!("theme {}: missing syntax scope '{s}'", path.display());
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
    from_str(&raw).map_err(|e| {
        // A pre-roles file fails on the missing `roles` field; name the real problem instead of
        // handing the author a bare serde error.
        if raw.contains("\"sheet\"") {
            format!("pre-roles theme format (has a 'sheet' section); see docs/FREYA_THEME_SPEC.md")
        } else {
            e.to_string()
        }
    })
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
    pub title: TextStyle,
    pub strong_body: TextStyle,
    pub body_medium: TextStyle,
    pub control: TextStyle,
    pub body: TextStyle,
    pub caption: TextStyle,
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
        title: role("title"),
        strong_body: role("strong_body"),
        body_medium: role("body_medium"),
        control: role("control"),
        body: role("body"),
        caption: role("caption"),
        data_value: role("data_value"),
        code_block: role("code_block"),
        field_label: role("field_label"),
        meta: role("meta"),
        mono_path: role("mono_path"),
    }
}

/// Build the JSON schema for the theme format: the [`Role`] vocabulary (each role an explicit
/// property, required unless it has a fallback), the editor's `syntax` scopes (threaded in as
/// `syntax_scopes` because this crate is Freya-free), `fonts` and the `typography` roles. The
/// frontend's `schema_in_sync` test keeps `themes/theme.schema.json` equal to this.
pub fn generate_schema(syntax_scopes: &[&str]) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let mut role_props = Map::new();
    let mut role_required = Vec::new();
    for role in Role::ALL {
        let doc = match role.fallback() {
            Fallback::Required => {
                role_required.push(role.name());
                json!({ "$ref": "#/$defs/color" })
            }
            Fallback::Role(other) => json!({
                "$ref": "#/$defs/color",
                "description": format!("Optional; omitted reads '{}'.", other.name()),
            }),
            Fallback::Transparent => json!({
                "$ref": "#/$defs/color",
                "description": "Optional; omitted is transparent.",
            }),
        };
        role_props.insert(role.name().to_string(), doc);
    }

    let mut syntax_props = Map::new();
    for s in syntax_scopes {
        syntax_props.insert((*s).to_string(), json!({ "$ref": "#/$defs/color" }));
    }

    // The type scale — one `typeRole` per named role; mirrors `Typography`'s fields.
    const TYPE_ROLES: &[&str] = &[
        "title",
        "strong_body",
        "body_medium",
        "control",
        "body",
        "caption",
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
        "required": ["id", "name", "mode", "roles", "syntax"],
        "additionalProperties": false,
        "properties": {
            "$schema": { "type": "string" },
            "id": { "type": "string" },
            "name": { "type": "string" },
            "mode": { "enum": ["dark", "light"] },
            "roles": { "type": "object", "additionalProperties": false, "required": role_required, "properties": Value::Object(role_props) },
            "syntax": { "type": "object", "additionalProperties": false, "required": syntax_scopes, "properties": Value::Object(syntax_props) },
            "fonts": { "type": "object", "properties": { "ui": { "type": "string" }, "mono": { "type": "string" } }, "additionalProperties": { "type": "string" } },
            "typography": { "type": "object", "additionalProperties": false, "properties": Value::Object(typo_props) }
        },
        "$defs": {
            "color": { "type": "string", "pattern": "^(#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?|rgba\\([^)]*\\))$" },
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
        let reg = ThemeRegistry::with_dirs(&[], &[]);
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
        // A pre-roles file (the old sheet/components format) is skipped the same way — it
        // fails on the missing `roles` field, and `parse_theme_file` names the real problem.
        fs::write(
            dir.join("legacy.json"),
            r#"{ "id": "legacy", "name": "Legacy", "mode": "dark", "sheet": {}, "components": {} }"#,
        )
        .unwrap();

        let reg = ThemeRegistry::with_dirs(std::slice::from_ref(&dir), &[]);
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
