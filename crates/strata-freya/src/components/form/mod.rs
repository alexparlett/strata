//! **Forms** — the vocabulary every settings-style surface in the app is built from: the export
//! window's options, the config modal's fields, the Settings window's panes.
//!
//! ```text
//! Form                 the container: sets the register once, separates the rows
//!   Row                a label, its explanation, and where the control goes
//!     <control>        just a child — ValueField, NumberField, DirectoryField, Switch,
//!                      SegmentedToggle, …
//! ```
//!
//! **One [`Row`], not one per window.** A row's three presentation choices — how the label is
//! set, how its explanation is shown, how rows are separated — always move together, so they
//! are a [`Variant`] on the *form*, provided through context and read by every row under it.
//! A caller that had to restate the register on each row would eventually get one out of step,
//! which is the same reasoning `SegmentedToggle` puts its variant on the pill.
//!
//! - [`Form::new`] — the **fields** register: an uppercase eyebrow label, its explanation on a
//!   hover ⓘ, rows separated by a gap. The export window and the config modal.
//! - [`Form::preferences`] — the **preferences** register: a sentence-case title, its
//!   explanation as inline subtext, rows separated by rules. The Settings window's panes.
//!
//! A row can also be **addressed**: [`Row::anchor`] names it, and something outside the form —
//! the Settings window's search (P4-09) — asks for it by that name through [`Reveal`], which is
//! the row scrolling itself into view and flashing once. See [`reveal`].
//!
//! The registers exist because the design settled them separately: it swept every inline
//! explainer in the app into a hover tip, and then its **"Settings consistency pass"** swept
//! that window's four back out — "settling on subtext everywhere, since every non-toggle
//! setting already used it" — and made its panes uniform divider-separated rows. So the split
//! is a decision to preserve, not a fork to merge; but it is *one* axis on one component, not
//! two components.
//!
//! ## Known divergences
//!
//! Where the canvases disagree and the difference is real, it is named here and reachable from
//! a call site — never averaged into a middle value, and never hidden inside a type:
//!
//! - **Where a `Switch` sits.** A preferences row puts it at the row's trailing edge with the
//!   label block as a second press target; a fields row stacks it under the label like any
//!   other control. That is [`Row::trailing`] — an explicit per-row choice, because it is the
//!   one presentation the two canvases genuinely disagree about *within* a register (nothing
//!   says a fields row may never want it).
//! - **The gap between a label and its control** — [`LABEL_GAP`] in the fields register,
//!   [`CONTROL_GAP`] in preferences, because a title-plus-subtext block needs more air under
//!   it than a single eyebrow line.
//! - **The gap between rows** — [`ROW_GAP`] against [`RULE_GAP`] either side of a rule.
//!
//! Each is one constant here when the design settles it.

mod field;
mod options;
mod reveal;
mod row;

use freya::prelude::*;

use crate::components::divider::Divider;

pub use field::{DirectoryField, NumberField, ValueField, FIELD_HEIGHT};
pub use options::{Choice, Control, Group, Make, OptionList, TextField};
pub use reveal::{Reveal, RevealScroll};
pub use row::{Note, Row};

// `%[no_ext]`: the form's dress is read by its pieces (the form, its rows, its fields) rather
// than by one component, so there is no type for the generated `…ThemePartialExt` builder to
// hang off.
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

/// Resolve a **single-character field**: the two escapes the canvases document (`\t`, `\n`), a
/// literal backslash, or one plain character. Empty is `None` (such a field is optional);
/// anything longer is an error the surface shows rather than a silent truncation.
///
/// Shared, because a delimiter, a quote and a comment marker are the same field wherever they
/// appear: the export window and the Configure window both offer them, and a `\t` that resolved
/// in one and not the other would be the same control meaning two things.
pub fn one_char(what: &str, raw: &str) -> Result<Option<char>, String> {
    let resolved = match raw {
        "" => return Ok(None),
        "\\t" => '\t',
        "\\n" => '\n',
        "\\\\" => '\\',
        other => {
            let mut chars = other.chars();
            let first = chars.next().expect("non-empty");
            if chars.next().is_some() {
                return Err(format!(
                    "The CSV {what} has to be a single character (or \\t for tab), not {other:?}"
                ));
            }
            first
        }
    };
    Ok(Some(resolved))
}

/// Read the form dress. Every piece in this module resolves its colours through here, so a
/// form's look is one theme rather than one per window (AGENTS.md §3).
pub fn form_theme() -> FormTheme {
    get_theme!(&None::<FormThemePartial>, FormThemePreference, "form")
}

/// The gap between a fields row's label and its control (canvas `var(--sp-3)`), and between
/// that label and its ⓘ.
pub(crate) const LABEL_GAP: f32 = 8.;
/// The gap under a preferences row's title, before its subtext (canvas `var(--sp-1)`).
pub(crate) const HINT_GAP: f32 = 2.;
/// The gap between a preferences row's label block and its control (canvas `var(--sp-4)`), and
/// between a trailing control and the label block beside it.
pub(crate) const CONTROL_GAP: f32 = 12.;
/// The gap between a value box and whatever is set beside it (canvas `var(--sp-3)`) — a
/// [`NumberField`]'s unit label, a [`DirectoryField`]'s browse button. One constant, because
/// it is one role: what separates a box from the thing that qualifies it.
pub(crate) const FIELD_GAP: f32 = 8.;
/// The gap between two rows of a fields form.
pub(crate) const ROW_GAP: f32 = 20.;
/// The gap either side of a preferences form's rule (canvas `var(--sp-6)`).
pub(crate) const RULE_GAP: f32 = 24.;

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

        // Scoped to this form's subtree, so every row under it is set in the same register
        // without the caller repeating itself.
        let variant = self.variant;
        use_provide_context(move || variant);

        // Spelled out rather than set as `spacing`, because the preferences separator is three
        // children (gap, rule, gap) and the two registers should read as one loop.
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
                // The scope is the format, exactly as both real windows pass it — which is the
                // thing under test: without it the shared labels pair across the switch.
                let scope = if csv { "CSV" } else { "JSON" };
                OptionList::new(scope, groups(csv), move |_: Edit| {})
            },
            (600., 800.).into(),
            |r| r.provide_root_context(|| State::create(true)),
            1.,
        );

        // CSV, then JSON, then CSV — the switch the crash report named, both ways.
        for next in [true, false, true] {
            let mut csv = count;
            csv.set(next);
            runner.render();
            runner.render();
        }
    }
}
