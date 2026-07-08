//! `DropdownMenu` — a click-opened **action menu owned by its trigger button** (S29). You
//! give it the trigger content + the menu rows; it owns the open state, measures the
//! trigger (`get_client_rect`) to anchor the menu directly beneath it (left- or
//! right-aligned), and dismisses via the base [`Backdrop`]. Distinct from:
//! - [`Select`](super::select::Select) — a value picker (shows the current value);
//! - [`ContextMenu`](super::context_menu::ContextMenu) — opened by right-click at the cursor.
//!
//! Any click inside the menu closes it (so action rows just do their thing); interactive
//! content that must stay open (a search field) should `stop_propagation` on its click.

use std::rc::Rc;

use dioxus::prelude::*;

use super::popup::{Backdrop, Point, Popup};

/// A trigger `<button>` (styled by `class`) that opens `children` as a menu beneath it.
#[component]
pub fn DropdownMenu(
    /// Trigger button content (icon / label / chevron).
    trigger: Element,
    #[props(into, default)] class: String,
    #[props(into, default)] title: String,
    /// Menu width in px (also the width used for right-alignment).
    width: Option<u32>,
    /// Right-align the menu's right edge to the trigger's (default: left-align).
    #[props(default)] align_right: bool,
    /// Menu rows.
    children: Element,
) -> Element {
    let mut open = use_signal(|| false);
    let mut anchor = use_signal(|| Point { x: 0.0, y: 0.0 });
    let mut trigger_ref = use_signal(|| None::<Rc<MountedData>>);

    rsx! {
        button {
            class: "{class}",
            title: "{title}",
            onmounted: move |e| trigger_ref.set(Some(e.data())),
            onmousedown: move |e| e.stop_propagation(),
            ondoubleclick: move |e| e.stop_propagation(),
            onclick: move |_| {
                let handle = trigger_ref.peek().clone();
                spawn(async move {
                    let Some(h) = handle else { return };
                    if let Ok(r) = h.get_client_rect().await {
                        let mw = width.map(|w| w as f64).unwrap_or(r.size.width);
                        let x = if align_right {
                            r.origin.x + r.size.width - mw
                        } else {
                            r.origin.x
                        };
                        anchor.set(Point { x, y: r.origin.y + r.size.height + 4.0 });
                        open.set(true);
                    }
                });
            },
            {trigger}
        }
        if open() {
            Backdrop { on_close: move |_| open.set(false),
                Popup { at: anchor(), width,
                    // Bubbled inner click closes the menu (action rows); a search field
                    // etc. stops propagation to stay open.
                    div { onclick: move |_| open.set(false), {children} }
                }
            }
        }
    }
}
