//! The workbench's "no query open" empty state, shown when every tab is closed: a centred
//! database-icon tile, a title + one-line prompt, and New-query / Reopen-closed actions. Ported from
//! the Dioxus `.ws-empty` — its saved-queries list and the button keyboard hints wait on those
//! features landing in Freya.

use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;

use crate::apps::project::state::{Chan, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Control, Prose, Title};

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
        let can_reopen = radio.read().can_reopen();

        let (background, tile_bg, tile_border, icon_c, title_c, sub_c) = {
            let c = &use_theme().read().colors;
            (
                c.surface_secondary,
                c.surface_tertiary,
                c.border,
                c.text_placeholder,
                c.text_primary,
                c.text_secondary,
            )
        };

        // The hero: a 60×60 rounded tile (elevated surface + hairline border) with a faint database
        // glyph.
        let tile = rect()
            .width(Size::px(60.))
            .height(Size::px(60.))
            .corner_radius(14.)
            .background(tile_bg)
            .border(Border::new().width(1.).fill(tile_border))
            .center()
            .margin(Gaps::new(0., 0., 16., 0.))
            .child(Icon::new(IconName::Database).color(icon_c).size(26.));

        // New query (primary) — and Reopen closed (secondary), only when something's on the stack.
        let new_btn = Button::new()
            .filled()
            .on_press(move |_| {
                radio.write().open_blank();
            })
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(6.)
                    .child(Icon::new(IconName::Plus).size(14.))
                    .child(Control::new("New query")),
            );

        let reopen_btn = can_reopen.then(|| {
            Button::new()
                .on_press(move |_| {
                    radio.write().reopen_last();
                })
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(6.)
                        .child(Icon::new(IconName::Reopen).size(14.))
                        .child(Control::new("Reopen closed")),
                )
        });

        let actions = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(12.)
            .child(new_btn)
            .maybe_child(reopen_btn);

        rect()
            .width(Size::fill())
            .height(Size::fill())
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .padding(Gaps::new(40., 40., 40., 40.))
            .background(background)
            .child(tile)
            .child(
                rect()
                    .margin(Gaps::new(0., 0., 8., 0.))
                    .child(Title::new("No query open").color(title_c)),
            )
            .child(
                rect()
                    .margin(Gaps::new(0., 0., 20., 0.))
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
