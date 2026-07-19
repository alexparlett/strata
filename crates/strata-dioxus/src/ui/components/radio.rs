//! `RadioGroup` + `Radio` — the exclusive-choice control of the design system
//! (`docs/DESIGN_SYSTEM.md` §06). Part of the **S28** control library.
//!
//! Unlike a [`Checkbox`](super::Checkbox) (a genuinely standalone on/off), a radio
//! only means anything **within a group**, so the group is the real component:
//! [`RadioGroup`] takes `options` + a single `value` + `on_select` — the same
//! shape as [`Select`](super::Select) / [`Segment`](super::Segment). [`Radio`] is
//! the single row it renders (a labelled circle); expose it for hand-built layouts.
//! The inner dot is a CSS `::after` on `.ds-radio.on`.

use dioxus::prelude::*;

/// One choice in a [`RadioGroup`].
#[derive(Clone, PartialEq)]
pub struct RadioOption {
    pub value: String,
    pub label: String,
}

impl RadioOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// A vertical set of mutually-exclusive options; the one whose value equals
/// `value` is selected. Emits the chosen value through `on_select`.
#[component]
pub fn RadioGroup(
    options: Vec<RadioOption>,
    #[props(into)] value: String,
    on_select: EventHandler<String>,
    #[props(default)] disabled: bool,
) -> Element {
    rsx! {
        div { class: "ds-radio-group", "role": "radiogroup",
            for opt in options {
                {
                    let selected = opt.value == value;
                    let v = opt.value.clone();
                    rsx! {
                        Radio {
                            key: "{opt.value}",
                            label: opt.label.clone(),
                            selected: selected,
                            disabled: disabled,
                            on_select: move |_| on_select.call(v.clone()),
                        }
                    }
                }
            }
        }
    }
}

/// A single radio row (circle + text `label`). Controlled — the owner tracks
/// exclusivity; usually rendered for you by [`RadioGroup`].
#[component]
pub fn Radio(
    #[props(into)] label: String,
    selected: bool,
    on_select: EventHandler<()>,
    #[props(default)] disabled: bool,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            "role": "radio",
            "aria-checked": if selected { "true" } else { "false" },
            class: if disabled { "ds-radio-row disabled" } else { "ds-radio-row" },
            disabled: disabled,
            onclick: move |_| { if !disabled { on_select.call(()); } },
            span { class: if selected { "ds-radio on" } else { "ds-radio" } }
            span { class: "ds-radio-label", "{label}" }
        }
    }
}
