//! The Freya theme — Strata's **native** theme format applied to Freya.
//!
//! The theme **data model** (authored shapes, built-in loader, [`Typography`] resolution,
//! schema generator) lives in [`strata_core::theme`] and is re-exported here; this module is
//! the Freya-specific half: a theme file's `sheet` and `palette` become the [`StrataPalette`]
//! Freya resolves references against, and each `components` entry — a tagged [`Pref`],
//! `{ "specific": … }` or `{ "reference": "<slot>" }` — is coerced into Freya `Preference`s and
//! applied as a *partial* merge over Freya's registered default.
//!
//! One `theme_registry!` invocation is the single source of truth: it generates both the
//! runtime override application and the [`REGISTRY`] data that [`generate_schema`] turns into
//! `themes/theme.schema.json`. The `schema_in_sync` test regenerates the schema and fails if
//! the committed file has drifted, so the two can never diverge (regenerate with
//! `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`).

use std::collections::BTreeMap;
use std::ops::Deref;
use std::sync::Arc;

use crate::apps::export::ExportThemePreference;
use crate::apps::launcher::LauncherThemePreference;
use crate::apps::project::{
    CancelButtonThemePreference, CatalogThemePreference, CellViewThemePreference,
    DataGridThemePreference, DrawerThemePreference, ExplainPlanThemePreference,
    HeaderBarThemePreference, InspectorThemePreference, RecordViewThemePreference,
    StatusBarThemePreference, TabBarThemePreference, TabThemePreference,
};
use crate::apps::settings::SettingsThemePreference;
use crate::components::avatar::AvatarThemePreference;
use crate::components::form::FormThemePreference;
use crate::components::run_button::RunButtonThemePreference;
use crate::components::segmented_toggle::SegmentedToggleThemePreference;
use crate::components::toggle_button::ToggleButtonThemePreference;
use crate::components::type_palette::TypePaletteThemePreference;
use crate::state::{use_config_channel, ConfigChan, ConfigStation, ThemePreview, ThemeSel};
use freya::prelude::*;
use strata_code_editor::editor_theme::EditorSyntaxThemePreference;
use strata_code_editor::prelude::EditorThemePreference;
use strata_core::theme::ThemeRegistry;

pub use strata_core::theme::{
    resolve_typography, typography, Kind, Mode, Pref, SheetDef, SpecificValue, StrataTheme,
    TextStyle, Typography,
};

/// The app-wide theme registry handle for context — an `Arc` over the discovered
/// [`ThemeRegistry`], cheap to clone. Created **once** in `main` and provided at every
/// window root, so all apps (project, launcher, settings, …) share the same discovery.
/// Derefs to the registry, so callers use it directly (`themes.get_or_default(…)`,
/// `themes.entries()`).
#[derive(Clone)]
pub struct ThemesCtx(Arc<ThemeRegistry>);

impl ThemesCtx {
    /// Discover the registry (built-ins + the user themes dir) and wrap it for context.
    pub fn discover() -> Self {
        Self(Arc::new(ThemeRegistry::discover()))
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

/// The theme's colour source: the 27 core slots every Freya component references, plus the
/// theme file's own `palette` — the app-named tones the sheet has no slot for (a muted meta
/// text, a hairline, an accent), so each is stated once and referenced everywhere instead of
/// repeated as a `specific` per field.
///
/// Freya consults [`Palette::color`] only for names the core sheet doesn't carry, so a palette
/// key can never shadow a slot a built-in component depends on. Unknown names resolve to
/// **magenta** rather than Freya's `primary` fallback — same convention as [`pc`], so a typo in
/// an open namespace is visible on screen (and warned about at load, via
/// `StrataTheme::unresolved_references`).
struct StrataPalette {
    sheet: ColorsSheet,
    named: BTreeMap<String, Color>,
}

impl Palette for StrataPalette {
    fn sheet(&self) -> &ColorsSheet {
        &self.sheet
    }

    fn color(&self, name: &str) -> Option<Color> {
        Some(
            self.named
                .get(name)
                .copied()
                .unwrap_or(Color::from_rgb(255, 0, 255)),
        )
    }
}

/// Copy a theme file's `sheet` into Freya's palette (a free fn — `SheetDef` lives in
/// `strata-core`, so an inherent impl isn't possible here).
fn to_colors_sheet(sheet: &SheetDef) -> ColorsSheet {
    ColorsSheet {
        primary: pc(&sheet.primary),
        secondary: pc(&sheet.secondary),
        tertiary: pc(&sheet.tertiary),
        success: pc(&sheet.success),
        warning: pc(&sheet.warning),
        error: pc(&sheet.error),
        info: pc(&sheet.info),
        background: pc(&sheet.background),
        surface_primary: pc(&sheet.surface_primary),
        surface_secondary: pc(&sheet.surface_secondary),
        surface_tertiary: pc(&sheet.surface_tertiary),
        surface_inverse: pc(&sheet.surface_inverse),
        surface_inverse_secondary: pc(&sheet.surface_inverse_secondary),
        surface_inverse_tertiary: pc(&sheet.surface_inverse_tertiary),
        border: pc(&sheet.border),
        border_focus: pc(&sheet.border_focus),
        border_disabled: pc(&sheet.border_disabled),
        text_primary: pc(&sheet.text_primary),
        text_secondary: pc(&sheet.text_secondary),
        text_placeholder: pc(&sheet.text_placeholder),
        text_inverse: pc(&sheet.text_inverse),
        text_highlight: pc(&sheet.text_highlight),
        focus: pc(&sheet.focus),
        active: pc(&sheet.active),
        disabled: pc(&sheet.disabled),
        overlay: pc(&sheet.overlay),
        shadow: pc(&sheet.shadow),
    }
}

/// The window-chrome background for a theme — its sheet `background` colour. Fed to
/// `WindowConfig::with_background` so a resize never flashes the default white.
pub fn window_background(t: &StrataTheme) -> Color {
    pc(&t.sheet.background)
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
        // `peek`s — the side effect below owns reactivity.
        move || {
            let sel = peek_selection(config, preview);
            let os_dark = sel.sync_os && *preferred.peek() == PreferredTheme::Dark;
            strata_theme(themes.get_or_default(&sel.effective(os_dark)))
        }
    });
    use_side_effect(move || {
        // Both inputs are read on every pass, not short-circuited on the preview: a Save
        // writes the settings and clears the preview, and a subscription taken only while
        // the preview was absent would miss whichever of the two landed second.
        let committed = ThemeSel::from(&settings.read().settings);
        let sel = preview.read().clone().unwrap_or(committed);
        // Short-circuit: only subscribe to the OS appearance while actually syncing.
        let os_dark = sel.sync_os && *preferred.read() == PreferredTheme::Dark;
        let id = sel.effective(os_dark);
        let applied = theme.peek().name;
        if applied != id {
            theme.set(strata_theme(themes.get_or_default(&id)));
        }
    });
}

/// A Freya `Theme` for the given Strata theme (resolved through the [`ThemesCtx`]
/// registry): our `sheet` + `components` over Freya's light/dark base (which supplies
/// every built-in's default + the layout/typography defaults).
pub fn strata_theme(t: &StrataTheme) -> Theme {
    let mut th = match t.mode {
        Mode::Light => light_theme(),
        Mode::Dark => dark_theme(),
    };
    // Freya's `Theme.name` is `&'static str`; the id is a runtime string (built-in or custom),
    // so leak it once (negligible, lives for the program).
    th.name = Box::leak(t.id.clone().into_boxed_str());
    th.palette = Box::new(StrataPalette {
        sheet: to_colors_sheet(&t.sheet),
        named: t.palette.iter().map(|(k, v)| (k.clone(), pc(v))).collect(),
    });
    apply_component_overrides(&mut th, &t.components, &t.fonts);
    register_component_themes(&mut th, &t.components, &t.fonts);
    // Install the resolved type scale onto the theme itself (its `Box<dyn Any>` component store), so
    // typography components read it with a standard `use_theme().get::<Typography>(..)` — no provider,
    // no cache. Keyed `strata_typography` to avoid Freya's built-in `typography`.
    th.set(TYPOGRAPHY_KEY, resolve_typography(t));
    th
}

/// The `Theme` key the resolved [`Typography`] scale is installed under (see [`strata_theme`]).
/// Prefixed `strata_` so it never collides with Freya's built-in `typography` component theme.
pub const TYPOGRAPHY_KEY: &str = "strata_typography";

/// Our own `define_theme!` components — the ones Freya's base themes don't register. **Unlike the
/// built-ins** (which inherit a Freya default and take *partial* `components` overrides), these are
/// defined **wholly in the theme file**: `"key" => Type { field, … }` names a component + its
/// colour fields, and registration reads every field straight from `components.<key>`. There are no
/// code defaults — a field the theme file omits renders magenta, on purpose (the theme owns it).
///
/// The constructor lives behind a **local** trait (not an inherent `impl`) so the same macro works
/// for **external** preference types too — an inherent impl is illegal outside a type's own crate,
/// but a local-trait impl isn't. That's how `code_editor` (freya's own `EditorThemePreference`)
/// joins this list rather than needing a bespoke `th.set`.
trait ComponentTheme {
    /// Every field a magenta placeholder, then replaced field-by-field from the theme file.
    fn placeholder() -> Self;
}

// Field coercion (`kind` → `set_*` fn) and kind lookup — the single mapping, shared by
// `strata_components!` (custom) and `theme_registry!` (builtin). Defined above `strata_components!`'s
// invocation so the custom macro can delegate to them rather than re-listing the mapping.
macro_rules! set_field {
    (color,  $fonts:expr, $dst:expr, $f:expr, $key:expr) => {
        set_color($dst, $f, $key)
    };
    (font,   $fonts:expr, $dst:expr, $f:expr, $key:expr) => {
        set_font($dst, $f, $key, $fonts)
    };
    (f32,    $fonts:expr, $dst:expr, $f:expr, $key:expr) => {
        set_f32($dst, $f, $key)
    };
    (i32,    $fonts:expr, $dst:expr, $f:expr, $key:expr) => {
        set_i32($dst, $f, $key)
    };
    (gaps,   $fonts:expr, $dst:expr, $f:expr, $key:expr) => {
        set_gaps($dst, $f, $key)
    };
    (corner, $fonts:expr, $dst:expr, $f:expr, $key:expr) => {
        set_corner($dst, $f, $key)
    };
}

macro_rules! kind_of {
    (color) => {
        Kind::Color
    };
    (font) => {
        Kind::Font
    };
    (f32) => {
        Kind::F32
    };
    (i32) => {
        Kind::I32
    };
    (gaps) => {
        Kind::Gaps
    };
    (corner) => {
        Kind::Corner
    };
}

/// Placeholder value for a custom-component field of the given kind — magenta for a colour (so an
/// omission is glaringly visible), else a zeroed scalar/gaps/corner. A bare field defaults to colour.
macro_rules! strata_placeholder {
    () => {
        Preference::Specific(Color::from_rgb(255, 0, 255))
    };
    (color) => {
        Preference::Specific(Color::from_rgb(255, 0, 255))
    };
    (f32) => {
        Preference::Specific(0.0_f32)
    };
    (i32) => {
        Preference::Specific(0_i32)
    };
    (gaps) => {
        Preference::Specific(Gaps::default())
    };
    (corner) => {
        Preference::Specific(CornerRadius::default())
    };
    (font) => {
        Preference::Specific(String::new())
    };
}

/// Thin adapters over the shared `set_field!` / `kind_of!` so a *bare* field (no `: kind`) defaults to
/// colour — the only thing `strata_components!` needs beyond the builtin mapping, which stays defined
/// once, in `set_field!` / `kind_of!` (`font` included: it resolves through the theme's `fonts`
/// map, threaded through both macros).
macro_rules! strata_set {
    ($fonts:expr, $dst:expr, $f:expr, $key:expr) => {
        set_color($dst, $f, $key)
    };
    ($fonts:expr, $dst:expr, $f:expr, $key:expr, $kind:ident) => {
        set_field!($kind, $fonts, $dst, $f, $key)
    };
}

macro_rules! strata_kind {
    () => {
        Kind::Color
    };
    (font) => {
        Kind::Font
    };
    ($kind:ident) => {
        kind_of!($kind)
    };
}

macro_rules! strata_components {
    // Each field is a bare ident (colour — the common case) or `ident: kind` (`f32` / `gaps` / `corner`),
    // mirroring `theme_registry!`, so a custom component can carry layout tokens, not just colours.
    ($( $key:literal => $ty:ty { $( $field:ident $(: $kind:ident)? ),* $(,)? } ),* $(,)?) => {
        // `macro_rules!` can't build `$ty { … }` from a fragment, but `Self { … }` inside a trait
        // impl for the concrete type can — and a local-trait impl is allowed for external types too.
        $(
            impl ComponentTheme for $ty {
                fn placeholder() -> Self {
                    Self { $( $field: strata_placeholder!($($kind)?) ),* }
                }
            }
        )*

        /// Register each custom component **from the theme file** (`components.<key>`). No code
        /// defaults: a field the file omits keeps its placeholder. `fonts` is the theme's
        /// `fonts` map, for `font`-kind fields.
        fn register_component_themes(
            th: &mut Theme,
            components: &BTreeMap<String, BTreeMap<String, Pref>>,
            fonts: &BTreeMap<String, String>,
        ) {
            $(
                {
                    let mut p = <$ty as ComponentTheme>::placeholder();
                    if let Some(f) = components.get($key) {
                        $( strata_set!(fonts, &mut p.$field, f, stringify!($field) $(, $kind)?); )*
                    }
                    th.set($key, p);
                }
            )*
        }

        /// Our custom components in [`REGISTRY`]'s shape, so `generate_schema` emits + validates them.
        const CUSTOM_REGISTRY: &[(&str, &[(&str, Kind)])] = &[
            $( ($key, &[ $( (stringify!($field), strata_kind!($($kind)?)) ),* ]) ),*
        ];
    };
}

strata_components! {
    // The **shared** type palette — the seven categorical hues every type-showing surface reads
    // (datagrid header · record view · catalog swatches; the EXPLAIN plan borrows the same ramp for
    // operator kinds). A `%[no_ext]` token group, not a component: stated once here so a theme
    // can't drift between the surfaces that must agree. Keyed by `Kind`, Strata's display taxonomy
    // — not by Arrow. See `components::type_palette`.
    "type_palette" => TypePaletteThemePreference {
        str_color, num_color, bool_color, ts_color, struct_color, list_color, map_color,
    },
    // The header bar's own surface: background, text, and the rule under it. Its switcher paints
    // with root palette colours and the shared `avatar` / `menu` divider — no header-only dress.
    "header_bar" => HeaderBarThemePreference { background, color, border_fill },
    // The launcher / welcome window: its card body + raised rail, the title-bar and rail rules,
    // the two recessive text tones the sheet doesn't name (title strip · group eyebrows), the
    // nav pill's accent tint, and the project-row hover fill plus Remove's danger one.
    // Everything else it paints — the accent, the text ramp, the action hover fill — is a root
    // palette colour; its rail rows are `SidebarRow`'s dress and its tiles `avatar`'s.
    "launcher" => LauncherThemePreference {
        background, rail_background, border_fill, title_color, label_color, nav_background,
        row_hover_background, remove_hover_background,
    },
    // The Settings window: its body + raised category rail, the three rules (title bar, rail
    // edge, footer), the window's gear mark, and the nav tree's dress — group heading, its
    // chevron, a resting row and the current one's pill. `hint_color` is a pane's explanatory
    // subtext, which every category page uses under each setting. Its rows are `SideBarItem`s
    // (so the router's `ActivableRoute` drives selection — see the rail's own module) wearing
    // the shared `sidebar_item` dress, and its footer buttons `button`'s. The `card_*` /
    // `selected_color` / `badge_*` set is the Appearance pane's theme grid (P4-04) — the cards'
    // own dress; the *swatches* inside a card are the previewed theme's colours, not this
    // window's, so they aren't tokens here. The `table_*` set is the Engine pane's properties
    // grid (P4-07): its surface, border, rule and radius are Freya's builtin `table` theme below,
    // and these three are the row states a table cannot have an opinion about, because *which*
    // row is selected or striped is the caller's answer.
    "settings" => SettingsThemePreference {
        background, nav_background, border_fill, icon_color, icon_background, group_color,
        chevron_color, item_color, item_active_background, item_active_color, hint_color,
        card_background, card_border_fill, card_hover_border_fill, card_divider_fill,
        selected_color, badge_builtin_color, badge_user_color,
        table_head_background, table_selection_background, table_zebra_background,
    },
    // The Export window: its body, the raised surface its inset blocks sit on (format cards,
    // text fields, the preview, the two transfer panes), the rules and control edges, the
    // window's download mark, the option labels + their hint glyph, a format card's resting
    // and selected dress, the selected-partition order badge, and the warning banner's tinted
    // box. The banner's *glyph and text* deliberately are not fields here — they take the
    // sheet's `warning`, which is semantic and must follow the app-wide ramp wherever it
    // appears. Its controls are the shared ones: `Switch`, `Select`, `Input`, `Button`, and
    // `segmented_toggle` for every seg group.
    "export" => ExportThemePreference {
        background, panel_background, header_background, border_fill, divider_fill,
        control_border_fill, icon_color,
        icon_background, label_color, hint_color, empty_color, card_color, card_active_background,
        card_active_border_fill, badge_background, badge_color, warning_background,
        warning_border_fill,
    },
    // The initials tile a project row leads with (header switcher, launcher lists): the resting
    // dress plus the accent one a project with an open window wears.
    "avatar" => AvatarThemePreference {
        background, color, active_background, active_color, corner_radius: corner,
    },
    // Freya's own `EditorThemePreference` (the `CodeEditor` reads it off the app `Theme`), but NOT
    // registered by the base theme — so it's a custom component here, not a `theme_registry!` entry.
    "code_editor" => EditorThemePreference {
        background, gutter_selected, gutter_unselected, gutter_border, line_selected_background,
        cursor, highlight, text, whitespace,
        diagnostic_error, diagnostic_warning, diagnostic_info,
        panel_background, panel_border,
        completion_background, completion_border, completion_selected_background,
        completion_detail, completion_kind_table, completion_kind_view, completion_kind_column,
        completion_kind_function, completion_kind_keyword,
        font_family: font, font_size: f32, font_weight: i32, line_height: f32,
    },
    "code_editor_syntax" => EditorSyntaxThemePreference {
        text, whitespace, attribute, boolean, comment, constant, constructor, escape, function,
        function_macro, function_method, keyword, label, module, number, operator, property,
        punctuation, punctuation_bracket, punctuation_delimiter, punctuation_special, string,
        string_escape, string_special, tag, text_literal, text_reference, text_title, text_uri,
        text_emphasis, type_, variable, variable_builtin, variable_parameter,
    },
    // The Run button's three states (idle / disabled / running), each background + hover + fg.
    "run_button" => RunButtonThemePreference {
        background, hover_background, color,
        disabled_background, disabled_hover_background, disabled_color,
        running_background, running_hover_background, running_color,
    },
    // The results pane's Cancel control. Its values track `run_button`'s `running_*` set —
    // the same cancel dress, kept consistent when either is retuned.
    "cancel_button" => CancelButtonThemePreference { background, hover_background, border_fill, color },
    // The whole form vocabulary (`components::form`), one theme for all of it: a window form's
    // uppercase label + its ⓘ, a settings row's title + inline subtext, the rule a divided
    // list draws between rows, and a note's box. What a row *controls* is the caller's child,
    // so no control is dressed here.
    "form" => FormThemePreference {
        title_color, label_color, hint_color, divider_fill, note_background,
        note_border_fill, note_color,
    },
    // The segmented toggle (results Table/Chart switcher, the plan tabs, a form's pill): the
    // container per variant (raised `background` / recessed `form_background`, border,
    // divider) and per-item rest / active (accent-tint) dress.
    "segmented_toggle" => SegmentedToggleThemePreference {
        background, form_background, border_fill, divider_fill, item_color,
        item_active_background, item_active_color,
    },
    // The chrome-less icon toggle (plan Raw/Tree switch, reusable): rest bg + glyph, and the
    // accent-tint active dress — matching the segmented toggle's selected look.
    "toggle_button" => ToggleButtonThemePreference {
        background, color, active_background, active_color,
    },
    // The editor tab strip: `tab_bar` is the container (bg + divider); `editor_tab` is one
    // tab's resting/hover/active bg, label colour, and active accent.
    "tab_bar" => TabBarThemePreference { background, divider_fill },
    "tab" => TabThemePreference {
        background, hover_background, active_background, color, active_color, accent,
    },
    // The results-pane footer: surface bg, mono label colour, 1px top divider, plus the P2-08
    // cluster — muted sub-labels (`sub_color`) and the page-size trigger label
    // (`control_color`). The pager nav buttons are entirely the standard `flat_button` theme
    // (including its `disabled_*` set).
    "status_bar" => StatusBarThemePreference {
        background, color, border_fill, sub_color, control_color,
    },
    // The EXPLAIN plan view (P2-05): the sunken body + card surfaces, the shared hairline
    // (card/box borders, rails, bar track), the text ramp (muted body / values / detail keys /
    // links / raw), the HOTSPOT + time dress, and the categorical operator palette — the same
    // `type_*_color` set the datagrid carries (kind/group/tone mapping lives in the view).
    "explain_plan" => ExplainPlanThemePreference {
        background, card_background, border_fill, group_background, insight_background,
        color, value_color, key_color, muted_color, raw_color, hot_color, warm_color,
    },
    // The nested-cell value modal (P2-12): the dimmed backdrop, the card surface + border, the
    // header (name / dtype badge / ghost close), and the sunken mono JSON body.
    "cell_view" => CellViewThemePreference {
        backdrop, background, border_fill, divider_fill, name_color, badge_color,
        badge_background, close_color, close_hover_background, close_hover_color,
        body_background, body_color,
    },
    // The whole-row record view (P2-10): the same modal dress as `cell_view` (backdrop / card /
    // header divider), plus the field list — name / value / null text tones, the sunken nested
    // block, the field-row hairline, and the dtype labels' categorical `type_*_color` palette
    // (the datagrid header's set). Its buttons are entirely the standard button themes.
    "record_view" => RecordViewThemePreference {
        backdrop, background, border_fill, divider_fill, row_divider_fill, label_color,
        name_color, value_color, null_color, nested_background, nested_color,
    },
    // The catalog sidebar (P3-02): section labels + chevrons, the entry / column row text ramp and
    // hover / selected fills, the nested-column rail, the per-kind entry icons, the PART chip, the
    // validity triangle's tint (P3-04), and the same categorical `type_*_color` palette the grid
    // header wears (column swatch + dtype).
    "catalog" => CatalogThemePreference {
        label_color, chevron_color, name_color, column_color, meta_color, rail_fill,
        table_color, view_color, query_color, part_color, part_background, warn_color,
    },
    // The column inspector (P3-08): the section eyebrows + the facts box's keys, the title's
    // text ramp, the two box surfaces (facts rows / nested-field rows) with their border and
    // row hairline, the completeness bar's three parts, and the source-format badge's tone per
    // format — a closed set, since a theme can only name the formats it knows.
    "inspector" => InspectorThemePreference {
        background, label_color, name_color, value_color, field_color, meta_color, note_color,
        border_fill, divider_fill, box_background, field_background,
        emphasis_color, fill_color, null_color, tile_color,
        format_parquet_color, format_csv_color, format_json_color, format_arrow_color,
        format_view_color,
    },
    // The bottom drawer (P3-01 shell + P3-12 Problems + P3-13 Events + P3-14 History): its surface
    // and the rule under the header, the header's title, a group header's glyph and name, the
    // three text tones a row draws from (message / value / meta), a pressable row's hover surface,
    // the in-list hairline, and the empty state's copy. Named for a row's *role*, not for the tab
    // that first needed one. The severity ramp is NOT here: error / warning / info / success are
    // semantic and come off the sheet wherever they appear.
    "drawer" => DrawerThemePreference {
        background, border_fill, label_color, group_icon_color, group_color,
        meta_color, value_color, message_color, row_hover_fill, divider_fill, empty_color,
    },
    // The results datagrid (our custom virtualized grid — distinct from Freya's builtin `table`):
    // surface, header (name/label/active), row (rest/zebra/hover), selection, gutter, dividers, and
    // per-type cell + dtype-label colours.
    "datagrid" => DataGridThemePreference {
        background, arrow_fill, row_background, zebra_row_background, cell_hover_background,
        selection_border_fill, gutter_color, gutter_active_background,
        gutter_active_color, header_background, header_hover_background, header_color,
        header_label_color, header_active_background,
        header_active_color, divider_fill, column_divider_fill, header_divider_fill,
        cell_num_color, cell_ts_color, color,
        comfortable_cell_padding: gaps, compact_cell_padding: gaps,
    },
}

// ---- the registry (single source: runtime overrides + schema) ------------------------------
// (`set_field!` / `kind_of!` are defined above, before `strata_components!`, so both macros share them.)

/// Emits `apply_component_overrides` (runtime, generic over all listed components) **and** the
/// `REGISTRY` descriptor (for schema generation) from one declarative list.
macro_rules! theme_registry {
    ($( $key:literal => $ty:ty { $( $field:ident : $kind:ident ),* $(,)? } ),* $(,)?) => {
        fn apply_component_overrides(
            th: &mut Theme,
            components: &BTreeMap<String, BTreeMap<String, Pref>>,
            fonts: &BTreeMap<String, String>,
        ) {
            for (name, f) in components {
                match name.as_str() {
                    $(
                        $key => {
                            if let Some(mut p) = th.get::<$ty>($key).cloned() {
                                $( set_field!($kind, fonts, &mut p.$field, f, stringify!($field)); )*
                                th.set($key, p);
                            }
                        }
                    )*
                    _ => {}
                }
            }
        }

        /// Every themeable component + its fields' kinds. Drives [`generate_schema`].
        pub const REGISTRY: &[(&str, &[(&str, Kind)])] = &[
            $( ($key, &[ $( (stringify!($field), kind_of!($kind)) ),* ]) ),*
        ];
    };
}

theme_registry! {
    // Buttons
    "button"          => ButtonColorsThemePreference { background: color, hover_background: color, disabled_background: color, border_fill: color, hover_border_fill: color, focus_border_fill: color, disabled_border_fill: color, color: color, hover_color: color, disabled_color: color },
    "filled_button"   => ButtonColorsThemePreference { background: color, hover_background: color, disabled_background: color, border_fill: color, hover_border_fill: color, focus_border_fill: color, disabled_border_fill: color, color: color, hover_color: color, disabled_color: color },
    "outline_button"  => ButtonColorsThemePreference { background: color, hover_background: color, disabled_background: color, border_fill: color, hover_border_fill: color, focus_border_fill: color, disabled_border_fill: color, color: color, hover_color: color, disabled_color: color },
    "flat_button"     => ButtonColorsThemePreference { background: color, hover_background: color, disabled_background: color, border_fill: color, hover_border_fill: color, focus_border_fill: color, disabled_border_fill: color, color: color, hover_color: color, disabled_color: color },
    "button_layout"          => ButtonLayoutThemePreference { padding: gaps, margin: gaps, corner_radius: corner },
    "compact_button_layout"  => ButtonLayoutThemePreference { padding: gaps, margin: gaps, corner_radius: corner },
    "expanded_button_layout" => ButtonLayoutThemePreference { padding: gaps, margin: gaps, corner_radius: corner },
    // Cards
    "filled_card"  => CardColorsThemePreference { background: color, hover_background: color, border_fill: color, color: color, shadow: color },
    "outline_card" => CardColorsThemePreference { background: color, hover_background: color, border_fill: color, color: color, shadow: color },
    "card_layout"         => CardLayoutThemePreference { padding: gaps, corner_radius: corner },
    "compact_card_layout" => CardLayoutThemePreference { padding: gaps, corner_radius: corner },
    // Inputs
    "input"        => InputColorsThemePreference { background: color, focus_background: color, color: color, placeholder_color: color, border_fill: color, focus_border_fill: color },
    "filled_input" => InputColorsThemePreference { background: color, focus_background: color, color: color, placeholder_color: color, border_fill: color, focus_border_fill: color },
    "flat_input"   => InputColorsThemePreference { background: color, focus_background: color, color: color, placeholder_color: color, border_fill: color, focus_border_fill: color },
    "input_layout"          => InputLayoutThemePreference { corner_radius: corner, inner_margin: gaps },
    "compact_input_layout"  => InputLayoutThemePreference { corner_radius: corner, inner_margin: gaps },
    "expanded_input_layout" => InputLayoutThemePreference { corner_radius: corner, inner_margin: gaps },
    // Toggles
    "switch" => SwitchColorsThemePreference { background: color, thumb_background: color, toggled_background: color, toggled_thumb_background: color, focus_border_fill: color },
    "switch_layout"          => SwitchLayoutThemePreference { margin: gaps, width: f32, height: f32, padding: f32, thumb_size: f32, toggled_thumb_size: f32, pressed_thumb_size_offset: f32, thumb_offset: f32, toggled_thumb_offset: f32 },
    "expanded_switch_layout" => SwitchLayoutThemePreference { margin: gaps, width: f32, height: f32, padding: f32, thumb_size: f32, toggled_thumb_size: f32, pressed_thumb_size_offset: f32, thumb_offset: f32, toggled_thumb_offset: f32 },
    "checkbox" => CheckboxThemePreference { unselected_fill: color, unselected_border_fill: color, selected_fill: color, selected_border_fill: color, selected_icon_fill: color, hover_border_fill: color, focus_border_fill: color },
    "radio"    => RadioItemThemePreference { unselected_fill: color, selected_fill: color, border_fill: color },
    // Selection / overlays
    "select"         => SelectThemePreference { margin: gaps, list_margin: f32, select_background: color, background_button: color, hover_background: color, color: color, border_fill: color, focus_border_fill: color, arrow_fill: color },
    "menu_container" => MenuContainerThemePreference { background: color, padding: gaps, shadow: color, border_fill: color, corner_radius: corner },
    "menu_item"      => MenuItemThemePreference { background: color, hover_background: color, select_background: color, border_fill: color, select_border_fill: color, corner_radius: corner, color: color },
    "popup"          => PopupThemePreference { background: color, color: color, padding: gaps, spacing: f32 },
    "tooltip"        => TooltipThemePreference { background: color, color: color, border_fill: color, font_family: font, font_size: f32, font_weight: i32 },
    // Tabs / segmented
    "floating_tab"     => FloatingTabThemePreference { background: color, hover_background: color, color: color, padding: gaps, corner_radius: corner },
    "segmented_button" => SegmentedButtonThemePreference { background: color, border_fill: color, corner_radius: corner },
    "button_segment"   => ButtonSegmentThemePreference { background: color, hover_background: color, disabled_background: color, selected_background: color, focus_background: color, padding: gaps, selected_padding: gaps, color: color, selected_icon_fill: color },
    // Chips / sidebar / accordion
    "chip"         => ChipThemePreference { background: color, hover_background: color, selected_background: color, border_fill: color, hover_border_fill: color, selected_border_fill: color, focus_border_fill: color, padding: gaps, margin: f32, corner_radius: corner, color: color, hover_color: color, selected_color: color, selected_icon_fill: color, hover_icon_fill: color },
    "sidebar_item" => SideBarItemThemePreference { color: color, background: color, active_background: color, hover_background: color, focus_border_fill: color, corner_radius: corner, margin: gaps, padding: gaps },
    "accordion"    => AccordionThemePreference { color: color, background: color, border_fill: color },
    // Feedback / structure
    "scrollbar"        => ScrollBarThemePreference { background: color, thumb_background: color, hover_thumb_background: color, active_thumb_background: color, size: f32 },
    "progressbar"      => ProgressBarThemePreference { color: color, background: color, progress_background: color, height: f32 },
    "circular_loader"  => CircularLoaderThemePreference { primary_color: color },
    "skeleton"         => SkeletonThemePreference { background: color, corner_radius: corner },
    "resizable_handle" => ResizableHandleThemePreference { background: color, hover_background: color, corner_radius: corner },
    "slider"           => SliderThemePreference { background: color, thumb_background: color, thumb_inner_background: color, border_fill: color },
    "color_picker"     => ColorPickerThemePreference { background: color, border_fill: color, color: color },
    "table"            => TableThemePreference { background: color, arrow_fill: color, row_background: color, hover_row_background: color, divider_fill: color, corner_radius: corner, color: color },
    // NB: Freya's built-in `typography` component (title/subtitle/body/caption/overline) is
    // intentionally NOT registered — Strata's type scale is the richer role-based top-level
    // `typography` section (see `generate_schema` + `crate::components::typography`), not a
    // Freya component override.
}

/// The theme JSON schema for this app: the core model schema
/// ([`strata_core::theme::generate_schema`]) over our two component registries — the builtin
/// overrides ([`REGISTRY`]) and the custom components (`CUSTOM_REGISTRY`). The
/// `schema_in_sync` test keeps `themes/theme.schema.json` equal to this.
pub fn generate_schema() -> serde_json::Value {
    strata_core::theme::generate_schema(&[REGISTRY, CUSTOM_REGISTRY])
}

fn set_color(dst: &mut Preference<Color>, f: &BTreeMap<String, Pref>, key: &str) {
    if let Some(p) = f.get(key) {
        *dst = match p {
            Pref::Reference(slot) => Preference::reference(slot.clone()),
            Pref::Specific(SpecificValue::Color(s)) => Preference::Specific(pc(s)),
            _ => Preference::Specific(Color::from_rgb(255, 0, 255)),
        };
    }
}

/// A `font`-kind field: the authored string is a `fonts` key (`ui`/`mono`) resolved to the real
/// family name, or — when it names no key — a literal family name.
fn set_font(
    dst: &mut Preference<String>,
    f: &BTreeMap<String, Pref>,
    key: &str,
    fonts: &BTreeMap<String, String>,
) {
    if let Some(Pref::Specific(SpecificValue::Color(s))) = f.get(key) {
        let family = fonts.get(s).cloned().unwrap_or_else(|| s.clone());
        *dst = Preference::Specific(family);
    }
}

fn set_f32(dst: &mut Preference<f32>, f: &BTreeMap<String, Pref>, key: &str) {
    if let Some(Pref::Specific(SpecificValue::Scalar(n))) = f.get(key) {
        *dst = Preference::Specific(*n);
    }
}

fn set_i32(dst: &mut Preference<i32>, f: &BTreeMap<String, Pref>, key: &str) {
    if let Some(Pref::Specific(SpecificValue::Scalar(n))) = f.get(key) {
        *dst = Preference::Specific(*n as i32);
    }
}

fn set_gaps(dst: &mut Preference<Gaps>, f: &BTreeMap<String, Pref>, key: &str) {
    match f.get(key) {
        Some(Pref::Specific(SpecificValue::Scalar(n))) => {
            *dst = Preference::Specific(Gaps::new_all(*n))
        }
        Some(Pref::Specific(SpecificValue::Sides([t, r, b, l]))) => {
            *dst = Preference::Specific(Gaps::new(*t, *r, *b, *l))
        }
        _ => {}
    }
}

fn set_corner(dst: &mut Preference<CornerRadius>, f: &BTreeMap<String, Pref>, key: &str) {
    if let Some(Pref::Specific(SpecificValue::Scalar(n))) = f.get(key) {
        *dst = Preference::Specific(CornerRadius::new_all(*n));
    }
}

/// Parse an authored colour: `#rrggbb`, `#rrggbbaa`, or `rgba(r,g,b,a)`. Anything else →
/// magenta, so a bad value is obvious on screen.
///
/// `pub(crate)` for the Settings theme grid, which paints each card's thumbnail from the
/// previewed theme's own authored `sheet` — colours belonging to a theme that is *not* the one
/// installed, so they can't come off a resolved `Theme`.
pub(crate) fn pc(s: &str) -> Color {
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
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
    match hex.len() {
        6 => Color::from_rgb(byte(0), byte(2), byte(4)),
        8 => Color::from_rgb(byte(0), byte(2), byte(4)).with_a(byte(6)),
        _ => Color::from_rgb(255, 0, 255),
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use strata_core::theme::load;

    use super::{generate_schema, Preference};

    /// The committed `theme.schema.json` (root `themes/`, beside the theme files strata-core
    /// embeds) must equal what `generate_schema()` produces — so the schema can't drift from
    /// the registration. Regenerate with
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

    /// Every [`SLOTS`] name must actually resolve through Freya's core-slot match. The two lists
    /// are independent — ours gates `unresolved_references`, the fork's `core_slot` performs the
    /// resolution — so without this a name could validate clean and still paint magenta.
    ///
    /// Works by resolving against a palette with **no** extended names: anything `core_slot`
    /// doesn't know falls through to [`StrataPalette::color`], which answers magenta.
    #[test]
    fn slots_are_freya_core_slots() {
        use super::{to_colors_sheet, StrataPalette};
        use freya::prelude::{Color, ResolvablePreference};
        use std::collections::BTreeMap;
        use strata_core::theme::SLOTS;

        let sheet = to_colors_sheet(&load("midnight").sheet);
        let magenta = Color::from_rgb(255, 0, 255);
        assert!(
            !SLOTS.is_empty() && sheet.primary != magenta,
            "the probe relies on the sheet carrying no magenta"
        );
        let palette = StrataPalette {
            sheet,
            named: BTreeMap::new(),
        };
        for slot in SLOTS {
            let resolved: Color = Preference::reference(*slot).resolve(&palette);
            assert_ne!(
                resolved, magenta,
                "SLOTS entry '{slot}' is not one of Freya's core slots"
            );
        }
    }

    /// Every `reference` in a built-in theme must name a sheet slot or one of that theme's own
    /// `palette` keys. `reference` is an open namespace now, so the JSON schema can't enumerate
    /// the valid targets — this is the check that replaces the closed enum it used to be, and it
    /// is strictly stronger: it validates against the palette the theme actually declares.
    #[test]
    fn references_resolve() {
        for id in ["midnight", "daylight"] {
            let unresolved = load(id).unresolved_references();
            assert!(unresolved.is_empty(), "{id}: {unresolved:?}");
        }
    }

    /// Both committed theme files must parse — the app panics at launch otherwise. (The full
    /// `strata_theme` needs Freya's runtime context, so this pins the pure layers.) Regression:
    /// non-integer scalars (`line_height: 1.6`) broke serde's untagged buffering under the
    /// workspace's `arbitrary_precision` feature; `SpecificValue` now deserializes by hand.
    /// Also pins the `font`-kind path: `"mono"` resolves through `fonts` to a real family name.
    #[test]
    fn theme_files_parse_end_to_end() {
        for id in ["midnight", "daylight"] {
            let t = load(id);
            let editor = t
                .components
                .get("code_editor")
                .expect("code_editor authored");
            match editor.get("line_height") {
                Some(super::Pref::Specific(super::SpecificValue::Scalar(n))) => {
                    assert!((n - 1.6).abs() < f32::EPSILON, "{id}: line_height value")
                }
                _ => panic!("{id}: line_height must parse as a specific scalar"),
            }
            let mut family = Preference::Specific(String::new());
            super::set_font(&mut family, editor, "font_family", &t.fonts);
            assert_eq!(
                family,
                Preference::Specific("JetBrains Mono".to_string()),
                "{id}: fonts-map resolution"
            );
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
            // The applied id, surfaced as text so the assertions read what Freya installed
            // (`Theme.name` carries it) rather than re-deriving it.
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
                    ConfigStation::create_global(cfg.clone()),
                    create_global_theme_preview(),
                );
                let preview = handles.2;
                r.provide_root_context(move || handles);
                preview
            },
            1.,
        );
        assert_eq!(applied(&mut runner), "midnight");

        // The Settings window picks another theme: uncommitted, but live here.
        preview.set(Some(ThemeSel {
            theme: "daylight".to_string(),
            sync_os: false,
        }));
        assert_eq!(applied(&mut runner), "daylight");

        // Cancel drops the slot, and the window falls back to what is committed.
        preview.set(None);
        assert_eq!(applied(&mut runner), "midnight");
    }
}
