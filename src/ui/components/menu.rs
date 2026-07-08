//! Menu content primitives for a [`Popup`](super::popup::Popup) / [`ContextMenu`]
//! (super::context_menu::ContextMenu) / [`Select`](super::select::Select) — a clickable
//! [`MenuItem`] row and a [`MenuSep`] divider, both on the shared design-system
//! `.ds-menu-item` / `.ds-menu-sep` styling (Canvas §05/§07).

use dioxus::prelude::*;

use crate::ui::icons;

/// A clickable row inside a menu card. The caller's `onclick` should both dismiss the
/// menu (clear its signal) and perform the action. `selected` tints the row + adds a
/// check (for single-select menus); `danger` reddens; `disabled` dims + inert.
#[component]
pub fn MenuItem(
    icon: Option<Element>,
    label: String,
    meta: Option<String>,
    #[props(default)] danger: bool,
    #[props(default)] disabled: bool,
    #[props(default)] selected: bool,
    onclick: EventHandler<()>,
) -> Element {
    let mut cls = String::from("ds-menu-item");
    if disabled {
        cls.push_str(" disabled");
    } else if selected {
        cls.push_str(" sel");
    }
    if danger {
        cls.push_str(" danger");
    }
    rsx! {
        div {
            class: "{cls}",
            // No stop_propagation: the click bubbles to the menu's close-wrapper
            // (ContextMenu / DropdownMenu), which dismisses. The row just does its action.
            onclick: move |_| {
                if !disabled {
                    onclick.call(());
                }
            },
            if let Some(ic) = icon {
                span { class: "ds-mi-ico", {ic} }
            }
            span { class: "ds-mi-label", "{label}" }
            if let Some(m) = meta {
                span { class: "kbd-hint", "{m}" }
            }
            if selected {
                span { class: "ds-mi-check", {icons::check(13)} }
            }
        }
    }
}

/// A divider between [`MenuItem`]s.
#[component]
pub fn MenuSep() -> Element {
    rsx! { div { class: "ds-menu-sep" } }
}
