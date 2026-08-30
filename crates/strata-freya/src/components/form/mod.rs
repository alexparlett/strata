//! **Forms** — the vocabulary every settings-style surface in the app is built from: the export
//! window's options, the config modal's fields, the Settings window's panes.
//!
//! ```text
//! Form                 the container: sets the register once, separates the rows
//!   Section            optional: a heading over the rows that follow it
//!   Row                a label, its explanation, and where the control goes
//!     <control>        just a child — ValueField, NumberField, PathField, Switch,
//!                      SegmentedToggle, …
//! ```
//!
//! **One [`Row`], not one per window.** A row's three presentation choices — how the label is set,
//! how its explanation is shown, how rows are separated — always move together, so they are a
//! [`Variant`] on the *form*, read from context by every row under it. A caller restating the
//! register per row would eventually get one out of step.
//!
//! - [`Form::new`] — the **fields** register: an uppercase eyebrow label, its explanation on a
//!   hover ⓘ, rows separated by a gap. The export window and the config modal.
//! - [`Form::preferences`] — the **preferences** register: a sentence-case title, its explanation
//!   as inline subtext, rows separated by rules. The Settings window's panes.
//!
//! A row can also be **addressed**: [`Row::anchor`] names it, and the Settings search asks for it
//! through [`Reveal`], which scrolls the row into view and flashes it once.
//!
//! The two registers are a decision the design settled separately, not a fork to merge — but they
//! are one axis on one component, not two components.
//!
//! **Known divergences**, named here and reachable from a call site rather than averaged into a
//! middle value:
//!
//! - **Where a `Switch` sits** — [`Row::trailing`], an explicit per-row choice, because it is the
//!   one presentation the canvases disagree about *within* a register.
//! - **The gap between a label and its control** — [`LABEL_GAP`] against [`CONTROL_GAP`], because a
//!   title-plus-subtext block needs more air under it than a single eyebrow line.
//! - **The gap between rows** — [`ROW_GAP`] against [`RULE_GAP`] either side of a rule.

mod field;
mod options;
mod reveal;
mod row;

use freya::prelude::*;

use crate::components::divider::Divider;
use crate::components::metrics::{SP_1, SP_3, SP_4, SP_5, SP_6};

pub use field::{NumberField, PathField, ValueField, FIELD_HEIGHT};
pub use options::{Choice, Control, Group, Make, OptionList, TextField};
pub use reveal::{Reveal, RevealScroll};
pub use row::{Note, Row, Section};

define_theme!(
    %[no_ext]
    %[component]
    pub Form {
        %[fields]
        /// A preferences row's sentence-case title.
        title_color: Color,
        /// A fields row's uppercase eyebrow label.
        label_color: Color,
        /// A row's explanation, in both registers — the ⓘ that carries a fields row's tooltip,
        /// and the inline subtext under a preferences row's title. One field because it is one
        /// role: what a row says about itself, pitched under the label.
        hint_color: Color,
        /// The `REQUIRED` marker beside a label ([`Row::required`]). Its own field rather than
        /// the hint's: a hint is an explanation the reader may skip, and this is a constraint on
        /// what they can do next, so the two do not have to move together.
        required_color: Color,
        /// The rule between two rows of a [`Form::preferences`].
        divider_fill: Color,
        /// [`Note`]'s box — the same raised inset every other boxed thing in a form sits on, so
        /// a note reads as a callout rather than a hole in the surface.
        note_background: Color,
        note_border_fill: Color,
        /// A note's prose. **Not** `label_color`: that is the eyebrow tone, pitched to recede
        /// under a control, and a sentence set in it on the note's raised box has too little
        /// contrast to read. The canvas gives a note `--c-muted`, a step brighter.
        note_color: Color,
        /// The wash a row flashes when something [reveals](Reveal) it — the accent tint the flash
        /// starts at, fading to nothing. Named for the role and not for the Settings search that
        /// asks for it first: any form that can be jumped into wants the same mark.
        reveal_background: Color,
    }
);

/// Read the form dress. Every piece in this module resolves its colours through here, so a
/// form's look is one theme rather than one per window.
pub fn form_theme() -> FormTheme {
    get_theme!(&None::<FormThemePartial>, FormThemePreference, "form")
}

/// The gap between a fields row's label and its control (canvas `var(--sp-3)`), and between
/// that label and its ⓘ.
pub(crate) const LABEL_GAP: f32 = SP_3;
/// The gap under a preferences row's title, before its subtext (canvas `var(--sp-1)`).
pub(crate) const HINT_GAP: f32 = SP_1;
/// The gap between a preferences row's label block and its control (canvas `var(--sp-4)`), and
/// between a trailing control and the label block beside it.
pub(crate) const CONTROL_GAP: f32 = SP_4;
/// The gap between a value box and whatever is set beside it (canvas `var(--sp-3)`) — a
/// [`NumberField`]'s unit label, a [`PathField`]'s browse button. One constant, because
/// it is one role: what separates a box from the thing that qualifies it.
pub(crate) const FIELD_GAP: f32 = SP_3;
/// The gap between two rows of a fields form.
pub(crate) const ROW_GAP: f32 = SP_5;
/// The gap either side of a preferences form's rule (canvas `var(--sp-6)`).
pub(crate) const RULE_GAP: f32 = SP_6;

/// Which register a form is set in — see the module doc. Provided by [`Form`] and read by every
/// [`Row`] under it, so it is chosen once per surface.
#[derive(PartialEq, Clone, Copy, Default, Debug)]
pub enum Variant {
    /// Eyebrow label, explanation on a hover ⓘ, rows separated by a gap.
    #[default]
    Fields,
    /// Sentence-case title, explanation as inline subtext, rows separated by rules.
    Preferences,
}

/// A form: its rows in order, and the only place the rhythm between them is spelled out.
#[derive(PartialEq)]
pub struct Form {
    children: Vec<Element>,
    variant: Variant,
}

impl Default for Form {
    fn default() -> Self {
        Self::new()
    }
}

impl Form {
    /// A form in the **fields** register (see [`Variant::Fields`]).
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            variant: Variant::default(),
        }
    }

    /// Set this form in the **preferences** register (see [`Variant::Preferences`]). Applies to
    /// its rows too — they read it from context, so it is set here and nowhere else.
    pub fn preferences(mut self) -> Self {
        self.variant = Variant::Preferences;
        self
    }
}

impl ChildrenExt for Form {
    fn get_children(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }
}

impl Component for Form {
    fn render(&self) -> impl IntoElement {
        let theme = form_theme();

        let variant = self.variant;
        use_provide_context(move || variant);

        let mut form = rect().width(Size::fill()).vertical();
        for (i, row) in self.children.iter().enumerate() {
            if i > 0 {
                form = match self.variant {
                    Variant::Fields => form.child(rect().height(Size::px(ROW_GAP))),
                    Variant::Preferences => form
                        .child(rect().height(Size::px(RULE_GAP)))
                        .child(Divider::horizontal().color(theme.divider_fill))
                        .child(rect().height(Size::px(RULE_GAP))),
                };
            }
            form = form.child(row.clone());
        }
        form
    }
}

#[cfg(test)]
mod tests {
    use freya_testing::TestingRunner;

    use super::*;
    use crate::components::form::{Choice, Control, Group, Make, OptionList, TextField};
    use crate::theme::strata_theme;

    /// A form's row count **changes** — an option list is rebuilt when the format switches, and
    /// CSV's nine groups become JSON's three and back again. This is that, and it used to
    /// panic in Freya's differ (`runner.rs`, `Option::unwrap()` on a missing `scope_id`):
    /// the separators between rows were unkeyed raw elements interleaved with keyed row
    /// *components*, so once the list shortened a position that had held a component was diffed
    /// against a spacer.
    #[test]
    fn a_form_survives_its_row_count_changing() {
        #[derive(Clone, PartialEq, Debug)]
        struct Edit(usize);

        /// The Configure window's real label/control sets. The shape that matters is that the
        /// two lists **share keys in different positions**: `SCHEMA-INFER ROWS` is 7th in one
        /// and 1st in the other, `COMPRESSION` 8th and 2nd, and `SHAPE` exists only in the
        /// shorter one. A prefix-subset does not reproduce this.
        fn groups(csv: bool) -> Vec<Group<Edit>> {
            let select = || Control::Select {
                options: vec![Choice {
                    label: "a".into(),
                    edit: Edit(0),
                    selected: true,
                }],
            };
            let text = || {
                Control::Text(TextField {
                    value: ",".into(),
                    placeholder: ",",
                    max_len: 8,
                    make: Make(|_: String| Edit(0)),
                })
            };
            let toggle = || Control::Toggle {
                on: true,
                edit: Edit(0),
                hint: None,
            };
            let num = || Control::Num {
                value: 1000,
                min: 0,
                max: 10_000,
                make: Make(|v: u32| Edit(v as usize)),
            };
            let spec: Vec<(&str, Control<Edit>)> = if csv {
                vec![
                    ("HEADER ROW", toggle()),
                    ("DELIMITER", text()),
                    ("QUOTE CHARACTER", text()),
                    ("ESCAPE CHARACTER", text()),
                    ("COMMENT CHARACTER", text()),
                    ("NEWLINES IN VALUES", toggle()),
                    ("RAGGED ROWS", toggle()),
                    ("SCHEMA-INFER ROWS", num()),
                    ("COMPRESSION", select()),
                ]
            } else {
                vec![
                    ("SHAPE", select()),
                    ("SCHEMA-INFER ROWS", num()),
                    ("COMPRESSION", select()),
                ]
            };
            spec.into_iter()
                .map(|(label, control)| Group {
                    label: label.into(),
                    hint: None,
                    control,
                })
                .collect()
        }

        let (mut runner, count) = TestingRunner::new(
            move || {
                use_init_theme(|| strata_theme(&strata_core::theme::load("midnight")));
                let csv = consume_context::<State<bool>>();
                let csv = *csv.read();
                let scope = if csv { "CSV" } else { "JSON" };
                OptionList::new(scope, groups(csv), move |_: Edit| {})
            },
            (600., 800.).into(),
            |r| r.provide_root_context(|| State::create(true)),
            1.,
        );

        for next in [true, false, true] {
            let mut csv = count;
            csv.set(next);
            runner.render();
            runner.render();
        }
    }
}
