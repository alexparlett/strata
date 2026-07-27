//! **Forms** — the shared vocabulary every settings-style surface in the app is built from: the
//! export window's options, the config modal's fields, the Settings window's panes.
//!
//! One module, because these surfaces kept arriving one at a time and re-typing each other's
//! label metrics, field boxes and gaps. What a form is made of lives here; a window contributes
//! only *which* controls go in the rows and what they mean.
//!
//! ## What a form is
//!
//! A [`FormList`] of rows. A row carries a label (and its explanation) over or beside its
//! control; the control itself is the caller's child, so a row wraps a [`ValueField`], a
//! `Switch`, a `SegmentedToggle` or anything else without this knowing which.
//!
//! ## Two rows, deliberately
//!
//! - **[`FieldRow`]** — the *window form*: an uppercase eyebrow label whose explanation hangs
//!   off a hover ⓘ, in a [`FormList`] separated by gaps. The export window and the config modal.
//! - **[`Setting`]** — the *settings pane*: a sentence-case title with its explanation as inline
//!   subtext under it, in a [`FormList::divided`] separated by rules.
//!
//! They look like one row seen twice and are not. The design swept every inline explainer in the
//! app into a hover tip, and then its **"Settings consistency pass"** swept that window's four
//! back out again — "settling on subtext everywhere, since every non-toggle setting already used
//! it" — and made its panes uniform divider-separated rows. So a settings pane reaching for
//! `FieldRow` (or an export option reaching for `Setting`) is a regression of a decision, not a
//! tidy-up. **Everything else about them is shared**, which is the reason they sit in one module
//! rather than one per window: the theme below, the field boxes, the list, the metrics.
//!
//! ## Known divergences
//!
//! Named rather than silently averaged — each is one canvas differing from another, and each is
//! a one-line change here if the design settles it:
//!
//! - A window form spaces its rows [`ROW_GAP`] apart; a settings pane puts [`SETTING_GAP`]
//!   either side of a rule.
//! - A window form's label sits [`LABEL_GAP`] above its control; a settings row's label *block*
//!   sits [`CONTROL_GAP`] above its.
//! - A window form stacks a `Switch` under its label like any other control; a settings row puts
//!   it at the trailing edge, with the label block as a second press target
//!   ([`Setting::switch`]).

mod field;
mod list;
mod row;
mod setting;

use freya::prelude::*;

pub use field::{NumberField, ValueField, FIELD_HEIGHT};
pub use list::FormList;
pub use row::{FieldNote, FieldRow};
pub use setting::Setting;

// `%[no_ext]`: the form's dress is read by its four pieces (row · setting · list · field)
// rather than by one `Form` component, so there is no type for the generated
// `…ThemePartialExt` builder to hang off.
define_theme!(
    %[no_ext]
    %[component]
    pub Form {
        %[fields]
        /// A [`Setting`]'s sentence-case title.
        title_color: Color,
        /// A [`FieldRow`]'s uppercase eyebrow label.
        label_color: Color,
        /// A row's explanation, in both of its forms — the ⓘ that carries a window form's
        /// tooltip, and the inline subtext under a setting's title. One field because it is one
        /// role: what a row says about itself, pitched under the label.
        hint_color: Color,
        /// The rule between two rows of a [`FormList::divided`].
        divider_fill: Color,
        /// [`FieldNote`]'s box — the same raised inset every other boxed thing in a form sits
        /// on, so a note reads as a callout rather than a hole in the surface.
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

/// The gap between a [`FieldRow`]'s label and its control (canvas `var(--sp-3)`), and between
/// the label and its ⓘ.
pub(crate) const LABEL_GAP: f32 = 8.;
/// The gap under a [`Setting`]'s title, before its subtext (canvas `var(--sp-1)`).
pub(crate) const HINT_GAP: f32 = 2.;
/// The gap between a [`Setting`]'s label block and its control (canvas `var(--sp-4)`).
pub(crate) const CONTROL_GAP: f32 = 12.;
/// The gap between a [`NumberField`] and the unit beside it (canvas `var(--sp-3)`).
pub(crate) const UNIT_GAP: f32 = 8.;
/// The gap between two rows of a spaced [`FormList`].
pub(crate) const ROW_GAP: f32 = 20.;
/// The gap either side of a divided [`FormList`]'s rule (canvas `var(--sp-6)`).
pub(crate) const SETTING_GAP: f32 = 24.;
