//! `Popup` — the **generic positioned-card base**: a single fixed-position card at a
//! client `Point`, nothing else. No dismissal, no focus, no branching — those are
//! *composed* around it:
//!
//! - a dismissable menu / dropdown = [`Backdrop`] `{ Popup { … } }` (the backdrop owns the
//!   click-catcher / Esc / focus + `on_close`);
//! - a hover tooltip = [`Tooltip`](super::tooltip::Tooltip), which is `Popup` + a
//!   pointer-transparent class.
//!
//! Keeping `Popup` a dumb positioner is what lets both reuse it. See
//! `docs/OVERLAY_ARCHITECTURE.md`.

use dioxus::prelude::*;

/// A screen point in client pixels — an overlay's anchor.
#[derive(Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A fixed-position card at `at`. `card_class` styles it (default the shared `ds-menu`);
/// `children` is the body. Stops click/contextmenu propagation so that, when composed
/// inside a [`Backdrop`], an inside-click doesn't bubble out and dismiss it.
#[component]
pub fn Popup(
    at: Point,
    card_class: Option<String>,
    width: Option<u32>,
    children: Element,
) -> Element {
    let (x, y) = (at.x, at.y);
    let card = card_class.unwrap_or_else(|| "ds-menu".to_string());
    let wstyle = width.map(|w| format!("width:{w}px;")).unwrap_or_default();
    rsx! {
        div {
            class: "{card}",
            style: "position:fixed;left:{x}px;top:{y}px;{wstyle}z-index:78;",
            onclick: move |e| e.stop_propagation(),
            oncontextmenu: move |e| e.stop_propagation(),
            {children}
        }
    }
}

/// Full-screen dismiss layer for a menu/dropdown: catches an outside click / right-click /
/// Esc and calls `on_close`, and grabs focus so Esc is caught without a document listener.
/// Compose it around a [`Popup`]: `Backdrop { on_close, Popup { at, … } }` — the `Popup`
/// card (z above this) stops propagation, so only outside-clicks reach `on_close`.
#[component]
pub fn Backdrop(on_close: EventHandler<()>, children: Element) -> Element {
    rsx! {
        div {
            class: "ctx-backdrop",
            tabindex: "0",
            onmounted: move |e| {
                spawn(async move { let _ = e.set_focus(true).await; });
            },
            // Don't let a dismiss-click bubble to a drag-on-mousedown parent (header).
            onmousedown: move |e| e.stop_propagation(),
            onclick: move |_| on_close.call(()),
            oncontextmenu: move |e| {
                e.prevent_default();
                on_close.call(());
            },
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    e.prevent_default();
                    on_close.call(());
                }
            },
            {children}
        }
    }
}
