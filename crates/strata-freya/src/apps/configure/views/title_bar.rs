//! The Configure window's title bar — **and the window's**: a 50px strip carrying the table
//! mark and the window's name, with the real OS traffic lights floating at its left.
//!
//! Nothing here is interactive, so the whole strip is the drag region — the same recipe as the
//! Export, Settings and launcher bars.

use freya::prelude::*;

use crate::apps::configure::ConfigureCtx;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Title;
use crate::components::window::window_theme;

/// The strip's height (canvas `padding: var(--sp-4) var(--sp-5)` around a 26px tile).
pub const TITLE_BAR_HEIGHT: f32 = 50.;

/// The gutter that keeps the bar's content clear of the OS traffic lights — the same reserve
/// every other window's bar keeps.
const TRAFFIC_LIGHT_GUTTER: f32 = 82.;

#[derive(PartialEq)]
pub struct TitleBar;

impl Component for TitleBar {
    fn render(&self) -> impl IntoElement {
        let win = window_theme();
        let ctx = use_consume::<ConfigureCtx>();
        let title = ctx.target.read().title();

        // The window's mark: the canvas's layered-strata glyph in an accent-tinted tile.
        let mark = rect()
            .width(Size::px(26.))
            .height(Size::px(26.))
            .corner_radius(6.)
            .center()
            .background(win.icon_background)
            .child(
                Icon::new(IconName::Database)
                    .size(15.)
                    .color(win.icon_color),
            );

        // One line, not the export bar's two: the canvas gives this window a title and no
        // subtitle, and the name it would carry is the title.
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
                    .spacing(12.)
                    .padding(Gaps::new(0., 16., 0., TRAFFIC_LIGHT_GUTTER))
                    .window_drag()
                    .child(mark)
                    .child(rect().width(Size::flex(1.)).child(Title::new(title))),
            )
            .child(Divider::horizontal().color(win.border_fill))
    }
}
