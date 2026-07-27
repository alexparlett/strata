//! One row of a [`Form`](super::Form): a label, its explanation, and the control.
//!
//! The control is the caller's **child** — this knows nothing about it, so a row wraps a
//! [`ValueField`](super::ValueField), a `Switch`, a `SegmentedToggle`, a `Select` or a [`Note`]
//! without changing shape. How the label and its explanation are *set* comes from the form's
//! [`Variant`], read from context; see the module doc.

use freya::prelude::*;

use crate::components::form::{form_theme, Variant, CONTROL_GAP, HINT_GAP, LABEL_GAP};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Caption, Eyebrow, Prose, Strong};

/// The ⓘ that carries a fields row's explanation.
const HINT_SIZE: f32 = 12.;

#[derive(PartialEq)]
pub struct Row {
    label: String,
    hint: Option<String>,
    trailing: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    children: Vec<Element>,
}

impl Row {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: None,
            trailing: false,
            on_press: None,
            children: Vec::new(),
        }
    }

    /// This row's explanation — a hover tooltip in the fields register, inline subtext under
    /// the title in preferences. Absent = nothing, rather than an empty tooltip or a blank line.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Put the control at the row's **trailing edge** instead of under the label block.
    ///
    /// For a control small enough to read as the row's answer to its own label — a `Switch` in
    /// the preferences register. Explicit per row rather than derived from the variant because
    /// it is the one presentation the canvases disagree about *within* a register (see the
    /// module doc's known divergences).
    pub fn trailing(mut self) -> Self {
        self.trailing = true;
        self
    }

    /// Make the label block activate the control, so the whole row acts as one target.
    ///
    /// The row is a **sibling** of the control, never its ancestor: a built-in's `on_press`
    /// does not stop propagation, so a pressable ancestor would take the same click and act
    /// twice — for a `Switch`, back to where it started.
    pub fn on_press(mut self, on_press: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(on_press.into());
        self
    }
}

impl ChildrenExt for Row {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for Row {
    fn render(&self) -> impl IntoElement {
        let theme = form_theme();
        // Set once on the form (see the module doc); a bare row outside one is set in the
        // register the app's window forms use.
        let variant = use_try_consume::<Variant>().unwrap_or_default();

        // The label block. In the fields register the explanation hangs off a ⓘ beside the
        // label; in preferences it is a line of subtext under it, wrapped — those are full
        // sentences and the pane is narrow, so `Caption`'s default single-line cap would eat
        // the end of half of them.
        //
        // A preferences **title** wraps for the same reason: it is a sentence-case phrase and
        // some of them are whole clauses ("Confirm before closing a tab or window with a
        // running query"), which at the window's minimum width would otherwise be clipped
        // mid-word by the single-line default. A fields eyebrow stays capped — it is a short
        // uppercase label, and one that grew long would be the wrong label.
        let label = match variant {
            Variant::Fields => rect()
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
                })),
            Variant::Preferences => rect()
                .vertical()
                .child(
                    Strong::new(self.label.clone())
                        .color(theme.title_color)
                        .width(Size::fill())
                        .wrap(),
                )
                .map(self.hint.clone(), |el, hint| {
                    el.child(rect().height(Size::px(HINT_GAP))).child(
                        Caption::new(hint)
                            .color(theme.hint_color)
                            .width(Size::fill())
                            .wrap(),
                    )
                }),
        };
        let label = label.map(self.on_press.clone(), |el, on_press| {
            el.on_press(move |e: Event<PressEventData>| on_press.call(e))
        });

        let gap = match variant {
            Variant::Fields => LABEL_GAP,
            Variant::Preferences => CONTROL_GAP,
        };

        if self.trailing {
            // Label block and control side by side. `Content::Flex` is what makes the label's
            // `flex(1.)` divide the row rather than take its natural width — without it the
            // control is pushed off the surface.
            let mut row = rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .spacing(CONTROL_GAP)
                .child(label.width(Size::flex(1.)));
            for child in &self.children {
                row = row.child(child.clone());
            }
            row
        } else {
            let mut row = rect()
                .width(Size::fill())
                .vertical()
                .spacing(gap)
                .child(label.width(Size::fill()));
            for child in &self.children {
                row = row.child(child.clone());
            }
            row
        }
    }
}

/// A form's explanatory block — a statement where a control would go.
///
/// Not a disabled control and not a hint: it is what a row says when there is nothing to set
/// (the export window's Arrow format, which has no write options at all). An empty row would
/// read as "still loading".
#[derive(PartialEq)]
pub struct Note {
    text: String,
}

impl Note {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl Component for Note {
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
