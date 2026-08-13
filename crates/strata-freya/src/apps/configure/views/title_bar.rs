//! The Configure window's title bar — **and the window's**: a 50px strip carrying the table
//! mark and the window's name, with the real OS traffic lights floating at its left.
//!
//! Nothing here is interactive, so the whole strip is the drag region — the same recipe as the
//! Export, Settings and launcher bars.

use freya::prelude::*;

use crate::apps::configure::ConfigureCtx;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{
    COMPACT_BUTTON, R_1, SP_4, SP_5, TITLE_BAR_HEIGHT, TRAFFIC_LIGHT_GUTTER,
};
use crate::components::typography::Title;
use crate::components::window::window_theme;

#[derive(PartialEq)]
pub struct TitleBar;

impl Component for TitleBar {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let title = ctx.target.read().title();

        let mark = rect()
            .width(Size::px(COMPACT_BUTTON))
            .height(Size::px(COMPACT_BUTTON))
            .corner_radius(R_1)
            .center()
            .background(win.icon_background)
            .child(
                Icon::new(IconName::Database)
                    .size(15.)
                    .color(win.icon_color),
            );

        rect()
            .width(Size::fill())
            .height(Size::px(TITLE_BAR_HEIGHT))
            .vertical()
            .content(Content::Flex)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(SP_4)
                    .padding(Gaps::new(0., SP_5, 0., TRAFFIC_LIGHT_GUTTER))
                    .window_drag()
                    .child(mark)
                    .child(rect().width(Size::flex(1.)).child(Title::new(title))),
            )
            .child(Divider::horizontal().color(win.border_fill))
    }
}
