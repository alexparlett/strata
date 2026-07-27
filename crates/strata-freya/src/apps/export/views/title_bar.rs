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
use crate::components::typography::{Meta, Title};

/// The strip's height (canvas `padding: var(--sp-4) var(--sp-5)` around a 26px tile).
pub const TITLE_BAR_HEIGHT: f32 = 50.;

/// The gutter that keeps the bar's content clear of the OS traffic lights — the same reserve
/// the Settings bar keeps.
const TRAFFIC_LIGHT_GUTTER: f32 = 82.;

#[derive(PartialEq)]
pub struct TitleBar;

impl Component for TitleBar {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let ctx = use_consume::<ExportCtx>();
        let subtitle = ctx.target.read().subtitle();

        // The window's mark: the download glyph in an accent-tinted tile.
        let mark = rect()
            .width(Size::px(26.))
            .height(Size::px(26.))
            .corner_radius(6.)
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
            .spacing(2.)
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
                    .spacing(12.)
                    .padding(Gaps::new(0., 16., 0., TRAFFIC_LIGHT_GUTTER))
                    .window_drag()
                    .child(mark)
                    .child(heading),
            )
            .child(Divider::horizontal().color(theme.border_fill))
    }
}
