//! `Select` — the app-themed single-select dropdown (S29, design system §05). A trigger
//! button (current value + chevron) that opens a [`Popup`] anchored **below the trigger**
//! and width-matched, listing the options as menu rows with the selected one accent-tinted
//! + checked. Composes the base [`Popup`] (dismiss backdrop + Esc) — the trigger-relative
//! anchoring lives here so `Popup` stays a dumb positioner.

use dioxus::prelude::*;

use super::popup::{Point, Popup};
use crate::ui::icons;

/// One option in a [`Select`]: the stored `value` + its display `label`.
#[derive(Clone, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

impl SelectOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// Height of the `.ds-select` trigger (kept in sync with the CSS) — used to anchor the
/// menu just below the trigger from the click event, no async rect measurement.
const TRIGGER_H: f64 = 34.0;

/// App-themed dropdown. `value` is the current option's `value`; picking a row calls
/// `on_select` with the chosen value. `width` sizes both the trigger and the menu.
#[component]
pub fn Select(
    #[props(into)] value: String,
    options: Vec<SelectOption>,
    on_select: EventHandler<String>,
    #[props(default = 160)] width: u32,
    #[props(into, default)] placeholder: String,
) -> Element {
    let mut open = use_signal(|| false);
    let mut anchor = use_signal(|| Point { x: 0.0, y: 0.0 });

    let current = options
        .iter()
        .find(|o| o.value == value)
        .map(|o| o.label.clone())
        .unwrap_or_else(|| {
            if placeholder.is_empty() {
                value.clone()
            } else {
                placeholder.clone()
            }
        });

    rsx! {
        button {
            class: "ds-select",
            style: "width:{width}px;",
            // Trigger client top-left = cursor client − cursor element offset (constant for
            // the element); anchor the menu one trigger-height below it.
            onclick: move |e: MouseEvent| {
                let cp = e.client_coordinates();
                let ep = e.element_coordinates();
                anchor.set(Point { x: cp.x - ep.x, y: cp.y - ep.y + TRIGGER_H + 4.0 });
                open.set(!open());
            },
            span { class: "ds-select-val", "{current}" }
            {icons::chevron_down(12)}
        }
        if open() {
            Popup {
                on_close: move |_| open.set(false),
                at: anchor(),
                card_class: "ds-menu".to_string(),
                width,
                for opt in options.iter().cloned() {
                    {
                        let selected = opt.value == value;
                        let picked = opt.value.clone();
                        rsx! {
                            div {
                                key: "{opt.value}",
                                class: if selected { "ds-menu-item sel" } else { "ds-menu-item" },
                                onclick: move |e| {
                                    e.stop_propagation();
                                    on_select.call(picked.clone());
                                    open.set(false);
                                },
                                span { class: "ds-mi-label", "{opt.label}" }
                                if selected {
                                    span { class: "ds-mi-check", {icons::check(13)} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
