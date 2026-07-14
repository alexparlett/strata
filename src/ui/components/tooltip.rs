//! `Tooltip` — the app-themed hover tooltip: a wrapper that gives its `children` (any
//! trigger) a floating message card while the pointer is over them. It composes [`Popup`]
//! (the base positioner) with the neutral `.ds-tooltip` chrome (§07) + the pointer-transparent
//! `ds-float` class, anchored just below the cursor. Backs the `title` prop on [`Button`],
//! [`IconButton`], and [`Badge`], and wraps bespoke triggers directly.
//!
//! A tooltip *card pinned at a computed point* — not hovering a trigger, e.g. the editor's
//! lint hover — uses `Popup { card_class: "ds-tooltip ds-float", … }` directly, not this.

use dioxus::prelude::*;

use super::popup::{Point, Popup, Rect};
use super::typography::Prose;

/// A hover wrapper that gives its `children` an app-themed tooltip (never a native `title=`),
/// shown just below the cursor while hovered. When `message` is empty it renders `children`
/// untouched — no wrapper element, no behaviour — so controls can pass it through
/// unconditionally (an empty `title` costs nothing). Backs the `title` prop on [`Button`],
/// [`IconButton`], and [`Badge`], and can wrap any bespoke trigger directly.
#[component]
pub fn Tooltip(#[props(into)] message: String, children: Element) -> Element {
    let mut at = use_signal(|| None::<Point>);
    if message.is_empty() {
        return rsx! { {children} };
    }
    rsx! {
        span {
            class: "ds-tt-anchor",
            onmouseenter: move |e| {
                let c = e.client_coordinates();
                at.set(Some(Point { x: c.x, y: c.y + 16.0 }));
            },
            onmouseleave: move |_| at.set(None),
            {children}
        }
        if let Some(p) = at() {
            // `ds-float` (pointer-events:none) is what makes the card a tooltip vs a menu —
            // it never steals the hover that drives it. Placement defaults to BOTTOM_START
            // (below/right of the cursor), auto-flipping near an edge (see `Popup`).
            Popup {
                anchor: Rect::point(p.x, p.y),
                card_class: "ds-tooltip ds-float",
                Prose { class: "ds-tt-msg", "{message}" }
            }
        }
    }
}
