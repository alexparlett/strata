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
//! ## The barrier, and the two keys it leaves alone
//!
//! Same-name global listeners fire in **pre-order**, and a prevented key stops the listeners
//! after it (the fork's guarantee — `keymap`'s module note). This wrapper is the card's
//! ancestor, so anything it consumes never reaches the card: the barrier therefore consumes
//! every key **except Esc and Enter**. Esc is the modal's own close request, answered by a
//! listener placed *after* the card subtree, so a control inside the card that consumed it
//! first — an open `Select` closing its list — wins over the close. Enter is deliberately
//! left to the surface's own handler, which should also sit after its controls — the Dialog
//! confirms on it; a surface with no confirm should consume it there instead. Everything
//! below the modal in document order still sees nothing, because whichever listener consumed
//! the key did so before the features' listeners run.
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
            // The barrier only: Esc and Enter pass this ancestor untouched — each is
            // answered *after* the card in document order (Esc by the trailing listener
            // below, Enter by the surface's own), so a control inside the card that
            // consumed the key first wins. An open `Select` closing its list on Esc is the
            // case this ordering exists for: consumed there, the close request never fires
            // and the surface keeps its state.
            .on_global_key_down({
                let barrier = self.barrier;
                move |e: Event<KeyboardEventData>| {
                    if !matches!(&e.key, Key::Named(NamedKey::Escape | NamedKey::Enter)) && barrier
                    {
                        // Consumed — that is what makes a modal surface modal.
                        e.prevent_default();
                    }
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
            // The close request, after the whole card subtree in pre-order — see the
            // barrier's note. Consumed in both barrier modes: Esc is the modal's own.
            .child(
                rect().on_global_key_down(move |e: Event<KeyboardEventData>| {
                    if matches!(&e.key, Key::Named(NamedKey::Escape)) {
                        if let Some(close) = &close {
                            close.call(());
                        }
                        e.prevent_default();
                    }
                }),
            )
    }
}
