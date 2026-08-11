use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station};
use strata_model::TabId;

use crate::apps::project::state::{Anchor, Chan, ChatsCtx, SessionState};
use crate::apps::project::views::ask_about;
use crate::components::icon::{Icon, IconName};
use crate::components::tones::tones;
use crate::components::typography::{Control, Readout, Title};
use crate::theme::{use_roles, Role};

/// The results pane after a query settles `Err`: the empty-state layout in error dress —
/// a rounded icon tile over a title, then the engine's message in mono. The message is
/// the query's own error (freya-query `Settled(Err)`); a new Run clears it by supersession.
/// The richer error surface (type banner · code frame · caret · hint) is the Problems /
/// error-view port, a later slice.
///
/// **The failure is where the assistant is offered** (AS-04). Error-anchored help is the most
/// convergent gesture in the field — DataGrip, `DBeaver`, Databricks, Snowflake and Hex all hang it
/// on the failure site — and it is one press into the chat pane with `@tab` already pinned: the
/// tab's SQL, and *this* message. The error has to be carried rather than looked up, because it
/// lives in this run's own query entry and no store holds it.
#[derive(PartialEq)]
pub struct ErrorState {
    message: String,
    /// Whose run failed — what the pinned anchor names, and what the assistant reads the SQL of.
    tab: TabId,
}

impl ErrorState {
    pub fn new(message: String, tab: TabId) -> Self {
        Self { message, tab }
    }
}

impl Component for ErrorState {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        // Subscribed for the tab's *name*, and a station for the write — one reads, one writes.
        let session = use_radio::<SessionState, Chan>(Chan::Tabs);
        let station = use_radio_station::<SessionState, Chan>();
        let chats = use_consume::<ChatsCtx>();
        let tab = self.tab;
        let message = self.message.clone();
        let name = session
            .read()
            .tabs
            .get(&tab)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "query".to_string());
        let (tile_bg, tile_border, icon_color, title_color, msg_color, background) = (
            roles.get(Role::ElementBackground),
            roles.get(Role::Border),
            tones().error,
            roles.get(Role::TextMuted),
            roles.get(Role::TextPlaceholder),
            roles.get(Role::SurfaceRaised),
        );

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
            // One press into the chat pane, with the query and this message already attached.
            .child(
                Button::new()
                    .outline()
                    .on_press(move |_| {
                        ask_about(
                            station,
                            chats,
                            Anchor::Tab {
                                id: tab,
                                name: name.clone(),
                                error: Some(message.clone()),
                            },
                        );
                    })
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(6.)
                            .child(Icon::new(IconName::Chat).size(13.))
                            .child(Control::new("Ask the assistant")),
                    ),
            )
    }
}
