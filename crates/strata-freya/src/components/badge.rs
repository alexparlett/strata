//! A **badge** — a small tinted pill carrying a short, static label: the catalog's `PART` marker,
//! the plan view's `HOTSPOT` / `ANALYZE` flags and its insight pills, the nested-cell modal's dtype
//! chip.
//!
//! Deliberately **not** Freya's [`Chip`]: that is a selectable control —
//! `on_press`, `selected`, `enabled`, keyboard focus, an a11y id, a cursor icon, and colour triples
//! for hover / selected / focus. A badge is a label. Rendering one as a `Chip` would give it press
//! and focus semantics it must not have, and leave most of that theme inert.
//!
//! Two type roles, matching the two the views already used:
//!
//! - [`tag`](Badge::tag) — an all-caps marker (`PART`, `HOTSPOT`), tracked mono at 10 (`Eyebrow`).
//! - [`value`](Badge::value) — a value-ish run (a dtype, an insight sentence), mono at 10 (`Meta`).
//!
//! The fill is either passed explicitly (where a theme owns that surface — `part_background`,
//! `badge_background`, `insight_background`) or derived from the foreground at [`TINT_ALPHA`]. The
//! derivation is the point: the two sites that tinted their own colour had drifted to alpha 41 and
//! 38 for no reason anyone chose.

use freya::prelude::*;

use crate::components::typography::{Eyebrow, Meta};

/// Alpha of a badge's fill when it is derived from the foreground colour (≈16%).
const TINT_ALPHA: u8 = 40;

/// Which type role dresses the label.
#[derive(PartialEq, Clone, Copy)]
enum BadgeRole {
    /// Tracked small-caps — a marker word.
    Tag,
    /// Plain mono meta — a value or a short sentence.
    Value,
}

#[derive(PartialEq)]
pub struct Badge {
    text: String,
    role: BadgeRole,
    color: Color,
    background: Option<Color>,
    radius: f32,
    padding: Gaps,
    height: Option<f32>,
}

impl Badge {
    fn build(text: impl Into<String>, color: Color, role: BadgeRole) -> Self {
        Self {
            text: text.into(),
            role,
            color,
            background: None,
            radius: 4.,
            // A tag hugs tighter than a value run — the two paddings the views already used.
            padding: match role {
                BadgeRole::Tag => Gaps::new(2., 4., 2., 4.),
                BadgeRole::Value => Gaps::new(2., 8., 2., 8.),
            },
            height: None,
        }
    }

    /// An all-caps marker word (`PART`, `HOTSPOT`, `ANALYZE`).
    pub fn tag(text: impl Into<String>, color: Color) -> Self {
        Self::build(text, color, BadgeRole::Tag)
    }

    /// A value-ish run — a dtype, an insight sentence.
    pub fn value(text: impl Into<String>, color: Color) -> Self {
        Self::build(text, color, BadgeRole::Value)
    }

    /// Pin the fill instead of deriving it from the foreground. Use where a theme owns that
    /// surface as its own token.
    pub fn background(mut self, background: Color) -> Self {
        self.background = Some(background);
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    pub fn padding(mut self, padding: impl Into<Gaps>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Fix the badge's height (and centre its label within it) instead of hugging the label.
    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }
}

impl Component for Badge {
    fn render(&self) -> impl IntoElement {
        let background = self.background.unwrap_or(self.color.with_a(TINT_ALPHA));
        let label = match self.role {
            BadgeRole::Tag => Eyebrow::new(self.text.clone())
                .color(self.color)
                .into_element(),
            BadgeRole::Value => Meta::new(self.text.clone())
                .color(self.color)
                .into_element(),
        };
        rect()
            .padding(self.padding)
            .corner_radius(self.radius)
            .background(background)
            .map(self.height, |el, h| el.height(Size::px(h)).center())
            .child(label)
    }
}
