//! `Tooltip` — a floating, non-dismissing hover card. It **is a [`Popup`]** (the base
//! positioner) plus the `ds-float` class, which makes the card pointer-transparent so it
//! never steals focus or swallows the `mousemove`/`mouseleave` on the surface underneath
//! that drive it. No backdrop, no dismissal — the caller mounts it conditionally and
//! unmounts on leave. Used by the SQL lint hover popover (S27), styled as a `.ds-callout`.

use dioxus::prelude::*;

use super::popup::{Point, Popup};

/// A pointer-transparent floating card at `at`. `card_class` styles it (default
/// `ds-callout`); `children` is the body.
#[component]
pub fn Tooltip(
    at: Point,
    #[props(into, default)] card_class: String,
    width: Option<u32>,
    children: Element,
) -> Element {
    let base = if card_class.is_empty() {
        "ds-callout"
    } else {
        card_class.as_str()
    };
    // `ds-float` = pointer-events:none — the only thing that makes this a tooltip vs a menu.
    let card = format!("{base} ds-float");
    rsx! {
        Popup { at, card_class: card, width, {children} }
    }
}
