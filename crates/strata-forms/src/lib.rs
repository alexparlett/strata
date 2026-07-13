//! # strata-forms — headless form validation-state for Dioxus
//!
//! A small, renderer-agnostic form layer (react-hook-form in spirit) where **the form
//! owns the data**. You give [`use_form`] an initial value of a type that implements
//! [`Form`] (usually via `#[derive(Form)]`); the returned [`FormState`] holds the draft,
//! validates a field **the moment it changes** (only that field — never a whole-form
//! sweep), tracks errors + touched, and gates submit on [`FormState::is_valid`].
//!
//! Controls bind through a [`FieldHandle`] ([`FormState::field`]): it carries the
//! current value, the field's error, and a commit callback — apply it to *any* input.
//! Values cross the boundary as `String` (see [`FormValue`]) so fields are addressable
//! by id and work inside list loops; typed fields round-trip through parse/format.
//!
//! This crate is headless: it renders nothing and knows nothing about your controls.
//! The styled field components live in the app that consumes it.

use std::collections::{BTreeMap, BTreeSet};

use dioxus::prelude::*;

pub use strata_forms_macro::Form;

/// Field `id` → error message. Empty ⇒ the form is valid. Ids are free-form; a list
/// field keys its rows (`"sources[2]"`).
pub type FormErrors = BTreeMap<String, String>;

/// String ⇄ typed-field conversion at the form boundary. `#[derive(Form)]` uses this to
/// read/write each field by id; controls exchange strings, the struct stays typed.
pub trait FormValue: Sized {
    fn to_field(&self) -> String;
    fn from_field(raw: &str) -> Result<Self, String>;
}

impl FormValue for String {
    fn to_field(&self) -> String {
        self.clone()
    }
    fn from_field(raw: &str) -> Result<Self, String> {
        Ok(raw.to_string())
    }
}

macro_rules! form_value_parse {
    ($($t:ty),* $(,)?) => {$(
        impl FormValue for $t {
            fn to_field(&self) -> String { self.to_string() }
            fn from_field(raw: &str) -> Result<Self, String> {
                raw.trim().parse::<$t>().map_err(|e| e.to_string())
            }
        }
    )*};
}
form_value_parse!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64, bool);

/// A form model: the typed draft. Implement via `#[derive(Form)]` for a struct, or by
/// hand for a catalog-driven form (see the engine settings). Every method is keyed by a
/// string field `id`.
pub trait Form: Clone + PartialEq + 'static {
    /// The current value of `id` as a string, or `None` for an unknown id.
    fn get_field(&self, id: &str) -> Option<String>;
    /// Set `id` from a raw string (parsed to the field's type; a parse failure is a
    /// no-op — controls only emit values their type accepts).
    fn set_field(&mut self, id: &str, raw: &str);
    /// Validate a single field / list row against the current draft; `None` = ok.
    fn validate_field(&self, id: &str) -> Option<String>;
    /// Every field id, in order (list fields expand to indexed ids).
    fn field_ids(&self) -> Vec<String>;
    /// All current errors — the default runs [`Form::validate_field`] over
    /// [`Form::field_ids`]. Used to seed on open + at submit.
    fn errors(&self) -> FormErrors {
        let mut out = FormErrors::new();
        for id in self.field_ids() {
            if let Some(msg) = self.validate_field(&id) {
                out.insert(id, msg);
            }
        }
        out
    }
}

/// The reactive form handle: owns the draft plus its error + touched state, and the
/// `on_submit` handler. `Copy`.
pub struct FormState<T: Form> {
    data: Signal<T>,
    errors: Signal<FormErrors>,
    touched: Signal<BTreeSet<String>>,
    on_submit: Callback<T>,
}

// Hand-written so the `Copy`/`Clone` bounds don't spuriously require `T: Copy` — a
// `Signal<T>` is `Copy` for any `T: 'static`.
impl<T: Form> Clone for FormState<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Form> Copy for FormState<T> {}

impl<T: Form> FormState<T> {
    /// The whole draft — a snapshot for submit / persist.
    pub fn data(&self) -> T {
        self.data.read().clone()
    }

    /// True when no field is in error.
    pub fn is_valid(&self) -> bool {
        self.errors.read().is_empty()
    }

    /// Submit the form: if every field validates, hand the draft snapshot to the
    /// `on_submit` handler; otherwise mark every field touched (so errors surface) and
    /// do nothing. The validity gate lives here — wire a button straight to it
    /// (`onclick: move |_| form.submit()`), no `disabled` needed.
    pub fn submit(self) {
        if self.is_valid() {
            self.on_submit.call(self.data());
        } else {
            let mut touched = self.touched;
            let ids = self.data.read().field_ids();
            for id in ids {
                touched.write().insert(id);
            }
        }
    }

    /// The current string value of a field (`""` for an unknown id).
    pub fn value(&self, id: &str) -> String {
        self.data.read().get_field(id).unwrap_or_default()
    }

    /// A field's error message, if any.
    pub fn error(&self, id: &str) -> Option<String> {
        self.errors.read().get(id).cloned()
    }

    /// Whether a field has been touched (edited at least once).
    pub fn is_touched(&self, id: &str) -> bool {
        self.touched.read().contains(id)
    }

    /// Set a field, then validate **only that field** and record the result; marks it
    /// touched. The single write path — a [`FieldHandle`]'s commit calls this.
    pub fn set(self, id: &str, raw: &str) {
        let mut data = self.data;
        let mut errors = self.errors;
        let mut touched = self.touched;
        data.write().set_field(id, raw);
        let err = data.read().validate_field(id);
        match err {
            Some(e) => {
                errors.write().insert(id.to_string(), e);
            }
            None => {
                errors.write().remove(id);
            }
        }
        if !touched.read().contains(id) {
            touched.write().insert(id.to_string());
        }
    }

    /// Replace the whole draft and re-seed errors from it; clears touched.
    pub fn reset(self, value: T) {
        let mut data = self.data;
        let mut errors = self.errors;
        let mut touched = self.touched;
        let errs = value.errors();
        data.set(value);
        errors.set(errs);
        touched.write().clear();
    }

    /// Re-seed every field's error from the current draft (e.g. after an external edit).
    pub fn revalidate(self) {
        let mut errors = self.errors;
        let errs = self.data.read().errors();
        errors.set(errs);
    }

    /// A handle for `id` to bind to a control: current value + error + a commit callback
    /// (which writes the field and re-validates just that field).
    pub fn field(&self, id: &str) -> FieldHandle {
        let form = *self;
        let id_owned = id.to_string();
        FieldHandle {
            value: self.value(id),
            error: self.error(id),
            on_set: Callback::new(move |v: String| form.set(&id_owned, &v)),
        }
    }
}

/// A control's binding to one field: snapshot value + error + a commit callback. Built
/// by [`FormState::field`] and type-erased over the form model, so styled field
/// components need not be generic.
#[derive(Clone, PartialEq)]
pub struct FieldHandle {
    value: String,
    error: Option<String>,
    on_set: Callback<String>,
}

impl FieldHandle {
    /// Current value for the control's `value`.
    pub fn value(&self) -> String {
        self.value.clone()
    }
    /// Error message for the caption, if any.
    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }
    /// Whether to flag the control invalid.
    pub fn invalid(&self) -> bool {
        self.error.is_some()
    }
    /// The commit callback — wire the control's change handler to it.
    pub fn commit(&self) -> Callback<String> {
        self.on_set
    }
}

/// Create a [`FormState`] scoped to the calling component, owning a draft from `init`.
/// `on_submit` receives the draft snapshot when [`FormState::submit`] runs on a valid
/// form. Errors are seeded once from the initial draft (so a pre-existing invalid blocks
/// submit) — not re-run on every edit.
pub fn use_form<T: Form>(
    init: impl FnOnce() -> T,
    on_submit: impl FnMut(T) + 'static,
) -> FormState<T> {
    let data = use_signal(init);
    let errors = use_signal(FormErrors::new);
    let touched = use_signal(BTreeSet::new);
    let state = FormState {
        data,
        errors,
        touched,
        on_submit: Callback::new(on_submit),
    };
    use_effect(move || {
        // `peek` reads without subscribing → this effect runs a single time after mount,
        // not on every edit (per-field validation handles edits).
        let mut errors = errors;
        errors.set(data.peek().errors());
    });
    state
}

/// Reusable field validators — each returns `Ok(())` or a short user-facing message.
pub mod validators {
    /// Non-blank (after trimming).
    pub fn non_empty(v: &str) -> Result<(), String> {
        if v.trim().is_empty() {
            Err("Required.".to_string())
        } else {
            Ok(())
        }
    }

    /// A SQL-style identifier: leading letter/`_`, then letters/digits/`_`.
    pub fn ident(v: &str) -> Result<(), String> {
        let mut chars = v.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return Err("Must start with a letter or _.".to_string()),
        }
        if chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            Ok(())
        } else {
            Err("Only letters, digits or _.".to_string())
        }
    }

    /// A non-negative whole number.
    pub fn whole(v: &str) -> Result<(), String> {
        v.trim()
         .parse::<u64>()
         .map(|_| ())
         .map_err(|_| "Enter a whole number (0 or more).".to_string())
    }

    /// A byte size like `2G`, `512M`, or a plain byte count (blank allowed).
    pub fn size(v: &str) -> Result<(), String> {
        let v = v.trim();
        if v.is_empty() {
            return Ok(());
        }
        let num = v.strip_suffix(|c: char| "KMGkmg".contains(c)).unwrap_or(v);
        match num.trim().parse::<f64>() {
            Ok(n) if n >= 0.0 => Ok(()),
            _ => Err("Use a size like 2G, 512M, or a byte count.".to_string()),
        }
    }

    /// Require one of `choices`.
    pub fn one_of(choices: &'static [&'static str]) -> impl Fn(&str) -> Result<(), String> {
        move |v: &str| {
            if choices.contains(&v) {
                Ok(())
            } else {
                Err("Choose one of the listed options.".to_string())
            }
        }
    }

    /// A count that must be at least 1 (for `usize` fields).
    pub fn at_least_one(n: &usize) -> Result<(), String> {
        if *n >= 1 {
            Ok(())
        } else {
            Err("Must be at least 1.".to_string())
        }
    }

    /// A usable strftime pattern (chrono-style): non-empty and free of invalid
    /// specifiers. Requires the `chrono` feature.
    #[cfg(feature = "chrono")]
    pub fn strftime(pattern: &str) -> Result<(), String> {
        if pattern.trim().is_empty() {
            return Err("Required.".to_string());
        }
        for item in chrono::format::StrftimeItems::new(pattern) {
            if matches!(item, chrono::format::Item::Error) {
                return Err("Not a valid date/time format.".to_string());
            }
        }
        Ok(())
    }
}
