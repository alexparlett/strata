use freya::prelude::{rect, Color, Component, ContainerSizeExt, IntoElement, Size, StyleExt};

#[derive(PartialEq)]
pub struct Dot {
    pub color: Color,
}

impl Component for Dot {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::px(7.))
            .height(Size::px(7.))
            .corner_radius(3.5)
            .background(self.color)
    }
}