//! The chat pane's header — **the chat switcher**, plus New chat and the pane's ×.
//!
//! The title is the trigger: pressing it lists every conversation in this window with its model
//! and message count, each deletable. That is the canvas's shape, and it is the right one for a
//! pane whose whole width is the transcript — a tab strip over 340px would be three ellipsized
//! stubs.
//!
//! **Deleting the last conversation opens a fresh one** rather than leaving a dead pane
//! ([`Chats::delete`](crate::apps::project::state::Chats)), which is also what lets the switcher
//! never render an empty list.

use freya::components::{Menu, MenuButton, Tooltip, TooltipContainer};
use freya::prelude::*;

use super::ChatTheme;
use crate::apps::project::state::{seed_pick, ChatId, ChatsCtx};
use crate::components::icon::{Icon, IconName};
use crate::components::tool_button::ToolButton;
use crate::components::typography::{Body, Meta};
use crate::state::{use_config, ConfigChan};

/// The header's height and inset (canvas: 40px, `0 var(--sp-3) 0 var(--sp-4)`).
const HEADER_H: f32 = 40.;
const HEADER_PAD: Gaps = Gaps::new(0., 6., 0., 10.);
/// The switcher card's width, and the room a row's title has inside it once the card's padding
/// and the delete button are taken out — a `Menu` hugs its children, so a long title would
/// otherwise stretch the card to the pane's whole width.
const MENU_WIDTH: f32 = 260.;
const MENU_ROW_CHROME: f32 = 56.;

pub struct ChatHeader {
    pub theme: ChatTheme,
    pub on_close: EventHandler<()>,
}

impl PartialEq for ChatHeader {
    fn eq(&self, other: &Self) -> bool {
        self.theme == other.theme
    }
}

impl Component for ChatHeader {
    fn render(&self) -> impl IntoElement {
        let mut chats = use_consume::<ChatsCtx>();
        let config = use_config(ConfigChan::Settings);
        let mut open = use_state(|| false);
        let theme = self.theme.clone();
        let on_close = self.on_close.clone();

        // A fresh conversation starts on Settings' defaults, resolved through the one funnel that
        // drops a provider that is no longer enabled.
        let fresh = move || seed_pick(&config.read().settings.ai);

        let (title, rows) = {
            let held = chats.read();
            let title = held.active().title.clone();
            let rows: Vec<(ChatId, String, String, bool)> = held
                .iter()
                .map(|chat| {
                    (
                        chat.id,
                        chat.title.clone(),
                        match chat.pick.model.is_empty() {
                            true => format!("{} messages", chat.message_count()),
                            false => {
                                format!("{} · {} messages", chat.pick.model, chat.message_count())
                            }
                        },
                        chat.id == held.active_id(),
                    )
                })
                .collect();
            (title, rows)
        };

        let switcher = rows.iter().fold(
            Menu::new()
                .min_width(Size::px(MENU_WIDTH))
                .on_close(move |()| open.set(false)),
            |menu, (id, name, meta, current)| {
                let id = *id;
                // **The delete is a sibling of the row, never inside it.** A built-in control's
                // press reaches its ancestors (AGENTS.md §3), so nesting it made every delete
                // also fire the row's own handler: the switcher closed on each one, and the row
                // press raced `delete` for a conversation that was about to stop existing.
                menu.child(
                    rect()
                        .width(Size::px(MENU_WIDTH - MENU_ROW_CHROME))
                        .horizontal()
                        .content(Content::Flex)
                        .cross_align(Alignment::Center)
                        .child(
                            MenuButton::new()
                                .on_press(move |_| {
                                    chats.write().show(id);
                                    open.set(false);
                                })
                                .child(
                                    rect()
                                        .width(Size::flex(1.))
                                        .vertical()
                                        .child(
                                            Body::new(name.clone())
                                                .color(match current {
                                                    true => theme.chip_color,
                                                    false => theme.title_color,
                                                })
                                                .width(Size::fill())
                                                .max_lines(1)
                                                .text_overflow(TextOverflow::Ellipsis),
                                        )
                                        .child(Meta::new(meta.clone()).color(theme.meta_color)),
                                ),
                        )
                        .child(ToolButton::new(IconName::Close, "Delete chat").on_press(
                            move |_| {
                                chats.write().delete(id, fresh());
                            },
                        )),
                )
            },
        );

        // The title *is* the trigger, so the pane's one heading and its one navigation control
        // are the same object — the canvas's shape, and the only one that fits a 340px pane.
        let trigger = Button::new()
            .flat()
            .height(Size::px(26.))
            .width(Size::fill())
            .on_press(move |_| open.toggle())
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(4.)
                    .child(
                        Body::new(title)
                            .color(self.theme.title_color)
                            .width(Size::flex(1.))
                            .max_lines(1)
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(12.)
                            .color(self.theme.meta_color),
                    ),
            );

        rect()
            .width(Size::fill())
            .height(Size::px(HEADER_H))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .padding(HEADER_PAD)
            .spacing(4.)
            .child(
                rect().width(Size::flex(1.)).child(
                    Attached::new(
                        TooltipContainer::new(Tooltip::new_text("Switch chat"))
                            .position(AttachedPosition::Bottom)
                            .child(trigger),
                    )
                    .bottom()
                    .align_start()
                    .offset(4.)
                    .maybe_child(open().then_some(switcher)),
                ),
            )
            .child(
                ToolButton::new(IconName::Plus, "New chat").on_press(move |_| {
                    chats.write().open(fresh());
                }),
            )
            .child(
                ToolButton::new(IconName::Close, "Close panel")
                    .on_press(move |_| on_close.call(())),
            )
    }
}
