//! `Popup` — an anchored, self-contained overlay container (context menu /
//! dropdown). egui-style: mount it conditionally (`if open { Popup { … } }`) and it
//! owns positioning, the full-screen click-catcher, Esc handling, and dismissal via
//! `on_close`. The caller owns *whether* it's mounted (a local `use_signal`) and
//! supplies the body as `children`. No central `AppState` enum, no reducer. See
//! `docs/OVERLAY_ARCHITECTURE.md`.

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
pub fn Popup(
    #[props(default = EventHandler::new(|_: ()| {}))] on_close: EventHandler<()>,
    at: Point,
    /// Card class — defaults to the context-menu look (`ctx-menu`). Pass e.g.
    /// `"menu"` for the richer dropdown chrome.
    card_class: Option<String>,
    /// Fixed card width in px (else content-sized).
    width: Option<u32>,
    /// Whether to render the full-screen click-catcher backdrop that dismisses on
    /// outside-click / right-click / Esc (and grabs focus so Esc is caught). Default
    /// `true` — a menu/dropdown. Pass `false` for a **hover tooltip**: no backdrop (so it
    /// never steals the editor's focus nor swallows the `mousemove` that drives
    /// show/hide) and a pointer-transparent card. Dismissal is then the caller's job —
    /// unmount it on mouseleave. (`on_close`/Esc are inert.) Used by the lint popover.
    #[props(default = true)]
    backdrop: bool,
    children: Element,
) -> Element {
    let (x, y) = (at.x, at.y);
    let card = card_class.unwrap_or_else(|| "ctx-menu".to_string());
    let wstyle = width.map(|w| format!("width:{w}px;")).unwrap_or_default();
    // A tooltip (`backdrop:false`) is a *pass-through* layer: `pointer-events:none` so it
    // never captures clicks or swallows the editor's `mousemove`, and it doesn't grab
    // focus. A menu (`backdrop:true`) captures + dismisses + focuses for Esc. The two
    // differ only in these attribute values, so it's one render tree.
    let pe = if backdrop { "auto" } else { "none" };
    rsx! {
        div {
            // Focusable so Escape is caught without a document-level listener (menu only —
            // `set_focus(false)` is a no-op, so a tooltip never steals the editor's focus).
            class: "ctx-backdrop",
            style: "pointer-events:{pe};",
            tabindex: "0",
            onmounted: move |e| {
                spawn(async move { let _ = e.set_focus(backdrop).await; });
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
            div {
                class: "{card}",
                style: "position:fixed;left:{x}px;top:{y}px;{wstyle}pointer-events:{pe};",
                onclick: move |e| e.stop_propagation(),
                oncontextmenu: move |e| e.stop_propagation(),
                {children}
            }
        }
    }
}
