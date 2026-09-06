//! Modal backdrop, keyboard boundary, and focus restoration for an overlay surface.

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
        let focus = use_a11y();
        let previous = use_hook(|| *Platform::get().focused_accessibility_id.peek());
        use_drop(move || previous.request_focus());
        let close = self.on_close_request.clone();
        let backdrop_close = self.on_close_request.clone();

        rect()
            .a11y_id(focus)
            .a11y_auto_focus(true)
            .a11y_modal(self.barrier)
            .a11y_role(AccessibilityRole::Dialog)
            .layer(Layer::Overlay)
            .position(Position::new_global())
            .on_global_key_down({
                let barrier = self.barrier;
                move |e: Event<KeyboardEventData>| {
                    if !matches!(&e.key, Key::Named(NamedKey::Escape | NamedKey::Enter)) && barrier
                    {
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
