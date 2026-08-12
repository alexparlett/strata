//! The **modal base** — what makes a centred surface modal, with no opinion about the card
//! on it or what confirming it means: the overlay layer over the window content, the
//! backdrop, and the key barrier. A modal is **open or closed**, nothing else — Esc and a
//! press outside the card are both a *close request*, and whether "confirm" even exists is
//! the surface's own semantic, handled on its own card.
//!
//! [`Dialog`](super::dialog::Dialog) wraps its confirm card in this; a working surface with
//! its own proportions (the Shape panel) wraps its own card in the same base — so "how a
//! modal behaves" is written once and "what a modal looks like" stays each surface's.
//!
//! ## The barrier, and the one key it leaves alone
//!
//! Same-name global listeners fire in **pre-order**, and a prevented key stops the listeners
//! after it (the fork's guarantee — `keymap`'s module note). This wrapper is the card's
//! ancestor, so anything it consumes never reaches the card: the barrier therefore consumes
//! every key **except Enter**, which is deliberately left to fall through to the surface's
//! own card handler — the Dialog confirms on it; a surface with no confirm should consume it
//! there instead. Everything below the modal in document order still sees nothing, because
//! whichever of the two consumed the key did so before the features' listeners run.
//!
//! The barrier's one honest limit is unchanged from the dialog it came from: `KeyDown`
//! outranks `GlobalKeyDown`, so a *focused* element (the SQL editor) still sees keys first.
//! Mount it only while the surface is open — it renders no closed state.

use freya::components::PopupBackground;
use freya::prelude::*;

use crate::theme::{use_roles, Role};

/// The overlay + barrier + backdrop around one card.
#[derive(PartialEq)]
pub struct Modal {
    card: Element,
    on_close_request: Option<EventHandler<()>>,
    barrier: bool,
}

impl Modal {
    pub fn new(card: impl IntoElement) -> Self {
        Self {
            card: card.into_element(),
            on_close_request: None,
            barrier: true,
        }
    }

    /// Whether the key barrier consumes keys at all (the default, right for a surface
    /// raised over live features). Pass `false` for one that *is* the window's whole
    /// content: Esc keeps its close meaning and every other chord stays the window's.
    pub fn barrier(mut self, barrier: bool) -> Self {
        self.barrier = barrier;
        self
    }

    /// Esc, or a press on the backdrop — the user asking for the modal to close. Freya's
    /// own name for the same semantic (`Popup::on_close_request`): the modal reports the
    /// ask, and the owner of the open/closed state acts on it.
    pub fn on_close_request(mut self, on_close_request: impl Into<EventHandler<()>>) -> Self {
        self.on_close_request = Some(on_close_request.into());
        self
    }
}

impl Component for Modal {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        let close = self.on_close_request.clone();
        let backdrop_close = self.on_close_request.clone();

        rect()
            // The overlay layer + global position lift the whole surface above the window
            // content (the same wrapper `Popup` puts around `PopupBackground`).
            .layer(Layer::Overlay)
            .position(Position::new_global())
            .on_global_key_down({
                let barrier = self.barrier;
                move |e: Event<KeyboardEventData>| {
                    match &e.key {
                        Key::Named(NamedKey::Escape) => {
                            if let Some(close) = &close {
                                close.call(());
                            }
                        }
                        // Deliberately not consumed here: this node is the card's ancestor
                        // and fires first, so a consumed Enter would never reach the
                        // surface's own confirm handler — see the module note.
                        Key::Named(NamedKey::Enter) => return,
                        _ if !barrier => return,
                        _ => {}
                    }
                    // Consumed either way — that is what makes a modal surface modal, and
                    // Esc its own in both modes.
                    e.prevent_default();
                }
            })
            .child(PopupBackground::new(
                self.card.clone(),
                move |_| {
                    if let Some(close) = &backdrop_close {
                        close.call(());
                    }
                },
                roles.get(Role::Backdrop),
            ))
    }
}
