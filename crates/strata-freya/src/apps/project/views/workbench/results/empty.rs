use freya::components::use_theme;
use freya::prelude::*;

use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Prose, Title};

/// The results pane before any rows exist: a rounded icon tile over a title + hint, centered.
#[derive(PartialEq)]
pub struct EmptyState;

impl Component for EmptyState {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let (tile_bg, tile_border, icon_color, title_color, sub_color, background) = {
            let c = &theme.read().colors;
            (
                c.surface_tertiary,
                c.border,
                c.text_placeholder,
                c.text_secondary,
                c.text_placeholder,
                c.surface_secondary,
            )
        };

        rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .spacing(12.)
            .background(background)
            .child(
                rect()
                    .width(Size::px(46.))
                    .height(Size::px(46.))
                    .corner_radius(8.)
                    .background(tile_bg)
                    .border(Border::new().width(1.).fill(tile_border))
                    .center()
                    .child(Icon::new(IconName::Rows).color(icon_color).size(22.)),
            )
            .child(Title::new("No results yet").color(title_color))
            .child(
                Prose::new("Run the query to load rows from your sources into the grid.")
                    .color(sub_color),
            )
    }
}
