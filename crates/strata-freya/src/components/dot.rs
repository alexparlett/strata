//! A small colour swatch. Round by default (the tab strip's dirty marker); `.square()` gives the
//! softly-rounded square the catalog uses as a column's type swatch.

use freya::prelude::{rect, Color, Component, ContainerSizeExt, IntoElement, Size, StyleExt};

/// The round marker's default diameter — the tab-strip dirty dot.
const DEFAULT_SIZE: f32 = 7.;
/// A square swatch's corner rounding (design `--r-xs`).
const SQUARE_RADIUS: f32 = 4.;

#[derive(PartialEq)]
pub struct Dot {
    color: Color,
    size: f32,
    square: bool,
}

impl Dot {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            size: DEFAULT_SIZE,
            square: false,
        }
    }

    /// Override the swatch's edge length (both axes).
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Draw a softly-rounded square instead of a circle.
    pub fn square(mut self) -> Self {
        self.square = true;
        self
    }
}

impl Component for Dot {
    fn render(&self) -> impl IntoElement {
        let radius = if self.square {
            SQUARE_RADIUS
        } else {
            self.size / 2.
        };
        rect()
            .width(Size::px(self.size))
            .height(Size::px(self.size))
            .corner_radius(radius)
            .background(self.color)
    }
}
