//! `ContextMenu` — a right-click menu on the shared [`Popup`] base (S29). **Controlled:**
//! the caller wires `oncontextmenu` on its target (`prevent_default()` + store the cursor
//! `Point` in a signal) and hands that here as `at`; this renders the shared `.ds-menu`
//! card at the anchor with the caller's rows (`.ds-menu-item`) as `children`. Dismissal —
//! outside-click / right-click / Esc — is the base `Popup`'s. Keyboard roving can layer on
//! later without touching consumers.

use dioxus::prelude::*;

use super::popup::{Backdrop, Point, Popup, Rect};

/// Right-click menu. `at` is `Some(point)` (the cursor anchor) while open, `None` closed;
/// `on_close` clears it. Children are the menu rows (`.ds-menu-item` / `.ds-menu-sep`).
#[component]
pub fn ContextMenu(
    at: Option<Point>,
    on_close: EventHandler<()>,
    #[props(default = 0)] width: u32,
    children: Element,
) -> Element {
    rsx! {
        if let Some(p) = at {
            Backdrop { on_close: move |_| on_close.call(()),
                Popup {
                    anchor: Rect::point(p.x, p.y),
                    card_class: "ds-menu".to_string(),
                    width: if width > 0 { Some(width) } else { None },
                    // Any bubbled inner click dismisses (action rows just do their thing).
                    div { onclick: move |_| on_close.call(()), {children} }
                }
            }
        }
    }
}
