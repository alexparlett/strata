//! The **window form**'s row: an uppercase label, an optional hover hint, and the control
//! beneath them. The export window's options and the config modal's fields.
//!
//! **The hint is a tooltip, not a line of grey text.** That was a deliberate design pass (the
//! canvas swept every inline explainer into a hover tip), and it is the sort of thing that
//! silently reverts the first time someone adds a field by copying a nearby one — which is the
//! argument for this being a component rather than a convention. The Settings window is the one
//! surface it does *not* apply to; [`Setting`](super::Setting) is that row, and the module doc
//! has the why.
//!
//! It carries the label and the hint only. What the row *controls* is the caller's child, so a
//! row wraps a [`ValueField`](super::ValueField), a `Switch`, a `Select` or anything else
//! without this knowing which.

use freya::prelude::*;

use crate::components::form::{form_theme, LABEL_GAP};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Eyebrow, Prose};

const HINT_SIZE: f32 = 12.;

#[derive(PartialEq)]
pub struct FieldRow {
    label: String,
    hint: Option<String>,
    children: Vec<Element>,
}

impl FieldRow {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: None,
            children: Vec::new(),
        }
    }

    /// The explanation this row's ⓘ carries. Absent = no glyph, rather than an empty tooltip.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl ChildrenExt for FieldRow {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for FieldRow {
    fn render(&self) -> impl IntoElement {
        let theme = form_theme();

        let header = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(LABEL_GAP)
            .child(Eyebrow::new(self.label.clone()).color(theme.label_color))
            .maybe_child(self.hint.clone().map(|hint| {
                TooltipContainer::new(Tooltip::new(hint))
                    .position(AttachedPosition::Top)
                    .child(
                        Icon::new(IconName::Info)
                            .size(HINT_SIZE)
                            .color(theme.hint_color),
                    )
            }));

        let mut row = rect()
            .width(Size::fill())
            .vertical()
            .spacing(LABEL_GAP)
            .child(header);
        for child in &self.children {
            row = row.child(child.clone());
        }
        row
    }
}

/// A form's explanatory block — a statement where a control would go.
///
/// Not a disabled control and not a hint: it is what a row says when there is nothing to set
/// (the export window's Arrow format, which has no write options at all). An empty row would
/// read as "still loading".
#[derive(PartialEq)]
pub struct FieldNote {
    text: String,
}

impl FieldNote {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl Component for FieldNote {
    fn render(&self) -> impl IntoElement {
        let theme = form_theme();
        // The box comes off the form's own theme, not the base sheet. Reading `surface_primary`
        // there looked equivalent and is not: it is a *lower* tone than the window body, so the
        // note read as a hole punched in the surface while the panes beside it read as raised. A
        // component's dress is its own theme's (AGENTS.md §3), and the sheet is only for the
        // semantic ramp — which a note is not.
        rect()
            .width(Size::fill())
            .padding((12., 12.))
            .corner_radius(6.)
            .background(theme.note_background)
            .border(Border::new().width(1.).fill(theme.note_border_fill))
            .child(Prose::new(self.text.clone()).color(theme.note_color).wrap())
    }
}
