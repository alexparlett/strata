//! The reusable resize handle + the window-fill toggle.
//!
//! `Resizer` is fully **self-contained**: it owns its drag in a component-local
//! `use_signal` and mutates the `size` signal it's handed. It uses HTML5 **drag**
//! events (`draggable` + `ondragstart`/`ondrag`/`ondragend`) rather than raw mouse
//! events: the webview does *not* capture the mouse to the mousedown element, so
//! `onmousemove` stops firing the moment the pointer leaves the thin handle — but a
//! native drag keeps delivering `ondrag` to the source element for the whole gesture,
//! wherever the pointer goes. No root driver and no shared resize state are needed —
//! the component that owns the panel owns its size (a local `Signal<f64>`).

use dioxus::prelude::*;

use crate::state::AppState;

/// Toggle the window between filling the screen work area and its previous size
/// (OS "zoom" — RustRover-style double-click-title-bar). Restores to the last
/// manual size; a manually screen-filled window is a no-op toggle.
pub fn toggle_window_fill(_state: Signal<AppState>) {
    dioxus::desktop::window().toggle_maximized();
}

/// A self-contained resize handle. Drags the `size` signal within `[min, max]`;
/// `axis_x` picks the tracked axis and `sign` its direction. Owns its drag locally —
/// no dispatch, no shared state. Render it as a sibling of the panel whose size it
/// controls (the panel owns `size`).
#[component]
pub fn Resizer(axis_x: bool, sign: f64, min: f64, max: f64, size: Signal<f64>) -> Element {
    let mut size = size;
    // (pointer origin on the drag axis, size at grab) while dragging, else None.
    let mut drag = use_signal(|| None::<(f64, f64)>);
    let axis = if axis_x { "col" } else { "row" };
    // Held lit for the whole drag (not just on hover) — matches the grid column grip.
    let dragging = if drag().is_some() { " resizing" } else { "" };
    rsx! {
        div {
            class: "resizer {axis}{dragging}",
            title: "Drag to resize",
            draggable: true,
            ondragstart: move |e| {
                let c = e.client_coordinates();
                drag.set(Some((if axis_x { c.x } else { c.y }, size())));
            },
            ondrag: move |e| {
                if let Some((origin, start)) = drag() {
                    let c = e.client_coordinates();
                    let cur = if axis_x { c.x } else { c.y };
                    // The final drag event (and the odd stray one) reports 0 — ignore it
                    // so the panel doesn't snap to its clamp on release.
                    if cur == 0.0 {
                        return;
                    }
                    size.set((start + (cur - origin) * sign).clamp(min, max));
                }
            },
            ondragend: move |_| drag.set(None),
        }
    }
}
