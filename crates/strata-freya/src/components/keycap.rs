//! A **key cap** — a chord, or one key of one, drawn as something you press.
//!
//! Two surfaces draw these and they must agree on what a key looks like: Settings ▸ Keymap, where
//! a row's chord is the thing being edited, and the command palette, where a row's chord is a hint
//! and `ESC` labels the way out. Before this they were the same three colours authored twice — the
//! `settings` theme carried a `keycap_*` trio no other window could name, which is the split
//! the theme conventions rule out: a component's own dress is the component's, not a consuming surface's.
//!
//! ## The two shapes, named rather than averaged
//!
//! The canvases genuinely differ, so both are here and neither is a compromise between them:
//!
//! - [`key`](KeyCap::key) — the **raised** cap the Keymap grid draws: 24px tall, a 22px floor so a
//!   single character still reads as a key, radius 6, and a heavier bottom edge. That edge is the
//!   whole message; it is what makes a cap read as a key rather than a chip, on the one surface
//!   where the user is about to press it.
//! - [`chip`](KeyCap::chip) — the **flat** hint the palette draws: radius 4, a uniform hairline,
//!   and the canvas's 2/8 inset. A palette row's chord is something you are being *told*, so it
//!   sits back; making it a raised key would put a control-shaped thing in a list of results.
//!
//! One cap per call, never a chord: the Keymap grid splits a chord into a cap per key
//! ([`chord_caps`](strata_core::keymap::chord_caps)) and the palette shows it as one run, and that
//! difference belongs to the caller rather than to a component with a mode.

use freya::components::{define_theme, get_theme};
use freya::prelude::*;

use crate::components::metrics::{R_1, R_XS, SP_1, SP_3};
use crate::components::typography::MonoValue;

define_theme!(
    %[no_ext]
    %[component]
    pub KeyCapColors {
        %[fields]
        background: Color,
        border_fill: Color,
        color: Color,
    }
);

/// This window's key-cap colours. A `%[no_ext]` token group rather than a component theme,
/// because the same three colours dress a cap wherever one appears — the same reason
/// [`type_palette`](super::type_palette) is one.
pub fn keycap_colors() -> KeyCapColorsTheme {
    get_theme!(
        &None::<KeyCapColorsThemePartial>,
        KeyCapColorsThemePreference,
        "keycap"
    )
}

/// A key cap's floor — a single character still has to read as a key.
const KEY_MIN_WIDTH: f32 = 22.;
const KEY_HEIGHT: f32 = 24.;
const KEY_INSET: f32 = SP_3;
const KEY_RADIUS: f32 = R_1;
/// The heavier bottom edge that makes a cap read as a key rather than a chip.
const KEY_EDGE: f32 = 1.;
const KEY_BOTTOM_EDGE: f32 = 2.;

/// The flat hint's inset and radius (canvas `padding: var(--sp-1) var(--sp-3)`, `--r-xs`).
const CHIP_INSET_Y: f32 = SP_1;
const CHIP_INSET_X: f32 = SP_3;
const CHIP_RADIUS: f32 = R_XS;
const CHIP_EDGE: f32 = 1.;

/// Which of the two dresses — see the module doc.
#[derive(PartialEq, Clone, Copy)]
enum Shape {
    Key,
    Chip,
}

#[derive(PartialEq)]
pub struct KeyCap {
    text: String,
    shape: Shape,
    color: Option<Color>,
}

impl KeyCap {
    /// The raised cap: Settings ▸ Keymap's, where the chord is what the row is about.
    pub fn key(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            shape: Shape::Key,
            color: None,
        }
    }

    /// The flat hint: the command palette's shortcut chip and its `ESC` label.
    pub fn chip(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            shape: Shape::Chip,
            color: None,
        }
    }

    /// Override the label's tone. For the palette, whose two chips sit at different distances —
    /// a row's shortcut hint against the `ESC` beside the search field. The box is unchanged;
    /// only what it says is nearer or further away.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

impl Component for KeyCap {
    fn render(&self) -> impl IntoElement {
        let colors = keycap_colors();
        let (radius, edge, padding) = match self.shape {
            Shape::Key => (
                KEY_RADIUS,
                BorderWidth {
                    top: KEY_EDGE,
                    right: KEY_EDGE,
                    bottom: KEY_BOTTOM_EDGE,
                    left: KEY_EDGE,
                },
                Gaps::new(0., KEY_INSET, 0., KEY_INSET),
            ),
            Shape::Chip => (
                CHIP_RADIUS,
                BorderWidth::from(CHIP_EDGE),
                Gaps::new(CHIP_INSET_Y, CHIP_INSET_X, CHIP_INSET_Y, CHIP_INSET_X),
            ),
        };

        rect()
            .maybe(self.shape == Shape::Key, |el| {
                el.min_width(Size::px(KEY_MIN_WIDTH))
                    .height(Size::px(KEY_HEIGHT))
            })
            .padding(padding)
            .center()
            .corner_radius(radius)
            .background(colors.background)
            .border(Border::new().width(edge).fill(colors.border_fill))
            .child(MonoValue::new(self.text.clone()).color(self.color.unwrap_or(colors.color)))
    }
}
