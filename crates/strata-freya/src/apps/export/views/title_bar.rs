//! The Export window's title bar — **and the window's**: a 50px strip carrying the download
//! tile, the window's name and the run it is exporting, with the real OS traffic lights
//! floating at its left.
//!
//! Nothing here is interactive, so the whole strip is the drag region — the same recipe as the
//! Settings and launcher bars.

use freya::prelude::*;

use crate::apps::export::{ExportCtx, ExportThemePartial, ExportThemePreference};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{
    COMPACT_BUTTON, R_1, SP_1, SP_4, SP_5, TITLE_BAR_HEIGHT, TRAFFIC_LIGHT_GUTTER,
};
use crate::components::typography::{Meta, Title};

#[derive(PartialEq)]
pub struct TitleBar;

impl Component for TitleBar {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let ctx = use_consume::<ExportCtx>();
        let subtitle = ctx.target.read().subtitle();

        // The window's mark: the download glyph in an accent-tinted tile.
        let mark = rect()
            .width(Size::px(COMPACT_BUTTON))
            .height(Size::px(COMPACT_BUTTON))
            .corner_radius(R_1)
            .center()
            .background(theme.icon_background)
            .child(
                Icon::new(IconName::Download)
                    .size(15.)
                    .color(theme.icon_color),
            );

        // Title over the run it describes — the canvas's two-line block.
        let heading = rect()
            .vertical()
            .spacing(SP_1)
            .width(Size::flex(1.))
            .child(Title::new("Export results"))
            .child(Meta::new(subtitle).color(theme.label_color));

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
            .child(Divider::horizontal().color(theme.border_fill))
    }
}
