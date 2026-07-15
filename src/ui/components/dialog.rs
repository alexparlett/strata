//! `Dialog` — a centred, scrimmed, blocking overlay container (confirm prompt /
//! quick view). egui-style, mirroring [`Popup`](super::popup::Popup): mount it
//! conditionally (`if let Some(t) = target() { Dialog { … } }`) and it owns the
//! dimming scrim, centring, focus, and Esc; the caller owns *whether* it's mounted
//! (a local `use_signal`) and hands in the card body as `children`. Dismissal
//! (backdrop click / Esc) calls `on_close`. See `docs/OVERLAY_ARCHITECTURE.md`.

use dioxus::prelude::*;

/// Centred modal dialog. Renders a dimming, blocking scrim (`.overlay`) with the
/// card (`card_class`) centred inside; calls `on_close` on backdrop-click or Esc.
/// The card `stop_propagation`s so clicks inside don't dismiss it.
#[component]
pub fn Dialog(
    on_close: EventHandler<()>,
    /// The card's own chrome class (e.g. `"confirm"`, `"cmdk"`, `"modal cell-modal"`).
    card_class: String,
    /// Stacking order (dialogs differ: cell 64, cmdk 70, remove 78). Default 60 —
    /// the `.overlay` base.
    z: Option<u32>,
    /// Top-align the card instead of centring it (command palette).
    #[props(default)]
    top: bool,
    /// The body autofocuses its own field (e.g. a search input). The container then
    /// leaves focus to that field and relies on Esc bubbling up to the scrim's
    /// `onkeydown`; otherwise the scrim grabs focus on mount so Esc works.
    #[props(default)]
    has_input: bool,
    children: Element,
) -> Element {
    let z = z.unwrap_or(60);
    let scrim = if top { "overlay top" } else { "overlay" };
    rsx! {
        div {
            class: "{scrim}",
            style: "z-index:{z};",
            // Focusable so Escape is caught without a document-level listener — but
            // only grab focus when the body has no field of its own to focus.
            tabindex: "0",
            onmounted: move |e| {
                if !has_input {
                    spawn(async move { let _ = e.set_focus(true).await; });
                }
            },
            onclick: move |_| on_close.call(()),
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    e.prevent_default();
                    // Don't let the root's Escape (query-cancel) handler also fire.
                    e.stop_propagation();
                    on_close.call(());
                }
            },
            div {
                class: "{card_class}",
                onclick: move |e| e.stop_propagation(),
                {children}
            }
        }
    }
}
