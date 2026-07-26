//! The bottom **drawer** shell (P3-01) — the frame the tabbed diagnostics panel (P3-11) and its
//! Problems / Events / History content (P3-12..14) grow into. For now it shows the active tab's
//! title (chosen by the rail's bottom group) + a collapse (×) over `surface_secondary`; the real
//! tab strip + Clear are P3-11, and the body is empty until then. Its top border is the resize
//! handle above it, so the shell draws none.

use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::DrawerTab;

use crate::apps::project::state::{Chan, SessionState};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Caption;

#[derive(PartialEq)]
pub struct Drawer;

impl Drawer {
    pub fn new() -> Self {
        Self
    }
}

impl Component for Drawer {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let tab = radio.read().layout.drawer.unwrap_or(DrawerTab::Problems);
        let title = match tab {
            DrawerTab::Problems => "Problems",
            DrawerTab::Events => "Events",
            DrawerTab::History => "History",
        };
        let theme = use_theme();
        let (bg, border, title_color) = {
            let t = theme.read();
            (
                t.colors().surface_secondary,
                t.colors().border,
                t.colors().text_secondary,
            )
        };

        rect()
            .expanded()
            .background(bg)
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(36.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::SpaceBetween)
                    .padding((0., 12.))
                    .child(Caption::new(title).color(title_color))
                    .child(
                        Button::new()
                            .flat()
                            .width(Size::px(24.))
                            .height(Size::px(24.))
                            .on_press(move |_| {
                                let mut radio = radio;
                                radio.write_channel(Chan::Layout).close_drawer();
                            })
                            .child(Icon::new(IconName::Close).size(13.)),
                    ),
            )
            .child(Divider::horizontal().color(border))
            // Empty body — the tabbed Problems / Events / History content fills it (P3-11..14).
            .child(rect().expanded())
    }
}
