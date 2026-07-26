//! The window frame — the router's **layout**, so it is mounted once and survives every
//! navigation: the title bar, then the category rail beside the pane, then the footer.
//!
//! Being the layout is the point. The pane is the only thing a category change touches, so
//! the nav's collapsed groups, the scroll frame and the footer's state all outlive it.

use freya::prelude::*;
use freya::router::*;

use crate::apps::settings::views::footer::Footer;
use crate::apps::settings::views::nav::Nav;
use crate::apps::settings::views::title_bar::TitleBar;
use crate::apps::settings::{Route, SettingsThemePartial, SettingsThemePreference};

#[derive(PartialEq)]
pub struct SettingsChrome;

impl Component for SettingsChrome {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );

        rect()
            .expanded()
            .vertical()
            .content(Content::Flex)
            .background(theme.background)
            .child(TitleBar)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .horizontal()
                    .content(Content::Flex)
                    .child(Nav)
                    .child(Outlet::<Route>::new()),
            )
            .child(Footer)
    }
}
