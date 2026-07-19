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

use super::popup::{Backdrop, Popup, Rect, RectAlign};

/// A trigger `<button>` (styled by `class`) that opens `children` as a menu placed against
/// it by `align` (default `BOTTOM_START`; e.g. `BOTTOM_END` to right-align).
#[component]
pub fn DropdownMenu(
    /// Trigger button content (icon / label / chevron).
    trigger: Element,
    #[props(into, default)] class: String,
    #[props(into, default)] style: String,
    #[props(into, default)] title: String,
    /// Menu width in px.
    width: Option<u32>,
    /// Placement of the menu relative to the trigger.
    #[props(default)]
    align: RectAlign,
    /// Optional caller-owned open state — pass one when the menu content must dismiss
    /// itself programmatically (e.g. a search box closing on Enter). Defaults to internal.
    #[props(default)]
    open: Option<Signal<bool>>,
    /// Menu rows.
    children: Element,
) -> Element {
    let internal = use_signal(|| false);
    let mut open = open.unwrap_or(internal);
    let mut anchor = use_signal(|| Rect::point(0.0, 0.0));
    let mut trigger_ref = use_signal(|| None::<Rc<MountedData>>);

    rsx! {
        button {
            class: "{class}",
            style: "{style}",
            title: "{title}",
            onmounted: move |e| trigger_ref.set(Some(e.data())),
            onmousedown: move |e| e.stop_propagation(),
            ondoubleclick: move |e| e.stop_propagation(),
            onclick: move |e| {
                // Don't let opening the menu also fire a parent row's click handler.
                e.stop_propagation();
                let handle = trigger_ref.peek().clone();
                spawn(async move {
                    let Some(h) = handle else { return };
                    if let Ok(r) = h.get_client_rect().await {
                        anchor.set(Rect { x: r.origin.x, y: r.origin.y, w: r.size.width, h: r.size.height });
                        open.set(true);
                    }
                });
            },
            {trigger}
        }
        if open() {
            Backdrop { on_close: move |_| open.set(false),
                Popup { anchor: anchor(), align, width,
                    // Bubbled inner click closes the menu (action rows); a search field
                    // etc. stops propagation to stay open.
                    div { onclick: move |_| open.set(false), {children} }
                }
            }
        }
    }
}
