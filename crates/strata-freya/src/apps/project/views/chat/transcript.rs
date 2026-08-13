//! The conversation itself: the open chat's turns, in order, each turn's blocks in the order
//! they arrived.
//!
//! **In arrival order, deliberately.** The model speaks, calls a tool, speaks again; a transcript
//! that hoisted every card to the bottom would separate its reasoning from its evidence, which is
//! the one thing a citation card exists to keep together.
//!
//! **The empty state is an invitation, not a feature list.** One sentence saying what the pane is
//! for — the canvas's — because a pane the user just opened has no history to explain.

use freya::clipboard::Clipboard;
use freya::prelude::*;
use freya::radio::use_radio;

use super::card::{OfferCard, StepCard, ACTIONS_PAD, CARD_PAD, CARD_RADIUS};
use super::ChatTheme;
use crate::apps::project::state::{Block, Chan, ChatsCtx, Reply, SessionState, Turn};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_XS, SP_1, SP_2, SP_3, SP_5, SP_6};
use crate::components::typography::{Eyebrow, Meta, Prose, Readout};

/// The gap between a turn's eyebrow and its body, and between blocks (canvas `--sp-3`).
const BLOCK_GAP: f32 = SP_3;
/// The height a turn's action row occupies whether or not it is showing — reserved, so revealing
/// it on hover does not shift the message above.
pub(super) const ACTIONS_H: f32 = 20.;
/// The empty state's own inset and the width its one sentence wraps in (canvas `max-width: 220px`).
const EMPTY_PAD: Gaps = Gaps::new_all(SP_6);
const EMPTY_W: f32 = 220.;

#[derive(PartialEq)]
pub struct Transcript {
    pub chats: ChatsCtx,
    pub theme: ChatTheme,
}

impl Component for Transcript {
    fn render(&self) -> impl IntoElement {
        // On `Chan::Tabs` because that is the channel a promote press writes, and this subtree's
        // cards hold the handle rather than consuming it once each.
        let session = use_radio::<SessionState, Chan>(Chan::Tabs);
        let chats = self.chats;
        let theme = self.theme.clone();

        let turns: Vec<Turn> = chats.read().active().turns.clone();
        if turns.is_empty() {
            return rect()
                .width(Size::fill())
                .center()
                .padding(EMPTY_PAD)
                .child(
                    Prose::new(
                        "Ask about your tables and views. Answers come back as SQL you can open \
                         in a tab.",
                    )
                    .color(theme.meta_color)
                    .width(Size::px(EMPTY_W))
                    .align(TextAlign::Center)
                    .wrap(),
                );
        }

        turns.into_iter().enumerate().fold(
            rect().width(Size::fill()).vertical().spacing(SP_5),
            |body, (at, turn)| {
                body.child(TurnRow {
                    turn,
                    session,
                    theme: theme.clone(),
                    key: (&at).into(),
                })
            },
        )
    }
}

/// One exchange — and **its own hover scope**.
///
/// Its own component rather than a fold in [`Transcript`] because the actions under a message
/// appear on hover: a `use_state` in the fold would be one flag for the whole transcript, so
/// hovering any message would reveal every message's row. One turn, one flag.
///
/// The actions are hidden rather than always shown: a time and a copy under every message in a
/// 340px pane is a column of furniture between the reader and the conversation. Their **slot is
/// reserved** either way, so revealing them never nudges the message above.
#[derive(PartialEq)]
struct TurnRow {
    turn: Turn,
    session: freya::radio::Radio<SessionState, Chan>,
    theme: ChatTheme,
    key: DiffKey,
}

impl KeyExt for TurnRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for TurnRow {
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let theme = self.theme.clone();
        let copyable = plain(&self.turn);
        let at = self.turn.at().to_string();

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(BLOCK_GAP)
            // **`over`/`out`, not `enter`/`leave`.** The enter pair is *exclusive*: it fires only
            // for the node actually under the pointer, so moving onto the copy button — a child
            // of this very row — read as leaving the turn, and the row hid itself under the
            // cursor. `PointerOver`/`PointerOut` are the non-exclusive pair (`is_emitted_once` is
            // false for them), so every node in the hit path is told, and a turn stays hovered
            // while the pointer is anywhere inside it, its own controls included.
            .on_pointer_over(move |_| hovered.set(true))
            .on_pointer_out(move |_| hovered.set(false))
            .child(match &self.turn {
                Turn::User { .. } => Eyebrow::new("YOU").color(theme.role_color).into_element(),
                Turn::Reply(_) => Eyebrow::new("STRATA")
                    .color(theme.chip_color)
                    .into_element(),
            })
            .child(match self.turn.clone() {
                Turn::User { text, chips, .. } => user(&text, &chips, &theme),
                Turn::Reply(reply) => self::reply(reply, self.session, &theme),
            })
            // **Always mounted, revealed by opacity** — never added and removed under the pointer.
            //
            // Building the children on `hovered` made them *appear where the cursor already was*:
            // approach the strip directly (rather than down from the message) and the button
            // materialises under the pointer, takes it, the turn reads as no longer hovered, the
            // button unmounts, the turn is hovered again — a loop, which is why it flickered and
            // could never be pressed. Nothing here mounts on hover now, so there is nothing to
            // oscillate.
            //
            // Invisible is not unreachable, and here that is exactly right: the only way to press
            // this is to have the pointer on it, which is the same condition that shows it.
            //
            // Copy first, then the time: the press is what the row is *for*, so it sits where the
            // eye lands and at the same offset under every message; the stamp is a fact about the
            // message and reads as its caption.
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(ACTIONS_H))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
                    .opacity(match hovered() {
                        true => 1.,
                        false => 0.,
                    })
                    .child(CopyMessage {
                        text: copyable,
                        theme: theme.clone(),
                    })
                    .child(Meta::new(at).color(theme.meta_color)),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// A fenced code block in the assistant's prose: the offer card's shell, with a copy press where
/// that one has Open in tab and Run.
///
/// It reads as the same kind of object because it *is* — a statement the assistant wrote — and
/// the difference that matters is the one the presses carry: this one is being explained, the
/// offer card is being handed over.
#[derive(PartialEq)]
struct CodeCard {
    code: String,
    theme: ChatTheme,
}

impl Component for CodeCard {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .vertical()
            .corner_radius(CARD_RADIUS)
            .background(self.theme.card_background)
            .border(Border::new().width(1.).fill(self.theme.card_border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    // A painted border is not laid out, so the inset carries it (AGENTS.md §3).
                    .padding(CARD_PAD)
                    .child(
                        Readout::new(self.code.clone())
                            .color(self.theme.sql_color)
                            .width(Size::fill())
                            .wrap(),
                    ),
            )
            .child(Divider::horizontal().color(self.theme.card_border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .padding(ACTIONS_PAD)
                    .child(CopyMessage {
                        text: self.code.clone(),
                        theme: self.theme.clone(),
                    }),
            )
    }
}

/// **A message as plain text** — what its copy button puts on the clipboard.
///
/// The *prose*, not the render: a step card's figures are the engine's own and belong to the
/// card, and an offer's SQL is already one press from a tab. What someone copies out of a
/// transcript is what was said.
fn plain(turn: &Turn) -> String {
    match turn {
        Turn::User { text, chips, .. } => match chips.is_empty() {
            true => text.clone(),
            false => format!("{}\n{text}", chips.join(" ")),
        },
        Turn::Reply(reply) => {
            let mut said: Vec<&str> = Vec::new();
            for block in &reply.blocks {
                match block {
                    Block::Prose(text) => said.push(text),
                    // An offer is a statement the user asked for, so it copies with the answer
                    // it came in.
                    Block::Offer { sql, .. } => said.push(sql),
                    Block::Step(_) => {}
                }
            }
            said.join("\n\n")
        }
    }
}

/// One message's copy press. Its own component so a turn's own re-render — a delta landing —
/// does not reset whichever button is mid-hover.
#[derive(PartialEq)]
struct CopyMessage {
    text: String,
    theme: ChatTheme,
}

impl Component for CopyMessage {
    fn render(&self) -> impl IntoElement {
        let text = self.text.clone();
        let color = self.theme.meta_color;

        // Nothing said yet, nothing to copy: a streaming turn's first frame has no prose.
        if text.trim().is_empty() {
            return rect().into_element();
        }

        // **The glyph alone, and no tooltip.** A `TooltipContainer` is an `Attached` overlay: it
        // renders outside this subtree, so the pointer crossing onto it leaves the turn, which
        // hides the row, which removes the overlay, which re-enters the turn — an oscillation,
        // and worst approaching from below where the row is the first thing met. A control that
        // only exists while its own message is hovered has little use for a hover label anyway;
        // the name it is still owed is an accessible one, which belongs on `Button` in the fork
        // (see `components::tool_button`).
        Button::new()
            .flat()
            .height(Size::px(ACTIONS_H))
            .on_press(move |_| {
                // The app's one clipboard stack, the same handle the grid's copy and the chart's
                // Copy Image use. Fire and forget: a failed write is a warning, not a dialog
                // about something the user will simply press again.
                if let Err(err) = Clipboard::set(text.clone()) {
                    tracing::warn!("chat copy failed: {err:?}");
                }
            })
            .child(Icon::new(IconName::Copy).size(12.).color(color))
            .into_element()
    }
}

/// The user's own message, with the chips it was sent with — what they were pointing at *when
/// they asked*, which is what a transcript is for.
fn user(text: &str, chips: &[String], theme: &ChatTheme) -> Element {
    let pinned = (!chips.is_empty()).then(|| {
        chips.iter().fold(
            rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::wrap_spacing(SP_2))
                .spacing(SP_2),
            |row, chip| {
                row.child(
                    rect()
                        .corner_radius(R_XS)
                        .background(theme.chip_background)
                        .padding(Gaps::new(SP_1, SP_3, SP_1, SP_3))
                        .child(Meta::new(chip.clone()).color(theme.chip_color)),
                )
            },
        )
    });

    rect()
        .width(Size::fill())
        .vertical()
        .spacing(SP_3)
        .maybe_child(pinned)
        .child(
            Prose::new(text.to_string())
                .color(theme.title_color)
                .width(Size::fill())
                .wrap(),
        )
        .into_element()
}

/// **Thinking** — what an open reply shows before it has said anything.
///
/// A live turn with no prose yet is otherwise a role eyebrow over nothing, which reads as the
/// send having gone nowhere. It is not a spinner and not a timer: the elapsed figures in this
/// pane are the engine's own (a step card's `elapsed_ms`), and a clock this pane started would be
/// a number nothing measured — so it says what is true and no more.
///
/// It goes as soon as the first delta lands or the first tool card opens, because by then the
/// turn is showing its own progress.
#[derive(PartialEq)]
struct Thinking {
    theme: ChatTheme,
}

impl Component for Thinking {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .child(CircularLoader::new().size(11.))
            .child(Meta::new("Thinking…").color(self.theme.meta_color))
    }
}

/// The assistant's half: its blocks, then the sentence for a turn that did not answer.
fn reply(
    reply: Reply,
    session: freya::radio::Radio<SessionState, Chan>,
    theme: &ChatTheme,
) -> Element {
    // Nothing said and nothing settled: the turn is out there, and saying so is the difference
    // between "working" and "the send went nowhere".
    if reply.blocks.is_empty() && !reply.settled {
        return Thinking {
            theme: theme.clone(),
        }
        .into_element();
    }

    let body = reply.blocks.into_iter().enumerate().fold(
        rect().width(Size::fill()).vertical().spacing(BLOCK_GAP),
        |body, (at, block)| {
            body.child(match block {
                // The fork's own viewer (AGENTS.md §3), keyed on its position so a streaming
                // delta re-parses one block rather than the whole turn.
                // **A fenced block is a card with a copy press** — the offer card's dress, minus
                // its Run: this is SQL the assistant is *explaining*, and the whole point of
                // `offer_sql` is that an executable statement arrives as its own card instead.
                // The renderer is the fork's `code_block` hook, so the markdown viewer still owns
                // the parse and this owns only the dress.
                Block::Prose(text) => MarkdownViewer::new(text)
                    .key(at)
                    .width(Size::fill())
                    .code_block({
                        let theme = theme.clone();
                        move |block: CodeBlock| {
                            Some(CodeCard {
                                code: block.code,
                                theme: theme.clone(),
                            })
                        }
                    })
                    .into_element(),
                // The cards are keyed through a wrapper rather than by growing a `DiffKey`
                // field: their position in the turn is the key, which is the wrapper's to know.
                Block::Step(step) => rect()
                    .key(at)
                    .width(Size::fill())
                    .child(StepCard {
                        step: *step,
                        session,
                        theme: theme.clone(),
                    })
                    .into_element(),
                // **A restored offer the catalog has moved out from under loses its press and
                // says nothing** (AS-07): it renders as the ordinary code block the assistant's
                // explanatory SQL already renders as. An error against a statement the user
                // never ran would be a complaint that the catalog changed, which is not a fault
                // and not news.
                Block::Offer { sql, stale } => rect()
                    .key(at)
                    .width(Size::fill())
                    .maybe_child((!stale).then(|| OfferCard {
                        sql: sql.clone(),
                        session,
                        theme: theme.clone(),
                    }))
                    .maybe_child(stale.then(|| CodeCard {
                        code: sql,
                        theme: theme.clone(),
                    }))
                    .into_element(),
            })
        },
    );

    // **The settle's own sentence.** A stop, a truncation and a provider fault each say what they
    // are in AS-02's words rather than this pane's, so one outcome cannot be described two ways.
    match reply.note {
        None => body.into_element(),
        Some(note) => body
            .child(
                Meta::new(note)
                    .color(theme.meta_color)
                    .width(Size::fill())
                    .wrap(),
            )
            .into_element(),
    }
}
