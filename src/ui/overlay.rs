//! Overlay containers (A3). egui-style, self-contained components you hand
//! content to — the container owns positioning, chrome, and dismissal; the caller
//! owns *whether* it's mounted (a local `use_signal`) and supplies the body as
//! `children`. No central `AppState` enum, no reducer. See
//! `docs/OVERLAY_ARCHITECTURE.md`.
//!
//! This module ships the **`Popup`** container (anchored menu / dropdown) plus the
//! `MenuItem` / `MenuSep` content primitives. `Window` / `Dialog` follow.

use dioxus::prelude::*;

/// A screen point in client pixels — a popup's anchor.
#[derive(Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Anchored popup container (context menu / dropdown). Mount it conditionally
/// (`if open { Popup { … } }`); it renders a full-screen click-catcher + a card
/// positioned at `at`, and calls `on_close` on outside-click, right-click, or Esc.
/// The card stops propagation so clicks inside don't dismiss it.
#[component]
pub fn Popup(on_close: EventHandler<()>, at: Point, children: Element) -> Element {
    let (x, y) = (at.x, at.y);
    rsx! {
        div {
            // Focusable so Escape is caught without a document-level listener.
            class: "ctx-backdrop",
            tabindex: "0",
            onmounted: move |e| {
                spawn(async move { let _ = e.set_focus(true).await; });
            },
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
            div {
                class: "ctx-menu",
                style: "left:{x}px;top:{y}px;",
                onclick: move |e| e.stop_propagation(),
                oncontextmenu: move |e| e.stop_propagation(),
                {children}
            }
        }
    }
}

/// A clickable row inside a [`Popup`] menu. The caller's `onclick` should both
/// dismiss the popup (clear its signal) and perform the action.
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
