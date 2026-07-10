//! `Segment` — the single-select "multi-button" of the design system
//! (`docs/DESIGN_SYSTEM.md` §06). Part of the **S28** control library.
//!
//! A pill container of mutually-exclusive options; the selected one gets the
//! **soft accent tint** (not a solid fill). Controlled via `value` + `on_select`
//! (string-keyed) — the exact shape of [`Select`](super::Select) / [`SelectOption`],
//! so `Segment { options, value, on_select }` reads like its dropdown sibling.
//!
//! Option icons are held as a typed [`IconName`] (which is `Copy + PartialEq`) so a
//! [`SegmentOption`] stays `Clone + PartialEq` and can live in the `Vec<SegmentOption>`
//! prop; the glyph is rendered at 14px via `IconName::el`.

use dioxus::prelude::*;

use crate::ui::icons::{IconName, IconSize};

/// One choice in a [`Segment`] control.
#[derive(Clone, PartialEq)]
pub struct SegmentOption {
    /// The stable value emitted on select.
    pub value: String,
    /// The visible label.
    pub label: String,
    /// Optional leading icon (a typed [`IconName`], rendered at 14px).
    pub icon: Option<IconName>,
}

impl SegmentOption {
    /// A label-only option.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            icon: None,
        }
    }

    /// An option with a leading icon (pass an [`IconName`], e.g. `IconName::Table`).
    pub fn with_icon(value: impl Into<String>, label: impl Into<String>, icon: IconName) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            icon: Some(icon),
        }
    }
}

/// A row of options; the one whose value equals `value` is selected. Emits the
/// clicked option's value through `on_select`.
#[component]
pub fn Segment(
    options: Vec<SegmentOption>,
    #[props(into)] value: String,
    on_select: EventHandler<String>,
    #[props(default)] disabled: bool,
    /// Compact (26px) toolbar density — the tight in-toolbar segmented (Table/Chart,
    /// Physical/Logical) rather than the standard §06 size.
    #[props(default)]
    compact: bool,
) -> Element {
    let cls = {
        let mut c = String::from("ds-seg");
        if compact {
            c.push_str(" compact");
        }
        if disabled {
            c.push_str(" disabled");
        }
        c
    };
    rsx! {
        div {
            class: "{cls}",
            "role": "tablist",
            for opt in options {
                {
                    let selected = opt.value == value;
                    let v = opt.value.clone();
                    rsx! {
                        button {
                            key: "{opt.value}",
                            r#type: "button",
                            "role": "tab",
                            class: if selected { "ds-seg-item on" } else { "ds-seg-item" },
                            "aria-selected": if selected { "true" } else { "false" },
                            disabled: disabled,
                            onclick: move |_| { if !disabled { on_select.call(v.clone()); } },
                            if let Some(ic) = opt.icon {
                                span { class: "ds-seg-ico", {ic.el(IconSize::Sm)} }
                            }
                            span { "{opt.label}" }
                        }
                    }
                }
            }
        }
    }
}
