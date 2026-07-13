//! App-side **styled form fields** over the headless [`strata_forms`] layer.
//!
//! `strata_forms` owns the data + validity (see its `Form` trait / `FormState` /
//! `#[derive(Form)]`); this module renders *our* controls bound to a
//! [`strata_forms::FieldHandle`] and shows the inline error caption. It lives in the
//! app (not the crate) because it references the app's control library — keeping the
//! crate renderer-agnostic.
//!
//! - [`FormField`] renders one of the common controls ([`FieldKind`]) already wired to
//!   a handle (value + `invalid` + commit) with the error caption underneath.
//! - [`Field`] is chrome for a bespoke control (a path row with a folder-picker, …):
//!   render your own markup as `children`, it adds the caption for the field.
//!
//! The headless pieces are re-exported here so call sites use one path.

use dioxus::prelude::*;

use super::{Caption, NumberStepper, Select, SelectOption, TextInput, Toggle};

pub use strata_forms::{
    use_form, validators, FieldHandle, Form, FormErrors, FormState, FormValue,
};

/// Which control a [`FormField`] renders. Values cross as `String` (matching the
/// headless boundary), so each kind parses/formats at its edge.
#[derive(Clone, PartialEq)]
pub enum FieldKind {
    /// A free-text field. `mono` renders the value monospaced.
    Text { placeholder: String, mono: bool },
    /// A clamped numeric stepper.
    Int {
        min: i64,
        max: Option<i64>,
        step: i64,
    },
    /// A single-select dropdown.
    Select(Vec<SelectOption>),
    /// An on/off toggle.
    Bool,
}

/// A styled, validated field: renders the control named by `kind`, bound to `field`
/// (value + `invalid` + commit from the form), with the error caption underneath. The
/// form validates the field the moment the control commits — no wiring here.
#[component]
pub fn FormField(
    field: FieldHandle,
    kind: FieldKind,
    #[props(default = 168)] width: u32,
) -> Element {
    let value = field.value();
    let invalid = field.invalid();
    let error = field.error();
    let commit = field.commit();

    let control = match kind {
        FieldKind::Text { placeholder, mono } => rsx! {
            TextInput {
                value: value.clone(),
                mono,
                placeholder,
                invalid,
                width,
                oninput: move |_| {},
                onchange: move |v: String| commit.call(v),
            }
        },
        FieldKind::Int { min, max, step } => {
            let n = value.parse::<i64>().unwrap_or(min);
            rsx! {
                NumberStepper {
                    value: n,
                    min: Some(min),
                    max,
                    step,
                    width,
                    on_change: move |v: i64| commit.call(v.to_string()),
                }
            }
        }
        FieldKind::Select(options) => rsx! {
            Select {
                value: value.clone(),
                width,
                options,
                on_select: move |v: String| commit.call(v),
            }
        },
        FieldKind::Bool => {
            let on = value == "true";
            rsx! {
                Toggle {
                    on,
                    on_toggle: move |_| commit.call((!on).to_string()),
                }
            }
        }
    };

    rsx! {
        div { class: "ds-formfield",
            {control}
            if let Some(e) = error {
                Caption { class: "ds-formfield-err", "{e}" }
            }
        }
    }
}

/// Chrome for a bespoke field: render your own control(s) as `children`, and this adds
/// the inline error caption from `field`. Use when the control isn't one of
/// [`FieldKind`] (a path row with a folder-picker, an indexed list row, …).
#[component]
pub fn Field(field: FieldHandle, children: Element) -> Element {
    rsx! {
        div { class: "ds-formfield",
            {children}
            if let Some(e) = field.error() {
                Caption { class: "ds-formfield-err", "{e}" }
            }
        }
    }
}
