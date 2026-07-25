//! The launcher's title bar — **and the window's**: a 38px strip carrying only the
//! centred "Welcome to Strata", with the real OS traffic lights floating at its left
//! (the window ships transparent-titlebar + fullsize-content-view, see
//! [`LauncherApp::window`]).
//!
//! Nothing here is interactive, so the whole strip is the drag region — the fork's
//! `window_drag` recipe straight, unlike the project header, which tracks *whose* fill a
//! double-press was because its geometry is persisted. The launcher's isn't.
//!
//! [`LauncherApp::window`]: crate::apps::launcher::LauncherApp::window

use freya::prelude::*;

use crate::apps::launcher::{LauncherThemePartial, LauncherThemePreference};
use crate::components::divider::Divider;
use crate::components::typography::Control;

/// The strip's height (canvas `height: 38px`); the traffic-light inset is derived from it.
pub const TITLE_BAR_HEIGHT: f32 = 38.;

#[derive(PartialEq)]
pub struct TitleBar;

impl Component for TitleBar {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<LauncherThemePartial>,
            LauncherThemePreference,
            "launcher"
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
                    .center()
                    .window_drag()
                    .child(Control::new("Welcome to Strata").color(theme.title_color)),
            )
            .child(Divider::horizontal().color(theme.border_fill))
    }
}
