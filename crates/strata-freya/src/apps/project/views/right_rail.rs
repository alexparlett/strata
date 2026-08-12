//! The **right** activity rail (AS-04) — the 48px strip down the window's other edge (design
//! `Strata.dc.html` `data-rg="rightrail"`), picking which assistive surface the right pane
//! shows: the column inspector, or the assistant's chat.
//!
//! [`ActivityRail`](super::rail::ActivityRail)'s mechanism verbatim — a [`ToggleButton`] per
//! pane, `on` *derived* from the layout rather than held here, and a press routing through the
//! layout store's toggle so pressing the lit one puts the pane away. What differs is only what
//! the two edges are *about*: the left rail lists the project's **data** surfaces (catalog,
//! agents, connections) and its diagnostics, and this one lists the surfaces that assist
//! whatever is in the middle. A pane on this edge is therefore a single choice rather than a
//! group of independent panels — which is what keeps a 1180px window readable with both rails,
//! a sidebar and the drawer up.
//!
//! No badge on either button. The inspector has nothing to count, and a chat's unread count is
//! a notion for a surface somebody else writes into — the assistant only ever answers a send
//! the user just made, in a pane they are looking at.

use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::RightPane;

use crate::apps::project::state::{Chan, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_1, SP_3};
use crate::components::toggle_button::{ChangeEventData, ToggleButton};
use crate::theme::{use_roles, Role};

#[derive(PartialEq)]
pub struct RightRail;

impl RightRail {
    pub fn new() -> Self {
        Self
    }
}

impl Component for RightRail {
    fn render(&self) -> impl IntoElement {
        // On `Chan::Layout` for the left rail's reason: a pane switch redresses the buttons, a
        // resize drag (`Chan::LayoutSize`) must not.
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let right = radio.read().layout.right;
        let background = use_roles().get(Role::SurfaceBackground);

        let button = move |icon: IconName, title: &str, pane: RightPane| {
            ToggleButton::new()
                .width(Size::px(40.))
                .height(Size::px(38.))
                .toggle(right == Some(pane))
                .title(title)
                .on_change(move |_: Event<ChangeEventData>| {
                    let mut radio = radio;
                    radio.write_channel(Chan::Layout).toggle_right_pane(pane);
                })
                .child(Icon::new(icon).size(18.))
        };

        rect()
            .width(Size::px(48.))
            .height(Size::fill())
            .background(background)
            .cross_align(Alignment::Center)
            .padding((SP_3, 0.))
            .spacing(SP_1)
            .child(button(
                IconName::Inspector,
                "Column inspector",
                RightPane::Inspector,
            ))
            .child(button(IconName::Chat, "Assistant", RightPane::Chat))
    }
}
