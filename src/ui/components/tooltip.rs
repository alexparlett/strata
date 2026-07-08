//! `Tooltip` — a floating, non-dismissing hover card. It **is a [`Popup`]** (the base
//! positioner) plus the `ds-float` class, which makes the card pointer-transparent so it
//! never steals focus or swallows the `mousemove`/`mouseleave` on the surface underneath
//! that drive it. No backdrop, no dismissal — the caller mounts it conditionally and
//! unmounts on leave. Default chrome is the neutral `.ds-tooltip` (§07). Used by the SQL
//! lint hover popover (S27).

use dioxus::prelude::*;

use super::popup::{Point, Popup, Rect, RectAlign};

/// A pointer-transparent floating card anchored at `at`, placed by `align` (default
/// `BOTTOM_START` — just below/right of the point). `card_class` styles it (default the
/// neutral `ds-tooltip`); `children` is the body.
#[component]
pub fn Tooltip(
    at: Point,
    #[props(default)] align: RectAlign,
    #[props(into, default)] card_class: String,
    width: Option<u32>,
    children: Element,
) -> Element {
    let base = if card_class.is_empty() {
        "ds-tooltip"
    } else {
        card_class.as_str()
    };
    // `ds-float` = pointer-events:none — the only thing that makes this a tooltip vs a menu.
    let card = format!("{base} ds-float");
    rsx! {
        Popup { anchor: Rect::point(at.x, at.y), align, card_class: card, width, {children} }
    }
}
