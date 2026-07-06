//! Menu content primitives for a [`Popup`](super::popup::Popup): a clickable
//! [`MenuItem`] row and a [`MenuSep`] divider.

use dioxus::prelude::*;

/// A clickable row inside a [`Popup`](super::popup::Popup) menu. The caller's
/// `onclick` should both dismiss the popup (clear its signal) and perform the
/// action.
#[component]
pub fn MenuItem(
    icon: Option<Element>,
    label: String,
    meta: Option<String>,
    #[props(default)] danger: bool,
    #[props(default)] disabled: bool,
    onclick: EventHandler<()>,
) -> Element {
    let cls = if disabled {
        "ctx-item disabled"
    } else if danger {
        "ctx-item danger"
    } else {
        "ctx-item"
    };
    rsx! {
        div {
            class: "{cls}",
            onclick: move |e| {
                e.stop_propagation();
                if !disabled {
                    onclick.call(());
                }
            },
            if let Some(ic) = icon {
                span { class: "ci", {ic} }
            }
            span { style: "flex:1;", "{label}" }
            if let Some(m) = meta {
                span { class: "kbd-hint", "{m}" }
            }
        }
    }
}

/// A divider between [`MenuItem`]s.
#[component]
pub fn MenuSep() -> Element {
    rsx! { div { class: "ctx-sep" } }
}
