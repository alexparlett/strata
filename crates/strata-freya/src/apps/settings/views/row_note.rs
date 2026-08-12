//! A **row note**: a full-width statement between two table rows, about the row above it.
//!
//! Both of the window's grids need one, and they needed it for the same reason. The Engine
//! pane (P4-07) says why a property cannot be applied; the Keymap pane (P4-08) says a chord is
//! already taken and offers to take it anyway. It is a sibling *between* rows rather than a cell
//! inside one because the thing it describes belongs to the whole row — a property's fault is
//! not its name's or its value's, and a chord clash is not the label's — and because a cell
//! stands at a fixed height so the columns line up, which a note that has to grow cannot.
//!
//! **One tone, not two.** The wash, the edge, the glyph and the message are all the same colour,
//! which is what makes it one statement: the Engine pane's is `error`, the Keymap pane's
//! `warning`. Named divergence from the canvas, which paints the keymap's conflict box with a
//! red edge over a red wash and then sets its text warm — a box that is red at the edge and
//! amber in the middle says both "this is broken" and "answer this", and only the second is
//! true of a clash.

use freya::prelude::*;

use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_3, SP_5};
use crate::components::typography::Caption;

/// The note's inset. Its sides line up with the header strip's and a cell's (`CELL_INSET`, the
/// canvas's `var(--sp-4)`) so the message starts on the same column as the row above it; its 6px
/// top and bottom are tighter than the canvas's `var(--sp-3)`, which is the value the Engine
/// pane's error strip shipped with and the one the two grids now share.
const NOTE_PADDING: Gaps = Gaps::new(SP_3, SP_5, SP_3, SP_5);
/// The gap between the glyph, the message and whatever is offered after it.
const NOTE_GAP: f32 = SP_3;
/// Alpha of the wash behind a note, derived from its tone.
const WASH_ALPHA: u8 = 20;

#[derive(PartialEq)]
pub struct RowNote {
    message: String,
    tone: Color,
    /// What the reader can do about it, if anything — the Keymap pane's Reassign / Cancel pair.
    /// A note without them is a statement; a note with them is a question.
    actions: Option<Element>,
}

impl RowNote {
    /// A note in `tone` — one of the sheet's semantic slots, since that is what the wash, the
    /// edge and the message are all derived from.
    pub fn new(message: impl Into<String>, tone: Color) -> Self {
        Self {
            message: message.into(),
            tone,
            actions: None,
        }
    }

    /// Put the answers to the note on its own line, after the message.
    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_element());
        self
    }
}

impl Component for RowNote {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(NOTE_GAP)
            .padding(NOTE_PADDING)
            .background(self.tone.with_a(WASH_ALPHA))
            .child(Icon::new(IconName::Alert).size(12.).color(self.tone))
            .child(
                Caption::new(self.message.clone())
                    .color(self.tone)
                    .width(Size::flex(1.))
                    .wrap(),
            )
            .maybe_child(self.actions.clone())
    }
}
