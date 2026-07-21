//! The Freya theme, loaded from Strata's **native** theme format (`themes/*.json`).
//!
//! Midnight/Daylight are built-ins (embedded); custom themes load the same shape from a
//! plugin dir (roadmap). A theme file has: a `sheet` copied 1:1 into Freya's `ColorsSheet`
//! (the palette every component references), a `components` map of per-component overrides
//! keyed by **Freya component key**, `tokens` for our own not-yet-built components, and
//! `fonts`. Each component field is a tagged `Preference` — `{ "specific": … }` or
//! `{ "reference": "<sheet slot>" }` — applied as a *partial* merge over Freya's registered
//! default.
//!
//! One `theme_registry!` invocation is the single source of truth: it generates both the
//! runtime override application and the [`REGISTRY`] data that [`generate_schema`] turns into
//! `themes/theme.schema.json`. The `schema_in_sync` test regenerates the schema and fails if
//! the committed file has drifted, so the two can never diverge (regenerate with
//! `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`).

use std::collections::BTreeMap;

use crate::apps::project::{
    DataGridThemePreference, HeaderBarThemePreference, StatusBarThemePreference, TabBarThemePreference,
    TabThemePreference,
};
use crate::components::run_button::RunButtonThemePreference;
use freya::prelude::*;
use serde::Deserialize;
use strata_code_editor::editor_theme::EditorSyntaxThemePreference;
use strata_code_editor::prelude::EditorThemePreference;

const MIDNIGHT_JSON: &str = include_str!("../themes/midnight.json");
const DAYLIGHT_JSON: &str = include_str!("../themes/daylight.json");

/// The 27 `ColorsSheet` slot names — reference targets + the required sheet keys.
const SLOTS: &[&str] = &[
    "primary", "secondary", "tertiary", "success", "warning", "error", "info",
    "background", "surface_primary", "surface_secondary", "surface_tertiary",
    "surface_inverse", "surface_inverse_secondary", "surface_inverse_tertiary",
    "border", "border_focus", "border_disabled",
    "text_primary", "text_secondary", "text_placeholder", "text_inverse", "text_highlight",
    "focus", "active", "disabled", "overlay", "shadow",
];

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Deserialize)]
#[allow(dead_code)]
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
    /// `TypeRole` objects, not the colour `Pref`s every `components.*` map holds. Consumed by the
    /// typography components (`crate::components::typography`) via [`typography`].
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

/// The payload of a `specific` — a colour, a scalar, or four gap sides (distinct JSON types).
#[derive(Deserialize)]
#[serde(untagged)]
pub enum SpecificValue {
    Color(String),
    Scalar(f32),
    Sides([f32; 4]),
}

/// A component field's value type — drives both the runtime coercion and the schema.
#[derive(Clone, Copy)]
pub enum Kind {
    Color,
    F32,
    Gaps,
    Corner,
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

impl SheetDef {
    fn to_colors_sheet(&self) -> ColorsSheet {
        ColorsSheet {
            primary: pc(&self.primary),
            secondary: pc(&self.secondary),
            tertiary: pc(&self.tertiary),
            success: pc(&self.success),
            warning: pc(&self.warning),
            error: pc(&self.error),
            info: pc(&self.info),
            background: pc(&self.background),
            surface_primary: pc(&self.surface_primary),
            surface_secondary: pc(&self.surface_secondary),
            surface_tertiary: pc(&self.surface_tertiary),
            surface_inverse: pc(&self.surface_inverse),
            surface_inverse_secondary: pc(&self.surface_inverse_secondary),
            surface_inverse_tertiary: pc(&self.surface_inverse_tertiary),
            border: pc(&self.border),
            border_focus: pc(&self.border_focus),
            border_disabled: pc(&self.border_disabled),
            text_primary: pc(&self.text_primary),
            text_secondary: pc(&self.text_secondary),
            text_placeholder: pc(&self.text_placeholder),
            text_inverse: pc(&self.text_inverse),
            text_highlight: pc(&self.text_highlight),
            focus: pc(&self.focus),
            active: pc(&self.active),
            disabled: pc(&self.disabled),
            overlay: pc(&self.overlay),
            shadow: pc(&self.shadow),
        }
    }
}

/// Load a Strata theme by id ("midnight" / "daylight"), defaulting to Midnight.
pub fn load(id: &str) -> StrataTheme {
    let json = match id {
        "daylight" => DAYLIGHT_JSON,
        _ => MIDNIGHT_JSON,
    };
    serde_json::from_str(json).expect("strata theme json")
}

/// A Freya `Theme` for the given Strata theme id: our `sheet` + `components` over Freya's
/// light/dark base (which supplies every built-in's default + the layout/typography defaults).
pub fn strata_theme(id: &str) -> Theme {
    let t = load(id);
    let mut th = match t.mode {
        Mode::Light => light_theme(),
        Mode::Dark => dark_theme(),
    };
    // Freya's `Theme.name` is `&'static str`; the id is a runtime string (built-in or custom),
    // so leak it once (negligible, lives for the program).
    th.name = Box::leak(t.id.clone().into_boxed_str());
    th.colors = t.sheet.to_colors_sheet();
    apply_component_overrides(&mut th, &t.components);
    register_component_themes(&mut th, &t.components);
    // Install the resolved type scale onto the theme itself (its `Box<dyn Any>` component store), so
    // typography components read it with a standard `use_theme().get::<Typography>(..)` — no provider,
    // no cache. Keyed `strata_typography` to avoid Freya's built-in `typography`.
    th.set(TYPOGRAPHY_KEY, resolve_typography(&t));
    th
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

/// The resolved type scale for a theme — one [`TextStyle`] per role. Provided at the window root
/// (see `project.rs`) and read by the typography components. Field names mirror the theme file's
/// `typography.<role>` keys.
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

/// The `Theme` key the resolved [`Typography`] scale is installed under (see [`strata_theme`]).
/// Prefixed `strata_` so it never collides with Freya's built-in `typography` component theme.
pub const TYPOGRAPHY_KEY: &str = "strata_typography";

/// Load + resolve the [`Typography`] scale for a theme id. Used to seed the theme (in
/// [`strata_theme`]) and as a defensive fallback; components read the installed copy off the active
/// theme via `use_theme().read().get::<Typography>(`[`TYPOGRAPHY_KEY`]`)`.
pub fn typography(id: &str) -> Typography {
    resolve_typography(&load(id))
}

/// Resolve the scale from an already-loaded theme — each role's `family` key (`ui`/`mono`) looked up
/// in `fonts` to the real family name. A role the file omits falls back to a neutral 13px UI style
/// so text still renders (the theme owns the scale).
fn resolve_typography(t: &StrataTheme) -> Typography {
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
    (color,  $dst:expr, $f:expr, $key:expr) => { set_color($dst, $f, $key) };
    (f32,    $dst:expr, $f:expr, $key:expr) => { set_f32($dst, $f, $key) };
    (gaps,   $dst:expr, $f:expr, $key:expr) => { set_gaps($dst, $f, $key) };
    (corner, $dst:expr, $f:expr, $key:expr) => { set_corner($dst, $f, $key) };
}

macro_rules! kind_of {
    (color)  => { Kind::Color };
    (f32)    => { Kind::F32 };
    (gaps)   => { Kind::Gaps };
    (corner) => { Kind::Corner };
}

/// Placeholder value for a custom-component field of the given kind — magenta for a colour (so an
/// omission is glaringly visible), else a zeroed scalar/gaps/corner. A bare field defaults to colour.
macro_rules! strata_placeholder {
    () => { Preference::Specific(Color::from_rgb(255, 0, 255)) };
    (color) => { Preference::Specific(Color::from_rgb(255, 0, 255)) };
    (f32) => { Preference::Specific(0.0_f32) };
    (gaps) => { Preference::Specific(Gaps::default()) };
    (corner) => { Preference::Specific(CornerRadius::default()) };
}

/// Thin adapters over the shared `set_field!` / `kind_of!` so a *bare* field (no `: kind`) defaults to
/// colour — the only thing `strata_components!` needs beyond the builtin mapping, which stays defined
/// once, in `set_field!` / `kind_of!`.
macro_rules! strata_set {
    ($dst:expr, $f:expr, $key:expr) => { set_color($dst, $f, $key) };
    ($dst:expr, $f:expr, $key:expr, $kind:ident) => { set_field!($kind, $dst, $f, $key) };
}

macro_rules! strata_kind {
    () => { Kind::Color };
    ($kind:ident) => { kind_of!($kind) };
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
        /// defaults: a field the file omits keeps its placeholder.
        fn register_component_themes(th: &mut Theme, components: &BTreeMap<String, BTreeMap<String, Pref>>) {
            $(
                {
                    let mut p = <$ty as ComponentTheme>::placeholder();
                    if let Some(f) = components.get($key) {
                        $( strata_set!(&mut p.$field, f, stringify!($field) $(, $kind)?); )*
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
    "header_bar" => HeaderBarThemePreference { background, color, border_fill },
    // Freya's own `EditorThemePreference` (the `CodeEditor` reads it off the app `Theme`), but NOT
    // registered by the base theme — so it's a custom component here, not a `theme_registry!` entry.
    "code_editor" => EditorThemePreference {
        background, gutter_selected, gutter_unselected, gutter_border, line_selected_background,
        cursor, highlight, text, whitespace,
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
    // The editor tab strip: `tab_bar` is the container (bg + divider); `editor_tab` is one
    // tab's resting/hover/active bg, label colour, and active accent.
    "tab_bar" => TabBarThemePreference { background, divider_fill },
    "tab" => TabThemePreference {
        background, hover_background, active_background, color, active_color, accent,
    },
    // The results-pane footer: surface bg, mono label colour, 1px top divider, future pager hover.
    "status_bar" => StatusBarThemePreference {
        background, color, border_fill, hover_background,
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
        cell_num_color, cell_ts_color, type_str_color, type_num_color, type_bool_color,
        type_ts_color, type_struct_color, type_list_color, type_map_color, color,
        comfortable_cell_padding: gaps, compact_cell_padding: gaps,
    },
}

// ---- the registry (single source: runtime overrides + schema) ------------------------------
// (`set_field!` / `kind_of!` are defined above, before `strata_components!`, so both macros share them.)

/// Emits `apply_component_overrides` (runtime, generic over all listed components) **and** the
/// `REGISTRY` descriptor (for schema generation) from one declarative list.
macro_rules! theme_registry {
    ($( $key:literal => $ty:ty { $( $field:ident : $kind:ident ),* $(,)? } ),* $(,)?) => {
        fn apply_component_overrides(th: &mut Theme, components: &BTreeMap<String, BTreeMap<String, Pref>>) {
            for (name, f) in components {
                match name.as_str() {
                    $(
                        $key => {
                            if let Some(mut p) = th.get::<$ty>($key).cloned() {
                                $( set_field!($kind, &mut p.$field, f, stringify!($field)); )*
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
    "button"          => ButtonColorsThemePreference { background: color, hover_background: color, border_fill: color, hover_border_fill: color, focus_border_fill: color, color: color, hover_color: color },
    "filled_button"   => ButtonColorsThemePreference { background: color, hover_background: color, border_fill: color, hover_border_fill: color, focus_border_fill: color, color: color, hover_color: color },
    "outline_button"  => ButtonColorsThemePreference { background: color, hover_background: color, border_fill: color, hover_border_fill: color, focus_border_fill: color, color: color, hover_color: color },
    "flat_button"     => ButtonColorsThemePreference { background: color, hover_background: color, border_fill: color, hover_border_fill: color, focus_border_fill: color, color: color, hover_color: color },
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
    "checkbox" => CheckboxThemePreference { unselected_fill: color, selected_fill: color, selected_icon_fill: color, border_fill: color },
    "radio"    => RadioItemThemePreference { unselected_fill: color, selected_fill: color, border_fill: color },
    // Selection / overlays
    "select"         => SelectThemePreference { margin: gaps, select_background: color, background_button: color, hover_background: color, color: color, border_fill: color, focus_border_fill: color, arrow_fill: color },
    "menu_container" => MenuContainerThemePreference { background: color, padding: gaps, shadow: color, border_fill: color, corner_radius: corner },
    "menu_item"      => MenuItemThemePreference { background: color, hover_background: color, select_background: color, border_fill: color, select_border_fill: color, corner_radius: corner, color: color },
    "popup"          => PopupThemePreference { background: color, color: color, padding: gaps, spacing: f32 },
    "tooltip"        => TooltipThemePreference { background: color, color: color, border_fill: color, font_size: f32 },
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

/// Build the JSON schema from [`REGISTRY`] + the sheet slots. The `schema_in_sync` test keeps
/// `themes/theme.schema.json` equal to this.
pub fn generate_schema() -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let ref_for = |k: &Kind| match k {
        Kind::Color => "#/$defs/colorPref",
        Kind::F32 | Kind::Corner => "#/$defs/numberPref",
        Kind::Gaps => "#/$defs/gapsPref",
    };

    let mut components = Map::new();
    for (key, fields) in REGISTRY.iter().chain(CUSTOM_REGISTRY.iter()) {
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

fn set_color(dst: &mut Preference<Color>, f: &BTreeMap<String, Pref>, key: &str) {
    if let Some(p) = f.get(key) {
        *dst = match p {
            Pref::Reference(slot) => Preference::Reference(sheet_slot(slot)),
            Pref::Specific(SpecificValue::Color(s)) => Preference::Specific(pc(s)),
            _ => Preference::Specific(Color::from_rgb(255, 0, 255)),
        };
    }
}

fn set_f32(dst: &mut Preference<f32>, f: &BTreeMap<String, Pref>, key: &str) {
    if let Some(Pref::Specific(SpecificValue::Scalar(n))) = f.get(key) {
        *dst = Preference::Specific(*n);
    }
}

fn set_gaps(dst: &mut Preference<Gaps>, f: &BTreeMap<String, Pref>, key: &str) {
    match f.get(key) {
        Some(Pref::Specific(SpecificValue::Scalar(n))) => *dst = Preference::Specific(Gaps::new_all(*n)),
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

/// Map a `reference` slot name to the `&'static str` Freya's `Preference::Reference` needs
/// (the 27 `ColorsSheet` slots; unknown → `primary`, so a typo shows).
fn sheet_slot(s: &str) -> &'static str {
    SLOTS.iter().copied().find(|&slot| slot == s).unwrap_or("primary")
}

/// Parse an authored colour: `#rrggbb`, `#rrggbbaa`, or `rgba(r,g,b,a)`. Anything else →
/// magenta, so a bad value is obvious on screen.
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
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
    match hex.len() {
        6 => Color::from_rgb(byte(0), byte(2), byte(4)),
        8 => Color::from_rgb(byte(0), byte(2), byte(4)).with_a(byte(6)),
        _ => Color::from_rgb(255, 0, 255),
    }
}

#[cfg(test)]
mod tests {
    use super::generate_schema;

    /// The committed `theme.schema.json` must equal what `generate_schema()` produces — so the
    /// schema can't drift from the registration. Regenerate with
    /// `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`.
    #[test]
    fn schema_in_sync() {
        let generated = generate_schema();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/themes/theme.schema.json");
        if std::env::var_os("UPDATE_SCHEMA").is_some() {
            let out = serde_json::to_string_pretty(&generated).unwrap() + "\n";
            std::fs::write(path, out).unwrap();
        } else {
            let committed: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(
                committed, generated,
                "theme.schema.json is stale — run `UPDATE_SCHEMA=1 cargo test -p strata-freya schema_in_sync`"
            );
        }
    }
}
