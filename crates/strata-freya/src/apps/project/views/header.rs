use crate::components::divider::Divider;
use freya::prelude::{define_theme, get_theme, rect, ChildrenExt, Color, Component, ContainerSizeExt, ContainerWithContentExt, Content, IntoElement, Size, StyleExt};


define_theme!(
    %[component]
    pub HeaderBar {
        %[fields]
        background: Color,
        color: Color,
        border_fill: Color,
    }
);

#[derive(PartialEq)]
pub struct HeaderBar {
    pub theme: Option<HeaderBarThemePartial>,
}

impl HeaderBar {
    pub fn new() -> Self {
        Self {
            theme: None,
        }
    }
}

impl Component for HeaderBar {
    fn render(&self) -> impl IntoElement {
        // `color` + `border_fill` are themed now (from the designer's `header`) but not painted
        // until the header grows its content + divider; the theme owns the whole definition.
        let HeaderBarTheme { background, border_fill, color } =
            get_theme!(&self.theme, HeaderBarThemePreference, "header_bar");
        rect()
            .background(background)
            .color(color)
            .content(Content::Flex)
            .height(Size::px(48.))
            .width(Size::fill())
            .child(
                rect().width(Size::fill()).height(Size::flex(1.))
            )
            .child(Divider::horizontal().color(border_fill))
    }
}