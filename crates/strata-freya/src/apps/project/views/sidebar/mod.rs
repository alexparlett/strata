//! The left **sidebar**: the frame (P3-01) plus the catalog pane that fills it (P3-02).
//!
//! The shell owns the header row and the collapse (×); what sits to the left of the × is the
//! active pane's, per the design canvas — the catalog puts its **filter + refresh** there (there
//! is no "CATALOG" label; the filter field is the header), while Connections (W7) keeps a plain
//! section label. The body below the divider is the pane itself.

mod catalog;

use freya::components::{use_theme, Input};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::SidebarPane;

use self::catalog::Catalog;
pub use self::catalog::CatalogThemePreference;
use crate::apps::project::state::{Chan, SessionState};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Eyebrow, InputTypography};

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
        let theme = use_theme();
        let (bg, border, label_color, faint) = {
            let t = theme.read();
            (
                t.colors.surface_secondary,
                t.colors.border,
                t.colors.text_placeholder,
                t.colors.text_placeholder,
            )
        };

        // The catalog filter lives in the header beside the refresh button, but its consumer is
        // the tree below — so the shell owns the signal and hands it down.
        let filter = use_state(String::new);

        let leading = match pane {
            SidebarPane::Catalog => rect()
                .width(Size::flex(1.))
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(8.)
                .child(
                    InputTypography::mono(
                        Input::new(filter)
                            .placeholder("Filter catalog…")
                            .compact()
                            .leading(Icon::new(IconName::Search).color(faint).size(13.))
                            .width(Size::fill()),
                    )
                    .width(Size::flex(1.)),
                )
                // Inert until P3-03 wires `Engine::refresh_catalog` — the affordance ships with
                // the surface it belongs to, its behaviour with the task that owns it.
                .child(
                    Button::new()
                        .flat()
                        .width(Size::px(24.))
                        .height(Size::px(24.))
                        .child(Icon::new(IconName::Reload).size(14.)),
                )
                .into_element(),
            SidebarPane::Connections => rect()
                .width(Size::flex(1.))
                .horizontal()
                .cross_align(Alignment::Center)
                .child(Eyebrow::new("CONNECTIONS").color(label_color))
                .into_element(),
        };

        let body = match pane {
            SidebarPane::Catalog => Catalog::new(filter).into_element(),
            // The connections pane is W7's; the frame is here so the rail's toggle has somewhere
            // to land.
            SidebarPane::Connections => rect().expanded().into_element(),
        };

        rect()
            .expanded()
            .background(bg)
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(48.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(8.)
                    .padding((0., 12.))
                    .child(leading)
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
            .child(body)
    }
}
