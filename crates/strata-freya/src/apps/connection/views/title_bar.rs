//! The connection editor's title bar — **and the window's**: a 50px strip carrying the store
//! tile, what the window is doing and which connection it is doing it to, with the real OS
//! traffic lights floating at its left.
//!
//! Nothing here is interactive, so the whole strip is the drag region — the same recipe as the
//! Configure, Export, Settings and launcher bars.

use freya::prelude::*;

use crate::apps::connection::ConnectionCtx;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{
    COMPACT_BUTTON, R_1, SP_1, SP_4, SP_5, TITLE_BAR_HEIGHT, TRAFFIC_LIGHT_GUTTER,
};
use crate::components::typography::{Meta, Title};
use crate::components::window::window_theme;
use crate::theme::{use_roles, Role};

#[derive(PartialEq)]
pub struct TitleBar;

impl Component for TitleBar {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        // The subtitle's tone. Through the roles because the window theme has no text field —
        // it dresses chrome (surfaces, rules, the mark), and a recessive line of prose is the
        // semantic ramp's, exactly as the Configure window's status block reads it.
        let muted = use_roles().get(Role::TextMuted);
        let ctx = use_consume::<ConnectionCtx>();
        let target = ctx.target.read();

        // The window's mark: the same glyph the Connections pane and the activity rail use, in
        // an accent-tinted tile.
        let mark = rect()
            .width(Size::px(COMPACT_BUTTON))
            .height(Size::px(COMPACT_BUTTON))
            .corner_radius(R_1)
            .center()
            .background(win.icon_background)
            .child(
                Icon::new(IconName::Connections)
                    .size(15.)
                    .color(win.icon_color),
            );

        // Title over the connection it names — the canvas's two-line block, and one line on a
        // new connection, whose URL is still being typed.
        let heading = rect()
            .vertical()
            .spacing(SP_1)
            .width(Size::flex(1.))
            .child(Title::new(target.title()))
            .maybe_child(
                target
                    .subtitle()
                    .map(|url| Meta::new(url).color(muted).into_element()),
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
                    .child(heading),
            )
            .child(Divider::horizontal().color(win.border_fill))
    }
}
