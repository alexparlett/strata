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
use freya::radio::use_radio_station;

use super::export::export_chat;
use super::ChatTheme;
use crate::apps::project::state::{
    open_stored, seed_pick, store_shed, use_report, AssistantCtx, ChatsCtx, LogCtx, ProjChan,
    ProjectState, RowKey,
};
use crate::apps::project::views::ChatDrop;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{COMPACT_BUTTON, RIGHT_PANE_HEADER_HEIGHT, SP_2, SP_3, SP_4};
use crate::components::tool_button::ToolButton;
use crate::components::typography::Control;
use crate::components::typography::{Body, Meta};
use crate::state::{use_config, ConfigChan};

/// The header's inset (canvas `0 var(--sp-3) 0 var(--sp-4)`); its height is the right side's own
/// [`RIGHT_PANE_HEADER_HEIGHT`], shared with the inspector it alternates with.
const HEADER_PAD: Gaps = Gaps::new(0., SP_3, 0., SP_4);
/// The switcher card's width, and the room a row's title has inside it once the card's padding
/// and the delete button are taken out — a `Menu` hugs its children, so a long title would
/// otherwise stretch the card to the pane's whole width.
const MENU_WIDTH: f32 = 260.;
const MENU_ROW_CHROME: f32 = 56.;
/// The actions menu's width — narrower than the switcher's, because its rows are two short
/// labels rather than a title over its model and message count.
const ACTIONS_WIDTH: f32 = 200.;

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
        let assistant = use_consume::<AssistantCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let root = project.read().root.clone();
        let report = use_report();
        let mut confirming = use_consume::<State<Option<ChatDrop>>>();
        let config = use_config(ConfigChan::Settings);
        let mut open = use_state(|| false);
        let mut menu = use_state(|| false);
        let mut exporting = use_state(|| false);
        let log = use_consume::<LogCtx>();
        let theme = self.theme.clone();
        let on_close = self.on_close.clone();

        let fresh = move || seed_pick(&config.read().settings.ai);

        let (title, rows) = {
            let held = chats.read();
            (held.active().title.clone(), held.rows())
        };

        let switcher = rows.iter().fold(
            Menu::new()
                .min_width(Size::px(MENU_WIDTH))
                .on_close(move |()| open.set(false)),
            |menu, row| {
                let key = row.key;
                let meta = match row.model.is_empty() {
                    true => format!("{} messages", row.messages),
                    false => format!("{} · {} messages", row.model, row.messages),
                };
                menu.child(
                    rect()
                        .width(Size::px(MENU_WIDTH - MENU_ROW_CHROME))
                        .horizontal()
                        .content(Content::Flex)
                        .cross_align(Alignment::Center)
                        .child(
                            MenuButton::new()
                                .on_press({
                                    let assistant = assistant.clone();
                                    let root = root.clone();
                                    move |_| {
                                        match key {
                                            RowKey::Live(id) => chats.write().show(id),
                                            RowKey::Shelved(id) => {
                                                open_stored(
                                                    &assistant,
                                                    chats,
                                                    root.clone(),
                                                    report,
                                                    id,
                                                );
                                            }
                                        }
                                        open.set(false);
                                    }
                                })
                                .child(
                                    rect()
                                        .width(Size::flex(1.))
                                        .vertical()
                                        .child(
                                            Body::new(row.title.clone())
                                                .color(match row.current {
                                                    true => theme.chip_color,
                                                    false => theme.title_color,
                                                })
                                                .width(Size::fill())
                                                .max_lines(1)
                                                .text_overflow(TextOverflow::Ellipsis),
                                        )
                                        .child(Meta::new(meta).color(theme.meta_color)),
                                ),
                        )
                        .child(ToolButton::new(IconName::Close, "Delete chat").on_press({
                            let title = row.title.clone();
                            move |_| {
                                confirming.set(Some(ChatDrop::One {
                                    key,
                                    title: title.clone(),
                                }));
                                open.set(false);
                            }
                        })),
                )
            },
        );

        let actions = Menu::new()
            .min_width(Size::px(ACTIONS_WIDTH))
            .on_close(move |()| menu.set(false))
            .child(
                MenuButton::new()
                    .on_press(move |_| {
                        exporting.set(true);
                        menu.set(false);
                    })
                    .child(Control::new("Export chat\u{2026}")),
            )
            .child(
                MenuButton::new()
                    .on_press(move |_| {
                        confirming.set(Some(ChatDrop::All));
                        menu.set(false);
                    })
                    .child(Control::new("Clear conversations\u{2026}")),
            );

        use_side_effect(move || {
            if !*exporting.read() {
                return;
            }
            exporting.set(false);
            export_chat(chats.peek().active(), log);
        });

        let trigger = Button::new()
            .flat()
            .height(Size::px(COMPACT_BUTTON))
            .width(Size::fill())
            .on_press(move |_| open.toggle())
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(SP_2)
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
            .height(Size::px(RIGHT_PANE_HEADER_HEIGHT))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .padding(HEADER_PAD)
            .spacing(SP_2)
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
                    store_shed(&root, chats, report);
                }),
            )
            .child(
                Attached::new(
                    ToolButton::new(IconName::Dots, "Chat actions")
                        .on_press(move |_| menu.toggle()),
                )
                .bottom()
                .align_end()
                .offset(4.)
                .maybe_child(menu().then_some(actions)),
            )
            .child(
                ToolButton::new(IconName::Close, "Close panel")
                    .on_press(move |_| on_close.call(())),
            )
    }
}
