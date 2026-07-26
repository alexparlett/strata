//! The activity rail — the permanent 48px tool-window strip down the left edge (design
//! `ActivityRail.dc.html` / `Strata.dc.html` `data-rg="rail"`, RustRover-style).
//!
//! Two groups of icon toggles over the panel `surface_primary`: the top group selects the
//! sidebar's tool pane (Catalog · Connections), the bottom group the drawer's tab (Problems ·
//! Events · History). Each button is a standard [`ToggleButton`] (reusing the `toggle_button`
//! theme, whose transparent-rest / accent-soft-active dress already matches the rail), sized to
//! the rail's 40×38. Its `on` state is *derived* from the layout — the single source of truth —
//! and a press routes through the layout store's toggle (`onRailPane` / `onOpen*` semantics):
//! open the pane/tab, or collapse it if it's already the active one. The Connections **button**
//! lives here; its sidebar pane content is W7.

use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::{DrawerTab, SidebarPane};

use crate::apps::project::state::{Chan, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::toggle_button::{ChangeEventData, ToggleButton};

#[derive(PartialEq)]
pub struct ActivityRail;

impl ActivityRail {
    pub fn new() -> Self {
        Self
    }
}

impl Component for ActivityRail {
    fn render(&self) -> impl IntoElement {
        // Subscribe on `Chan::Layout` — a collapse / pane switch re-renders the rail's active
        // dress, but a resize drag (on `Chan::LayoutSize`) does not.
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let layout = radio.read().layout;
        let background = use_theme().read().colors().surface_primary;

        // A rail toggle: 40×38, `on` derived from the layout, a press routing to `toggle`
        // (a fn pointer — `toggle_pane` / `toggle_drawer` for the chosen pane / tab).
        let button =
            move |icon: IconName, title: &str, active: bool, toggle: fn(&mut SessionState)| {
                ToggleButton::new()
                    .width(Size::px(40.))
                    .height(Size::px(38.))
                    .toggle(active)
                    .title(title)
                    .on_change(move |_: Event<ChangeEventData>| {
                        let mut radio = radio;
                        toggle(&mut radio.write_channel(Chan::Layout));
                    })
                    .child(Icon::new(icon).size(18.))
            };

        rect()
            .width(Size::px(48.))
            .height(Size::fill())
            .background(background)
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .padding((8., 0.))
            .spacing(2.)
            // Top group — the sidebar's tool panes.
            .child(button(
                IconName::Database,
                "Catalog",
                layout.sidebar == Some(SidebarPane::Catalog),
                |s| s.toggle_pane(SidebarPane::Catalog),
            ))
            .child(button(
                IconName::Connections,
                "Connections",
                layout.sidebar == Some(SidebarPane::Connections),
                |s| s.toggle_pane(SidebarPane::Connections),
            ))
            // Flexible spacer pushes the diagnostics group to the bottom.
            .child(rect().width(Size::px(1.)).height(Size::flex(1.)))
            // Bottom group — the drawer's diagnostics tabs.
            .child(button(
                IconName::Problems,
                "Problems",
                layout.drawer == Some(DrawerTab::Problems),
                |s| s.toggle_drawer(DrawerTab::Problems),
            ))
            .child(button(
                IconName::Lines,
                "Events",
                layout.drawer == Some(DrawerTab::Events),
                |s| s.toggle_drawer(DrawerTab::Events),
            ))
            .child(button(
                IconName::Clock,
                "History",
                layout.drawer == Some(DrawerTab::History),
                |s| s.toggle_drawer(DrawerTab::History),
            ))
    }
}
