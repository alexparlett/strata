//! Typography — one component per concrete **role** from the design-system type scale, mirroring
//! the dioxus `typography.rs`. The app renders text through these (never a raw `label()` with an
//! inline `font_size`) so the scale is the single source of truth: each role fixes family + size
//! (+ weight / line-height / letter-spacing) from the theme file's top-level `typography` section,
//! resolved once into a [`Typography`] and provided at the window root (see `project.rs`).
//!
//! Roles (family · weight · size): **Hero** (ui 600 26) · **Metric** (mono 600 22) · **Code**
//! (mono 500 20) · **Title** (ui 600 14.5) · **Strong** (ui 600 13) · **Body** (ui 500 13,
//! default UI text) · **Control** (ui 600 12.5, button/control label) · **Prose** (ui 400 12.5) ·
//! **Caption** (ui 400 11) · **MonoValue** (mono 500 12.5) · **Readout** (mono 400 12) ·
//! **Eyebrow** (mono 600 10, tracked) · **Meta** (mono 500 10) · **Path** (mono 400 11).
//!
//! Deliberately **not** an enum-variant API — every role is its own component so call sites read
//! `Title::new(..)`, `Meta::new(..)`, matching the dioxus ergonomics. The role fixes the *type*
//! (family · weight · size · tracking); everything else is a fluent override on the underlying
//! `label()` — `.color()`, `.align()`, `.width()`/`.max_width()`, `.max_lines()`/`.wrap()`,
//! `.text_overflow()` — so layout tweaks never mean dropping back to a raw `label()`.

use crate::theme::{TextStyle, Typography};
use freya::components::use_theme;
use freya::prelude::*;

/// The active type scale — read straight off the active `Theme`, where [`strata_theme`] installed it
/// under [`TYPOGRAPHY_KEY`]. A standard theme lookup (the same context mechanism every component
/// theme uses): no provider, no cache. The `unwrap_or_else` is defensive — the theme always seeds it.
///
/// [`strata_theme`]: crate::theme::strata_theme
/// [`TYPOGRAPHY_KEY`]: crate::theme::TYPOGRAPHY_KEY
fn scale() -> Typography {
    let theme = use_theme();
    let theme = theme.read();
    theme
        .get::<Typography>(crate::theme::TYPOGRAPHY_KEY)
        .cloned()
        .unwrap_or_else(|| crate::theme::typography(theme.name))
}

/// The overridable `label()` properties every typography role exposes. Every field is opt-in
/// (defaults to unset ⇒ inherit the ambient colour, default alignment, hug the content, `label()`'s
/// own truncation) except the single-line cap, so a bare `Body::new(..)` renders as it always did.
#[derive(PartialEq, Clone)]
struct TextOverrides {
    color: Option<Color>,
    align: Option<TextAlign>,
    /// Fix the text-box width; unset ⇒ the box hugs its content.
    width: Option<Size>,
    /// Cap how wide the box may grow; unset ⇒ no cap.
    max_width: Option<Size>,
    /// Line cap. `None` ⇒ unbounded (wraps across as many lines as needed).
    max_lines: Option<usize>,
    /// How a too-long, line-capped run is truncated.
    overflow: Option<TextOverflow>,
}

// Fully qualified: `CursorIcon::Default` is imported into scope above, so name `Default` on its own
// is ambiguous here.
impl std::default::Default for TextOverrides {
    fn default() -> Self {
        Self {
            color: None,
            align: None,
            width: None,
            max_width: None,
            max_lines: Some(1),
            overflow: None,
        }
    }
}

/// Apply a resolved role to a fresh `label()`: family + size + weight + line-height + letter-spacing
/// from the scale, then the caller's [`TextOverrides`] (colour · alignment · width · max-width · line
/// cap · truncation). Letter-spacing rides our Freya fork's `letter_spacing(..)`; a `None` line cap
/// clears `max_lines` so the text wraps freely.
fn styled(style: &TextStyle, text: &str, o: &TextOverrides) -> impl IntoElement {
    let o = o.clone();
    let mut el = label()
        .font_family(style.family.clone())
        .font_size(style.size)
        .font_weight(style.weight)
        .line_height(style.line_height)
        .letter_spacing(style.letter_spacing)
        .max_lines(o.max_lines)
        .text(text.to_string());
    if let Some(width) = o.width {
        el = el.width(width);
    }
    if let Some(max_width) = o.max_width {
        el = el.max_width(max_width);
    }
    if let Some(color) = o.color {
        el = el.color(color);
    }
    if let Some(align) = o.align {
        el = el.text_align(align);
    }
    if let Some(overflow) = o.overflow {
        el = el.text_overflow(overflow);
    }
    el
}

/// Generate one `Component` per role — `$Comp::new(text).color(c).width(..)`, reading `$field` off
/// the scale. Every role shares the same fluent [`TextOverrides`] builders, so any run can be
/// coloured, aligned, sized, line-capped or wrapped without dropping to a raw `label()`.
macro_rules! roles {
    ($( $(#[$doc:meta])* $Comp:ident => $field:ident ),* $(,)?) => {
        $(
            $(#[$doc])*
            #[derive(PartialEq)]
            pub struct $Comp {
                text: String,
                overrides: TextOverrides,
            }

            impl $Comp {
                /// A run of this role: ambient colour, default alignment, hugging its content on a
                /// single line.
                pub fn new(text: impl Into<String>) -> Self {
                    Self { text: text.into(), overrides: TextOverrides::default() }
                }

                /// Paint the text in `color` instead of inheriting the parent's.
                pub fn color(mut self, color: Color) -> Self {
                    self.overrides.color = Some(color);
                    self
                }

                /// Set the text alignment (default: start).
                pub fn align(mut self, align: TextAlign) -> Self {
                    self.overrides.align = Some(align);
                    self
                }

                /// Fix the text-box width (`Size::px(160.)`, `Size::flex(1.)`, …) so a long run
                /// truncates or wraps within it instead of hugging its content.
                pub fn width(mut self, width: impl Into<Size>) -> Self {
                    self.overrides.width = Some(width.into());
                    self
                }

                /// Cap how wide the text box may grow; it still hugs shorter content.
                pub fn max_width(mut self, max_width: impl Into<Size>) -> Self {
                    self.overrides.max_width = Some(max_width.into());
                    self
                }

                /// Cap the number of lines (default 1); the last line truncates per the overflow.
                pub fn max_lines(mut self, max_lines: usize) -> Self {
                    self.overrides.max_lines = Some(max_lines);
                    self
                }

                /// Let the text wrap across as many lines as it needs (removes the line cap).
                pub fn wrap(mut self) -> Self {
                    self.overrides.max_lines = None;
                    self
                }

                /// How a too-long, line-capped run is truncated (unset ⇒ the `label()` default, clip).
                pub fn text_overflow(mut self, overflow: TextOverflow) -> Self {
                    self.overrides.overflow = Some(overflow);
                    self
                }
            }

            impl Component for $Comp {
                fn render(&self) -> impl IntoElement {
                    styled(&scale().$field, &self.text, &self.overrides)
                }
            }
        )*
    };
}

roles! {
    /// Hero display — specimens / welcome (ui · 600 · 26).
    Hero => display,
    /// Data metric — a big mono figure (mono · 600 · 22).
    Metric => data_display,
    /// Code display — inline code / SQL at display size (mono · 500 · 20).
    Code => code_display,
    /// Title / window heading (ui · 600 · 14.5).
    Title => title,
    /// Strong body — emphasised UI text (ui · 600 · 13).
    Strong => strong_body,
    /// Body — the default UI text role (ui · 500 · 13).
    Body => body_medium,
    /// Control — a button / control label (ui · 600 · 12.5).
    Control => control,
    /// Prose — descriptions & secondary text (ui · 400 · 12.5).
    Prose => body,
    /// Caption — small supporting text (ui · 400 · 11).
    Caption => caption,
    /// Mono value — inline data / figures (mono · 500 · 12.5).
    MonoValue => data_value,
    /// Readout — a flowing code / data block (mono · 400 · 12).
    Readout => code_block,
    /// Eyebrow — an uppercase field label (mono · 600 · 10, tracked).
    Eyebrow => field_label,
    /// Meta — a recessive mono label / timestamp (mono · 500 · 10).
    Meta => meta,
    /// Path — a recessive mono path / footer (mono · 400 · 11).
    Path => mono_path,
}
