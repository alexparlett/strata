use freya::components::use_theme;
use freya::prelude::*;

use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Readout, Title};

/// The results pane after a query settles `Err`: the empty-state layout in error dress —
/// a rounded icon tile over a title, then the engine's message in mono. The message is
/// the query's own error (freya-query `Settled(Err)`); a new Run clears it by supersession.
/// The richer error surface (type banner · code frame · caret · hint) is the Problems /
/// error-view port, a later slice.
#[derive(PartialEq)]
pub struct ErrorState {
    message: String,
}

impl ErrorState {
    pub fn new(message: String) -> Self {
        Self { message }
    }
}

impl Component for ErrorState {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let (tile_bg, tile_border, icon_color, title_color, msg_color, background) = {
            let c = &theme.read().colors;
            (
                c.surface_tertiary,
                c.border,
                c.error,
                c.text_secondary,
                c.text_placeholder,
                c.surface_secondary,
            )
        };

        rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .spacing(12.)
            .padding((0., 24.))
            .background(background)
            .child(
                rect()
                    .width(Size::px(46.))
                    .height(Size::px(46.))
                    .corner_radius(8.)
                    .background(tile_bg)
                    .border(Border::new().width(1.).fill(tile_border))
                    .center()
                    .child(Icon::new(IconName::Alert).color(icon_color).size(22.)),
            )
            .child(Title::new("Query failed").color(title_color))
            .child(
                Readout::new(self.message.clone())
                    .color(msg_color)
                    .max_width(Size::px(560.))
                    .wrap(),
            )
    }
}
