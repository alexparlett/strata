//! `Select` — the app-themed single-select dropdown (S29, design system §05). A trigger
//! button (current value + chevron) opens a [`Popup`] placed against the trigger by a
//! [`RectAlign`] (default `BOTTOM_START`; e.g. `TOP_START` to open upward), listing the
//! options with the selected one accent-tinted + checked. Composes the base [`Popup`] +
//! [`Backdrop`]; the trigger is measured (`get_client_rect`) so the menu anchors to it.

use std::rc::Rc;

use dioxus::prelude::*;

use super::popup::{Backdrop, Popup, Rect, RectAlign};
use super::Icon;
use crate::ui::icons::{IconName, IconSize};

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

/// App-themed dropdown. `value` is the current option's `value`; picking a row calls
/// `on_select` with the chosen value. `width` sizes the trigger + menu; `align` places
/// the menu relative to the trigger.
#[component]
pub fn Select(
    #[props(into)] value: String,
    options: Vec<SelectOption>,
    on_select: EventHandler<String>,
    #[props(default = 160)] width: u32,
    #[props(into, default)] placeholder: String,
    #[props(default)] align: RectAlign,
) -> Element {
    let mut open = use_signal(|| false);
    let mut anchor = use_signal(|| Rect::point(0.0, 0.0));
    let mut trigger_ref = use_signal(|| None::<Rc<MountedData>>);

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
            onmounted: move |e| trigger_ref.set(Some(e.data())),
            onclick: move |_| {
                let handle = trigger_ref.peek().clone();
                spawn(async move {
                    let Some(h) = handle else { return };
                    if let Ok(r) = h.get_client_rect().await {
                        anchor.set(Rect { x: r.origin.x, y: r.origin.y, w: r.size.width, h: r.size.height });
                        open.set(true);
                    }
                });
            },
            span { class: "ds-select-val", "{current}" }
            Icon { name: IconName::ChevronDown, size: IconSize::Xs }
        }
        if open() {
            Backdrop { on_close: move |_| open.set(false),
                Popup {
                    anchor: anchor(),
                    align,
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
                                        span { class: "ds-mi-check", Icon { name: IconName::Check, size: IconSize::Sm } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
