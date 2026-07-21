//! A hairline divider — the 1px rule that separates groups. One place for the pattern that was
//! otherwise re-inlined as a bare `rect` all over (tab strip, menus, toolbars). Horizontal (fills the
//! width) or vertical (fills the height); the cross-axis length, thickness, colour and margin are all
//! overridable, and the colour defaults to the sheet's `border`.

use freya::components::use_theme;
use freya::prelude::*;

/// A 1px separating rule. Build with [`Divider::horizontal`] or [`Divider::vertical`].
#[derive(PartialEq)]
pub struct Divider {
    vertical: bool,
    length: Size,
    thickness: f32,
    color: Option<Color>,
    margin: Gaps,
}

impl Divider {
    /// A horizontal rule: `thickness` tall, filling the available width.
    pub fn horizontal() -> Self {
        Self {
            vertical: false,
            length: Size::fill(),
            thickness: 1.,
            color: None,
            margin: Gaps::new_all(0.),
        }
    }

    /// A vertical rule: `thickness` wide, filling the available height.
    pub fn vertical() -> Self {
        Self {
            vertical: true,
            length: Size::fill(),
            thickness: 1.,
            color: None,
            margin: Gaps::new_all(0.),
        }
    }

    /// Override the cross-axis extent (default: `fill`). Use `Size::px(18.)` for a short group rule,
    /// or `Size::fill_minimum()` inside a hug-content parent (e.g. a menu) where `fill` would blow the
    /// container out to its own parent's width.
    pub fn length(mut self, length: impl Into<Size>) -> Self {
        self.length = length.into();
        self
    }

    /// Override the line thickness (default `1.`).
    pub fn thickness(mut self, thickness: f32) -> Self {
        self.thickness = thickness;
        self
    }

    /// Paint the rule in `color` instead of the default sheet `border`.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Breathing room around the rule (e.g. vertical margin for a menu separator).
    pub fn margin(mut self, margin: impl Into<Gaps>) -> Self {
        self.margin = margin.into();
        self
    }
}

impl Component for Divider {
    fn render(&self) -> impl IntoElement {
        // `use_theme` is a hook, so read it unconditionally and only *then* fall back to it.
        let default_color = use_theme().read().colors.border;
        let color = self.color.unwrap_or(default_color);
        let t = Size::px(self.thickness);
        let base = rect().margin(self.margin).background(color);
        // A fixed `px` thickness holds even in a `Content::Flex` parent, so no min is needed — and a
        // *min* on the thickness is actively wrong: it lets flex distribution grow the line (that's what
        // thickened the menu rules from 1px). The length axis fills (or whatever `length` set).
        if self.vertical {
            base.width(t).height(self.length.clone())
        } else {
            base.height(t).width(self.length.clone())
        }
    }
}
