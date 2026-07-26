//! The bottom **drawer** shell (P3-01) — the frame the Problems / Events / History content
//! (P3-12..14) grows into. It shows the active tab's title (chosen by the rail's bottom group,
//! which is the design's tab switcher — the canvas's drawer-header tab pills were computed and
//! never rendered) over `surface_secondary`, plus the header's expand / restore and collapse
//! controls. The count label, Clear and the body itself belong to the content tasks. Its top
//! border is the resize handle above it, so the shell draws none.

use freya::components::{use_theme, Tooltip, TooltipContainer};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::DrawerTab;

use super::shell::set_drawer_panel_height;
use crate::apps::project::state::{Chan, SessionState};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Caption;

#[derive(PartialEq)]
pub struct Drawer {
    /// The controller for the vertical container this drawer is a panel of — the expand toggle
    /// resizes the live panel through it. Required, not defaulted: a drawer holding its own
    /// throwaway controller would toggle nothing.
    sizing: State<ResizableContext>,
}

impl Drawer {
    pub fn new(sizing: State<ResizableContext>) -> Self {
        Self { sizing }
    }
}

impl Component for Drawer {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let layout = radio.read().layout;
        let tab = layout.drawer.unwrap_or(DrawerTab::Problems);
        let title = match tab {
            DrawerTab::Problems => "Problems",
            DrawerTab::Events => "Events",
            DrawerTab::History => "History",
        };
        // The remembered restore height *is* the expanded flag — see
        // `SessionState::toggle_drawer_height`.
        let expanded = layout.drawer_restore_h.is_some();
        let theme = use_theme();
        let (bg, border, title_color) = {
            let t = theme.read();
            (
                t.colors().surface_secondary,
                t.colors().border,
                t.colors().text_secondary,
            )
        };

        let sizing = self.sizing;
        let expand = Button::new()
            .flat()
            .width(Size::px(24.))
            .height(Size::px(24.))
            .on_press(move |_| {
                let mut radio = radio;
                // Two halves of one resize: the layout write re-seeds the panel's
                // `initial_size` (its next mount, and the session file), and the controller
                // moves the panel that is on screen now.
                let h = radio.write_channel(Chan::Layout).toggle_drawer_height();
                set_drawer_panel_height(sizing, h);
            })
            .child(
                Icon::new(if expanded {
                    IconName::ChevronsDown
                } else {
                    IconName::ChevronsUp
                })
                .size(14.),
            );

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
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(2.)
                            .child(
                                TooltipContainer::new(Tooltip::new(if expanded {
                                    "Restore"
                                } else {
                                    "Expand"
                                }))
                                .position(AttachedPosition::Bottom)
                                .child(expand),
                            )
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
                    ),
            )
            .child(Divider::horizontal().color(border))
            // Empty body — the Problems / Events / History content fills it (P3-12..14).
            .child(rect().expanded())
    }
}
