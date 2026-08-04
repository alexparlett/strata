use freya::prelude::*;

use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Prose, Title};
use crate::theme::{use_roles, Role};

/// The results pane before any rows exist: a rounded icon tile over a title + hint, centered.
#[derive(PartialEq)]
pub struct EmptyState;

impl Component for EmptyState {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        let (tile_bg, tile_border, icon_color, title_color, sub_color, background) = (
            roles.get(Role::ElementBackground),
            roles.get(Role::Border),
            roles.get(Role::TextPlaceholder),
            roles.get(Role::TextMuted),
            roles.get(Role::TextPlaceholder),
            roles.get(Role::SurfaceRaised),
        );

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
