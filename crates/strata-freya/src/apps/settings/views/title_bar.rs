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

use crate::apps::settings::{SettingsThemePartial, SettingsThemePreference};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Title;

/// The strip's height (canvas `padding: var(--sp-4) var(--sp-5)` around a 26px tile); the
/// traffic-light inset is derived from it.
pub const TITLE_BAR_HEIGHT: f32 = 50.;

/// The gutter that keeps the bar's content clear of the OS traffic lights. The window insets
/// them to (16, 17), so the three buttons end around x = 68; the canvas's gear tile starts at
/// 82, which is the same reserve the project header keeps.
const TRAFFIC_LIGHT_GUTTER: f32 = 82.;

#[derive(PartialEq)]
pub struct TitleBar;

impl Component for TitleBar {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );

        // The window's mark: the gear in an accent-tinted tile, then its name in the scale's
        // Title role (ui 600 14.5, the comp's).
        let mark = rect()
            .width(Size::px(26.))
            .height(Size::px(26.))
            .corner_radius(6.)
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
                    .spacing(12.)
                    .padding(Gaps::new(0., 16., 0., TRAFFIC_LIGHT_GUTTER))
                    .window_drag()
                    .child(mark)
                    .child(Title::new("Settings")),
            )
            .child(Divider::horizontal().color(theme.border_fill))
    }
}
