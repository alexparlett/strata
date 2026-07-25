//! The left **sidebar** shell (P3-01) — the tool-pane frame the catalog (P3-02) and connections
//! (W7) grow into. It renders the active pane's section header + a collapse (×) over
//! `surface_secondary`; the body is intentionally empty until its content task lands. Which pane
//! it identifies (Catalog / Connections) follows the layout — the rail's top group selects it.

use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::SidebarPane;

use crate::apps::project::state::{Chan, SessionState};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Eyebrow;

#[derive(PartialEq)]
pub struct Sidebar;

impl Sidebar {
    pub fn new() -> Self {
        Self
    }
}

impl Component for Sidebar {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let pane = radio.read().layout.sidebar.unwrap_or(SidebarPane::Catalog);
        let label = match pane {
            SidebarPane::Catalog => "CATALOG",
            SidebarPane::Connections => "CONNECTIONS",
        };
        let theme = use_theme();
        let (bg, border, label_color) = {
            let t = theme.read();
            (
                t.colors.surface_secondary,
                t.colors.border,
                t.colors.text_placeholder,
            )
        };

        rect()
            .expanded()
            .background(bg)
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(40.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::SpaceBetween)
                    .padding((0., 12.))
                    .child(Eyebrow::new(label).color(label_color))
                    .child(
                        Button::new()
                            .flat()
                            .width(Size::px(24.))
                            .height(Size::px(24.))
                            .on_press(move |_| {
                                let mut radio = radio;
                                radio.write_channel(Chan::Layout).close_sidebar();
                            })
                            .child(Icon::new(IconName::Close).size(13.)),
                    ),
            )
            .child(Divider::horizontal().color(border))
            // Empty body — the catalog tree / filter (P3-02) and connections pane (W7) fill it.
            .child(rect().expanded())
    }
}
