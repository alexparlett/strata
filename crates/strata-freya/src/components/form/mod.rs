//! **Forms** — the vocabulary every settings-style surface in the app is built from: the export
//! window's options, the config modal's fields, the Settings window's panes.
//!
//! ```text
//! Form                 the container: sets the register once, separates the rows
//!   Row                a label, its explanation, and where the control goes
//!     <control>        just a child — ValueField, NumberField, Switch, SegmentedToggle, …
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
mod row;

use freya::prelude::*;

use crate::components::divider::Divider;

pub use field::{NumberField, ValueField, FIELD_HEIGHT};
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
    }
);

/// Read the form dress. Every piece in this module resolves its colours through here, so a
/// form's look is one theme rather than one per window (AGENTS.md §3).
pub(crate) fn form_theme() -> FormTheme {
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
/// The gap between a [`NumberField`] and the unit beside it (canvas `var(--sp-3)`).
pub(crate) const UNIT_GAP: f32 = 8.;
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
