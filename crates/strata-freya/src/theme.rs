//! The Freya theme, loaded from Strata's **native** theme format (`themes/*.json`).
//!
//! Unlike the Dioxus app's CSS-variable tokens, these files are authored directly against
//! what the app renders: a `sheet` block copied **1:1** into Freya's `ColorsSheet` (so every
//! built-in component's `Preference::Reference(...)` resolves to a real Strata colour), plus
//! `tokens` for our hand-rolled widgets (editor syntax, grid cells) — consumed as those land.
//! See `docs/FREYA_THEME_SPEC.md`.

use std::collections::BTreeMap;

use freya::prelude::*;
use serde::Deserialize;

const MIDNIGHT_JSON: &str = include_str!("../themes/midnight.json");
const DAYLIGHT_JSON: &str = include_str!("../themes/daylight.json");

/// Light/dark base — picks Freya's `light_theme()` / `dark_theme()` to sit under the sheet.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

/// A Strata theme file. `tokens`/`fonts` are parsed (format validated) and consumed by our
/// components + font setup as they arrive.
#[derive(Deserialize)]
#[allow(dead_code)]
pub struct StrataTheme {
    pub id: String,
    pub name: String,
    pub mode: Mode,
    pub sheet: SheetDef,
    #[serde(default)]
    pub tokens: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub fonts: BTreeMap<String, String>,
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
    /// 1:1 into Freya's `ColorsSheet` — no semantic mapping, just parse each authored colour.
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

/// A Freya `Theme` for the given Strata theme id — the loaded `sheet` over Freya's matching
/// light/dark base (which supplies the per-component layout/typography defaults we override
/// later).
pub fn strata_theme(id: &str) -> Theme {
    let t = load(id);
    let mut th = match t.mode {
        Mode::Light => light_theme(),
        Mode::Dark => dark_theme(),
    };
    th.colors = t.sheet.to_colors_sheet();
    th
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
