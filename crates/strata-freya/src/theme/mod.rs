//! The Freya theme — Strata's **native** theme format applied to Freya.
//!
//! The theme **data model** (the [`Role`] vocabulary, authored shapes, built-in loader,
//! [`Typography`] resolution, schema generator) lives in [`strata_core::theme`] and is
//! re-exported here; this module is the Freya-specific half. A theme file authors `roles` +
//! `syntax` + `fonts` + `typography` and nothing else: every component's dress is fixed onto
//! roles by the static mapping table in [`components`], registered once per theme build, with
//! colour fields as `Preference::Reference`s the fork resolves against [`StrataPalette`] at
//! read time.
//!
//! Resolution order is the fork's: `core_slot` answers the 27 `ColorsSheet` names first (fed
//! by [`bridge_sheet`], so un-overridden built-in defaults keep painting correctly), then
//! [`Palette::color`] answers the role names. Seven role names coincide with core slot names
//! (`background`, `border`, `shadow` and the four status tones) and take the first path — which
//! is harmless exactly as long as the bridge maps each of those slots to the same-named role;
//! `a_role_reference_resolves_to_its_own_colour` pins that either path answers the role's own
//! colour. An unknown name paints **magenta**, never Freya's `primary` fallback, so a typo is
//! visible on screen.

use std::ops::Deref;
use std::sync::Arc;

use freya::prelude::*;
use strata_code_editor::editor_theme::EditorSyntaxThemePreference;
use strata_code_editor::prelude::SYNTAX_SCOPES;
#[cfg(test)]
use strata_core::theme::generate_schema as core_schema;
use strata_core::theme::ThemeRegistry;

use crate::state::{use_config_channel, ConfigChan, ConfigStation, ThemePreview, ThemeSel};

mod components;

pub use strata_core::theme::{
    resolve_typography, typography, Mode, Role, StrataTheme, TextStyle, Typography,
};

/// The loud unknown: what an unresolvable name, missing role, or unparseable colour paints.
const MAGENTA: Color = Color::from_rgb(255, 0, 255);

/// The app-wide theme registry handle for context — an `Arc` over the discovered
/// [`ThemeRegistry`], cheap to clone. Created **once** in `main` and provided at every
/// window root, so all apps (project, launcher, settings, …) share the same discovery.
/// Derefs to the registry, so callers use it directly (`themes.get_or_default(…)`,
/// `themes.entries()`).
#[derive(Clone)]
pub struct ThemesCtx(Arc<ThemeRegistry>);

impl ThemesCtx {
    /// Discover the registry (built-ins + the user themes dir) and wrap it for context.
    /// The editor's scope list rides along so discovery can warn about a user theme's
    /// `syntax` section too.
    pub fn discover() -> Self {
        Self(Arc::new(ThemeRegistry::discover(SYNTAX_SCOPES)))
    }
}

/// There is exactly one registry per process (discovered in `main`), so identity is the
/// only meaningful comparison — and it's what lets a component hold the handle as a prop
/// without its diff ever seeing a change.
impl PartialEq for ThemesCtx {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Deref for ThemesCtx {
    type Target = ThemeRegistry;

    fn deref(&self) -> &ThemeRegistry {
        &self.0
    }
}

/// Every role's resolved colour, indexed by [`Role`] ordinal. Built once per theme build by
/// [`resolve_roles`] and installed on the Freya `Theme` under [`ROLES_KEY`], the same
/// `Any`-map mechanism the [`Typography`] rides.
#[derive(Clone, Copy, PartialEq)]
pub struct RoleColors([Color; Role::COUNT]);

impl RoleColors {
    pub fn get(&self, role: Role) -> Color {
        self.0[role as usize]
    }
}

/// The `Theme` key the resolved [`RoleColors`] are installed under (see [`strata_theme`]).
pub const ROLES_KEY: &str = "strata_roles";

/// This window's resolved role colours — how a surface reads a colour its component theme has
/// no field for. A standard theme lookup (one theme read, same subscription as `use_theme`);
/// call it from a component's `render`. The `unwrap_or` is defensive — [`strata_theme`] always
/// seeds the key; an unseeded theme paints all-magenta rather than panicking pre-launch.
pub fn use_roles() -> RoleColors {
    let theme = use_theme();
    let theme = theme.read();
    theme
        .get::<RoleColors>(ROLES_KEY)
        .copied()
        .unwrap_or(RoleColors([MAGENTA; Role::COUNT]))
}

/// The theme's colour source. The fork resolves a `Preference::Reference` against the core
/// sheet first (fed by [`bridge_sheet`]) and only then against [`Palette::color`], which
/// answers the dotted role names from [`RoleColors`].
///
/// `color` never answers `None`: Freya's own fallback for an unresolved reference is
/// `primary`, which would *hide* a typo — magenta keeps it visible on screen (and discovery
/// warned about it at load).
struct StrataPalette {
    sheet: ColorsSheet,
    roles: RoleColors,
}

impl Palette for StrataPalette {
    fn sheet(&self) -> &ColorsSheet {
        &self.sheet
    }

    fn color(&self, name: &str) -> Option<Color> {
        Some(match Role::from_name(name) {
            Some(role) => self.roles.get(role),
            None => MAGENTA,
        })
    }
}

/// Resolve every role of a theme file into colours — authored value, else the fallback chain,
/// else magenta (a required role the file omitted; discovery warned).
fn resolve_roles(t: &StrataTheme) -> RoleColors {
    let mut colors = [MAGENTA; Role::COUNT];
    for role in Role::ALL {
        if let Some(v) = t.role_value(*role) {
            colors[*role as usize] = pc(v);
        }
    }
    RoleColors(colors)
}

/// Feed the fork's 27-slot `ColorsSheet` from the resolved roles, so built-in component
/// defaults the mapping table does not override still paint correctly. Mapped by each old
/// slot's **behaviour in fork defaults**, not by name similarity — e.g. `surface_tertiary`
/// is the fork's control fill, so it reads `element.background`; `disabled` is overwhelmingly
/// a text tone, so it reads `text.disabled`.
///
/// The fork's own vocabulary deliberately stays untouched (upstream-shaped):
/// the pluggable `Palette` commit was the fork-level fix, and this bridge is the app's use of
/// that seam.
fn bridge_sheet(roles: &RoleColors) -> ColorsSheet {
    ColorsSheet {
        primary: roles.get(Role::Accent),
        secondary: roles.get(Role::AccentRing),
        tertiary: roles.get(Role::AccentHover),
        success: roles.get(Role::Success),
        warning: roles.get(Role::Warning),
        error: roles.get(Role::Error),
        info: roles.get(Role::Info),
        background: roles.get(Role::Background),
        surface_primary: roles.get(Role::SurfaceBackground),
        surface_secondary: roles.get(Role::SurfaceRaised),
        surface_tertiary: roles.get(Role::ElementBackground),
        surface_inverse: roles.get(Role::DropTarget),
        surface_inverse_secondary: roles.get(Role::ScrollbarThumbHover),
        surface_inverse_tertiary: roles.get(Role::ScrollbarThumbActive),
        border: roles.get(Role::Border),
        border_focus: roles.get(Role::BorderFocused),
        border_disabled: roles.get(Role::BorderDisabled),
        text_primary: roles.get(Role::Text),
        text_secondary: roles.get(Role::TextMuted),
        text_placeholder: roles.get(Role::TextPlaceholder),
        text_inverse: roles.get(Role::TextOnAccent),
        text_highlight: roles.get(Role::TextAccent),
        focus: roles.get(Role::AccentSelection),
        active: roles.get(Role::ElementSelected),
        disabled: roles.get(Role::TextDisabled),
        overlay: roles.get(Role::Backdrop),
        shadow: roles.get(Role::Shadow),
    }
}

/// A role's colour read from the **authored** file (fallbacks applied) — for the places that
/// paint a theme which is *not* installed: the pre-launch window background, and the Settings
/// grid's thumbnails of every discovered theme. Missing → magenta, total by construction.
pub(crate) fn authored_role(t: &StrataTheme, role: Role) -> Color {
    t.role_value(role).map(pc).unwrap_or(MAGENTA)
}

/// The window-chrome background for a theme — its `background` role. Fed to
/// `WindowConfig::with_background` so a resize never flashes the default white.
pub fn window_background(t: &StrataTheme) -> Color {
    authored_role(t, Role::Background)
}

/// The theme selection in force **right now**, without subscribing to either input — for the
/// pre-launch window background, where there is no reactive scope and no `Platform` yet.
///
/// The preview outranks the settings for the same reason it does in [`use_strata_theme`]: a
/// window opened while the Settings window is previewing a theme has to come up wearing that
/// theme, not the committed one it is about to replace.
pub fn peek_selection(config: ConfigStation, preview: ThemePreview) -> ThemeSel {
    preview
        .peek()
        .clone()
        .unwrap_or_else(|| ThemeSel::from(&config.peek().settings))
}

/// Install this window's Freya theme and keep it **derived** from the app-global config's
/// [`Settings`](strata_core::config::Settings) selection (`theme` + `sync_os`), the Settings
/// window's live [`ThemePreview`] where it has one, and — only while syncing — the OS
/// appearance (this window's `Platform.preferred_theme`, seeded from the window's real
/// theme and live via winit `ThemeChanged`). Every window root mounts this; the Settings UI
/// writes the preview (live, uncommitted) or the config store (on Save) and every window
/// repaints.
///
/// Subscribes to [`ConfigChan::Settings`] only, so the recents/open-set churn of opening a
/// project never re-derives a theme.
///
/// There is no stored applied-theme id to keep coherent: windows stay consistent because
/// each computes the same pure derivation (`effective_id`) of the same global inputs.
/// The `Theme.name` guard (it carries the applied id) skips no-op rebuilds — including
/// the mount-time echo of the id `use_init_theme` already resolved, and the Save that
/// commits a preview the window is already wearing.
pub fn use_strata_theme(themes: ThemesCtx, config: ConfigStation, preview: ThemePreview) {
    let platform = use_hook(Platform::get);
    let preferred = platform.preferred_theme;
    let settings = use_config_channel(config, ConfigChan::Settings);
    let mut theme = use_init_theme({
        let themes = themes.clone();
        move || {
            let sel = peek_selection(config, preview);
            let os_dark = sel.sync_os && *preferred.peek() == PreferredTheme::Dark;
            strata_theme(themes.get_or_default(&sel.effective(os_dark)))
        }
    });
    use_side_effect(move || {
        let committed = ThemeSel::from(&settings.read().settings);
        let sel = preview.read().clone().unwrap_or(committed);
        let os_dark = sel.sync_os && *preferred.read() == PreferredTheme::Dark;
        let id = sel.effective(os_dark);
        let applied = theme.peek().name;
        if applied != id {
            theme.set(strata_theme(themes.get_or_default(&id)));
        }
    });
}

/// A Freya `Theme` for the given Strata theme (resolved through the [`ThemesCtx`] registry):
/// the resolved roles over Freya's light/dark base, with every component's dress registered
/// from the static mapping table and the editor's syntax colours from the file's `syntax`
/// section. Only the palette, the syntax and the fonts vary per theme — the table does not.
pub fn strata_theme(t: &StrataTheme) -> Theme {
    let mut th = match t.mode {
        Mode::Light => light_theme(),
        Mode::Dark => dark_theme(),
    };
    th.name = Box::leak(t.id.clone().into_boxed_str());
    let roles = resolve_roles(t);
    let typo = resolve_typography(t);
    th.palette = Box::new(StrataPalette {
        sheet: bridge_sheet(&roles),
        roles,
    });
    components::register_component_themes(&mut th, &typo);
    th.set(
        "code_editor_syntax",
        EditorSyntaxThemePreference::from_scopes(|scope| t.syntax.get(scope).map(|s| pc(s))),
    );
    th.set(ROLES_KEY, roles);
    th.set(TYPOGRAPHY_KEY, typo);
    th
}

/// The `Theme` key the resolved [`Typography`] scale is installed under (see [`strata_theme`]).
/// Prefixed `strata_` so it never collides with Freya's built-in `typography` component theme.
pub const TYPOGRAPHY_KEY: &str = "strata_typography";

/// The theme JSON schema for this app: the core model schema
/// ([`strata_core::theme::generate_schema`], imported as `core_schema` — a genuine collision
/// with this wrapper's own name) over the editor's [`SYNTAX_SCOPES`]. The `schema_in_sync`
/// test keeps `themes/theme.schema.json` equal to this.
///
/// Test-only because that test is its only caller and its whole purpose: the committed schema is
/// regenerated with `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`, which is where
/// this codegen lives rather than in a script beside the crate.
#[cfg(test)]
pub fn generate_schema() -> serde_json::Value {
    core_schema(SYNTAX_SCOPES)
}

/// Parse an authored colour: `#rrggbb`, `#rrggbbaa`, or `rgba(r,g,b,a)`. Anything else →
/// magenta, so a bad value is obvious on screen. Uninstalled-theme reads go through
/// [`authored_role`], so nothing outside this module parses colour strings.
fn pc(s: &str) -> Color {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix("rgba(").and_then(|x| x.strip_suffix(')')) {
        let p: Vec<&str> = inner.split(',').map(str::trim).collect();
        if p.len() == 4 {
            let r = p[0].parse::<u8>().unwrap_or(0);
            let g = p[1].parse::<u8>().unwrap_or(0);
            let b = p[2].parse::<u8>().unwrap_or(0);
            let a = p[3].parse::<f32>().unwrap_or(1.0);
            return Color::from_rgb(r, g, b).with_a((a * 255.0).round() as u8);
        }
    }
    let hex = s.trim_start_matches('#');
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return MAGENTA;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
    match hex.len() {
        6 => Color::from_rgb(byte(0), byte(2), byte(4)),
        _ => Color::from_rgb(byte(0), byte(2), byte(4)).with_a(byte(6)),
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use strata_code_editor::editor_theme::SYNTAX_SCOPES;
    use strata_core::theme::{load, Role};

    use super::{generate_schema, Preference};

    /// The committed `theme.schema.json` (root `themes/`, beside the theme files strata-core
    /// embeds) must equal what `generate_schema()` produces — so the schema can't drift from
    /// the vocabulary. Regenerate with
    /// `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.
    #[test]
    fn schema_in_sync() {
        let generated = generate_schema();
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/theme.schema.json"
        );
        if env::var_os("UPDATE_SCHEMA").is_some() {
            let out = serde_json::to_string_pretty(&generated).unwrap() + "\n";
            fs::write(path, out).unwrap();
        } else {
            let committed: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(
                committed, generated,
                "theme.schema.json is stale — run `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`"
            );
        }
    }

    /// A role's reference must resolve to the role's **own** colour, whichever path answers.
    /// Seven role names coincide with fork core slot names (`background`, `border`, `shadow`,
    /// and the four status tones), and `core_slot` answers those first — harmless exactly as
    /// long as [`bridge_sheet`](super::bridge_sheet) maps each of those slots to the
    /// same-named role. If a future role's name ever collides with a slot the bridge points
    /// elsewhere, the reference would silently paint the *other* role's colour — this is the
    /// test that makes that loud.
    ///
    /// Probe: give every role a unique synthetic colour, bridge from the same set, and
    /// require resolution to return each role's own.
    #[test]
    fn a_role_reference_resolves_to_its_own_colour() {
        use freya::prelude::{Color, ResolvablePreference};

        use super::{bridge_sheet, RoleColors, StrataPalette};

        let mut colors = [Color::BLACK; Role::COUNT];
        for (i, slot) in colors.iter_mut().enumerate() {
            *slot = Color::from_rgb(i as u8, (i as u8).wrapping_add(101), 7);
        }
        let roles = RoleColors(colors);
        let palette = StrataPalette {
            sheet: bridge_sheet(&roles),
            roles,
        };
        for role in Role::ALL {
            let resolved: Color = Preference::reference(role.name()).resolve(&palette);
            assert_eq!(
                resolved,
                roles.get(*role),
                "role '{}' resolves to another role's colour (a core slot shadows it and the \
                 bridge points that slot elsewhere)",
                role.name()
            );
        }
    }

    /// Both built-in themes must author the whole vocabulary: no unknown names (a typo), no
    /// missing required roles (magenta), and the full syntax scope list — in both directions.
    /// A user theme gets the same checks as discovery warnings; for the built-ins they are a
    /// hard failure.
    #[test]
    fn builtin_themes_author_every_role_and_scope() {
        for id in ["midnight", "daylight"] {
            let t = load(id);
            assert!(
                t.unknown_roles().is_empty(),
                "{id}: {:?}",
                t.unknown_roles()
            );
            assert!(
                t.missing_roles().is_empty(),
                "{id}: {:?}",
                t.missing_roles()
            );
            let unknown = t.unknown_syntax(SYNTAX_SCOPES);
            assert!(unknown.is_empty(), "{id}: {unknown:?}");
            let missing = t.missing_syntax(SYNTAX_SCOPES);
            assert!(missing.is_empty(), "{id}: {missing:?}");
        }
    }

    /// Both committed theme files must parse — the app panics at launch otherwise — and the
    /// pure resolution layers must hold: a known role's authored hex, a syntax scope, the
    /// `fonts` lookup, and a **non-integer** scalar surviving serde (the
    /// `arbitrary_precision` regression now lives in `TypeRole`'s floats).
    #[test]
    fn theme_files_parse_end_to_end() {
        use strata_core::theme::resolve_typography;

        let accents = [("midnight", "#4cc6ff"), ("daylight", "#2b7fd0")];
        for (id, accent) in accents {
            let t = load(id);
            assert_eq!(t.roles["accent"], accent, "{id}: accent role");
            assert!(t.syntax.contains_key("punctuation.bracket"), "{id}: syntax");
            let typo = resolve_typography(&t);
            assert_eq!(
                typo.code_block.family, "JetBrains Mono",
                "{id}: fonts lookup"
            );
            assert_eq!(typo.code_block.line_height, Some(1.6), "{id}: float scalar");
        }
    }

    /// **The live-theme preview.** A window's theme is derived from the committed settings
    /// *unless* the Settings window has an uncommitted pick, and taking the slot back is what
    /// makes Cancel a revert. Every window runs this same derivation off the same globals, so
    /// pinning it in one window pins "previews live across windows".
    ///
    /// Worth a mounted test rather than a unit one on `ThemeSel`: the ordering is inside
    /// `use_strata_theme`'s effect, and the value under test is what Freya actually installed
    /// (`Theme.name` carries the applied id).
    #[test]
    fn a_preview_outranks_the_committed_theme_until_it_is_dropped() {
        use freya::elements::label::Label;
        use freya::prelude::*;
        use freya_testing::TestingRunner;
        use strata_core::config::AppConfig;

        use super::{use_strata_theme, ThemesCtx};
        use crate::state::{create_global_theme_preview, ConfigStation, ThemePreview, ThemeSel};

        type Handles = (ThemesCtx, ConfigStation, ThemePreview);

        fn app() -> impl IntoElement {
            let (themes, config, preview) = use_consume::<Handles>();
            use_strata_theme(themes, config, preview);
            label().text(use_theme().read().name)
        }

        /// The id the window is currently themed with. Two passes: the derivation is a side
        /// effect, so the first settles it and the second repaints the probe from it.
        fn applied(runner: &mut TestingRunner) -> String {
            runner.sync_and_update();
            runner.sync_and_update();
            runner
                .find(|_, element| Label::try_downcast(element))
                .expect("the probe label is on screen")
                .text
                .to_string()
        }

        let mut cfg = AppConfig::default();
        cfg.settings.theme = "midnight".to_string();
        let (mut runner, mut preview) = TestingRunner::new(
            app,
            (100., 100.).into(),
            move |r| {
                let handles: Handles = (
                    ThemesCtx::discover(),
                    ConfigStation::create_global(cfg),
                    create_global_theme_preview(),
                );
                let preview = handles.2;
                r.provide_root_context(move || handles);
                preview
            },
            1.,
        );
        assert_eq!(applied(&mut runner), "midnight");

        preview.set(Some(ThemeSel {
            theme: "daylight".to_string(),
            sync_os: false,
        }));
        assert_eq!(applied(&mut runner), "daylight");

        preview.set(None);
        assert_eq!(applied(&mut runner), "midnight");
    }
}
