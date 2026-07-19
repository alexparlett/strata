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

use freya::prelude::*;
use serde::Deserialize;

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
    th
}

// ---- the registry (single source: runtime overrides + schema) ------------------------------

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
    "button"          => ButtonColorsThemePreference { background: color, hover_background: color, border_fill: color, focus_border_fill: color, color: color },
    "filled_button"   => ButtonColorsThemePreference { background: color, hover_background: color, border_fill: color, focus_border_fill: color, color: color },
    "outline_button"  => ButtonColorsThemePreference { background: color, hover_background: color, border_fill: color, focus_border_fill: color, color: color },
    "flat_button"     => ButtonColorsThemePreference { background: color, hover_background: color, border_fill: color, focus_border_fill: color, color: color },
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
    // Typography
    "typography" => TypographyThemePreference { title: f32, subtitle: f32, body: f32, caption: f32, overline: f32 },
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
    for (key, fields) in REGISTRY {
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
            "fonts": { "type": "object", "properties": { "ui": { "type": "string" }, "mono": { "type": "string" } }, "additionalProperties": { "type": "string" } }
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
            "sheet": { "type": "object", "additionalProperties": false, "required": slots, "properties": Value::Object(sheet_props) }
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
