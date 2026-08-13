//! The Settings window's title bar — **and the window's**: a 50px strip carrying the gear
//! tile and the window's name, with the real OS traffic lights floating at its left (the
//! window ships transparent-titlebar + fullsize-content-view, see [`SettingsApp::window`]).
//!
//! Nothing here is interactive, so the whole strip is the drag region — the fork's
//! `window_drag` recipe straight, like the launcher's. The project header tracks *whose* fill
//! a double-press was because its geometry is persisted; this window's isn't.
//!
//! [`SettingsApp::window`]: crate::apps::settings::SettingsApp::window

use freya::prelude::*;

use crate::apps::settings::settings_theme;
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{
    COMPACT_BUTTON, R_1, SP_4, SP_5, TITLE_BAR_HEIGHT, TRAFFIC_LIGHT_GUTTER,
};
use crate::components::typography::Title;

#[derive(PartialEq)]
pub struct TitleBar;

impl Component for TitleBar {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();

        let mark = rect()
            .width(Size::px(COMPACT_BUTTON))
            .height(Size::px(COMPACT_BUTTON))
            .corner_radius(R_1)
            .center()
            .background(theme.icon_background)
            .child(Icon::new(IconName::Gear).size(15.).color(theme.icon_color));

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
                    .cross_align(Alignment::Center)
                    .spacing(SP_4)
                    .padding(Gaps::new(0., SP_5, 0., TRAFFIC_LIGHT_GUTTER))
                    .window_drag()
                    .child(mark)
                    .child(Title::new("Settings")),
            )
            .child(Divider::horizontal().color(theme.border_fill))
    }
}
