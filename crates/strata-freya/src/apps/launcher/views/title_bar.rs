//! The launcher's title bar — **and the window's**: a 38px strip carrying only the
//! centred "Welcome to Strata", with the real OS traffic lights floating at its left
//! (the window ships transparent-titlebar + fullsize-content-view, see
//! [`LauncherApp::window`]).
//!
//! The strip's background is the drag region — the fork's `window_drag` recipe straight, unlike the
//! project header, which tracks *whose* fill a double-press was because its geometry is persisted.
//! The launcher's isn't. Off macOS the [`WindowControls`] pinned at its right are the one
//! interactive thing in it, and they stop the pointer-down themselves.
//!
//! [`LauncherApp::window`]: crate::apps::launcher::LauncherApp::window

use freya::prelude::*;

use crate::apps::launcher::{LauncherThemePartial, LauncherThemePreference};
use crate::components::chrome::WindowControls;
use crate::components::divider::Divider;
use crate::components::menu_bar::MenuBar;
use crate::components::metrics::{SP_1, WINDOW_CONTROLS_GUTTER};
use crate::components::typography::Control;
use crate::menu::MenuScope;

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
                    // Symmetric padding, so the title stays centred in the strip rather than in
                    // the space left of the controls. Zero on macOS, where the traffic lights are
                    // at the leading edge and their gutter is the same number on the other side.
                    .padding(Gaps::new(
                        0.,
                        WINDOW_CONTROLS_GUTTER,
                        0.,
                        WINDOW_CONTROLS_GUTTER,
                    ))
                    .window_drag()
                    .child(Control::new("Welcome to Strata").color(theme.title_color)),
            )
            .child(Divider::horizontal().color(theme.border_fill))
            .child(
                rect()
                    .position(Position::new_absolute().top(0.).left(SP_1))
                    .height(Size::px(TITLE_BAR_HEIGHT))
                    .cross_align(Alignment::Center)
                    .child(MenuBar::new(use_consume::<MenuScope>())),
            )
            .child(WindowControls::new(TITLE_BAR_HEIGHT))
    }
}
