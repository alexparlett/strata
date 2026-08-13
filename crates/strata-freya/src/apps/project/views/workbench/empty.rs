//! The workbench's "no query open" empty state, shown when every tab is closed: a centred
//! database-icon tile, a title + one-line prompt, and New-query / Reopen-closed actions. Ported from
//! the Dioxus `.ws-empty` — its saved-queries list waits on saved queries landing in Freya. The
//! New-query button wears the effective new-tab chord as an inline key-cap chip (the comp's chip,
//! keymap-derived so a rebind repaints it); Reopen names the tab it would restore in its tooltip.

use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::config::Command;

use crate::apps::project::state::{Chan, SessionState};
use crate::components::badge::Badge;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_4, SP_1, SP_3, SP_4, SP_5, SP_8};
use crate::components::typography::{Control, Prose, Title};
use crate::keymap::use_hint;
use crate::theme::{use_roles, Role};

/// The centre-pane placeholder when the session has no open tabs.
#[derive(PartialEq)]
pub struct EmptyState;

impl EmptyState {
    pub fn new() -> Self {
        Self
    }
}

impl Component for EmptyState {
    fn render(&self) -> impl IntoElement {
        let mut radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let last_closed = radio.read().closed.last().map(|(_, t)| t.name.clone());
        let new_hint = use_hint(Command::NewTab);

        let roles = use_roles();
        let (background, tile_bg, tile_border, icon_c, title_c, sub_c, chip_c) = (
            roles.get(Role::SurfaceRaised),
            roles.get(Role::ElementBackground),
            roles.get(Role::Border),
            roles.get(Role::TextPlaceholder),
            roles.get(Role::Text),
            roles.get(Role::TextMuted),
            roles.get(Role::TextOnAccent),
        );

        let tile = rect()
            .width(Size::px(60.))
            .height(Size::px(60.))
            .corner_radius(R_4)
            .background(tile_bg)
            .border(Border::new().width(1.).fill(tile_border))
            .center()
            .margin(Gaps::new(0., 0., SP_5, 0.))
            .child(Icon::new(IconName::Database).color(icon_c).size(26.));

        let chip = (!new_hint.is_empty()).then(|| {
            Badge::value(new_hint.clone(), chip_c.with_a(153))
                .background(chip_c.with_a(51))
                .padding((SP_1, 5.))
        });
        let new_btn = Button::new()
            .filled()
            .on_press(move |_| {
                radio.write().open_blank();
            })
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
                    .child(Icon::new(IconName::Plus).size(14.))
                    .child(Control::new("New query"))
                    .maybe_child(chip),
            );

        let reopen_btn = last_closed.map(|name| {
            TooltipContainer::new(Tooltip::new_text(format!("Reopen {name}")))
                .position(AttachedPosition::Top)
                .child(
                    Button::new()
                        .on_press(move |_| {
                            radio.write().reopen_last();
                        })
                        .child(
                            rect()
                                .horizontal()
                                .cross_align(Alignment::Center)
                                .spacing(SP_3)
                                .child(Icon::new(IconName::Reopen).size(14.))
                                .child(Control::new("Reopen closed")),
                        ),
                )
        });

        let actions = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(SP_4)
            .child(new_btn)
            .maybe_child(reopen_btn);

        rect()
            .width(Size::fill())
            .height(Size::fill())
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .padding(Gaps::new(SP_8, SP_8, SP_8, SP_8))
            .background(background)
            .child(tile)
            .child(
                rect()
                    .margin(Gaps::new(0., 0., SP_3, 0.))
                    .child(Title::new("No query open").color(title_c)),
            )
            .child(
                rect()
                    .margin(Gaps::new(0., 0., SP_5, 0.))
                    .cross_align(Alignment::Center)
                    .child(
                        Prose::new(
                            "Open a new query tab to explore your data, or run SELECT * on a table \
                             from the catalog.",
                        )
                            .color(sub_c)
                            .align(TextAlign::Center)
                            .max_width(Size::px(340.))
                            .wrap(),
                    ),
            )
            .child(actions)
    }
}
