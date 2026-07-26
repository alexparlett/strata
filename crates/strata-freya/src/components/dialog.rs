//! The **modal dialog shell** — every centred confirm in the app is **header · body · footer** on
//! this one card. Callers supply what differs: the header's icon, tone and title run; the body;
//! the buttons.
//!
//! The chrome it owns is the part that must not drift between dialogs (it already had):
//!
//! - **Card** — a fixed width, 14px radius on `surface_tertiary`, hairline border, drop shadow,
//!   clipped so the footer's fill meets the corners; the comps' `24 / 24 / 16 / 24` inset with 12
//!   between header and body.
//! - **Header** — a tinted icon chip beside a title over its subject ([`DialogHeader`]). One chip
//!   for every dialog; only the icon and the tone vary.
//! - **Body** — whatever the caller passes, full width *under* the header rather than indented
//!   beside the chip. That is the structural half of the fix: previously one dialog put its copy
//!   in a column next to the icon and the other ran it across the card.
//! - **Action strip** — a `surface_secondary` band under a hairline, buttons end-aligned, **58px
//!   tall**: `--sp-4` (12) above and below a [`ACTION_HEIGHT`] button row. The strip stamps that
//!   height onto its own actions, so a dialog physically cannot ship a squashed button — and the
//!   number itself belongs to the design system (`components::ACTION_HEIGHT`), not to this
//!   component, because every committing button in the app wears it.
//! - **Modal semantics** — Esc dismisses, Enter confirms, and every other key is consumed *at the
//!   global layer*. That barrier is why dialogs mount early at the window root: same-name global
//!   listeners fire in document order, so a dialog above the features swallows keystrokes meant
//!   for it before ⌘W or Esc-to-cancel-the-query can act on them.
//!
//!   **It is not yet focus containment.** `KeyDown` (priority 4) outranks `GlobalKeyDown` (5) and
//!   its cancel set includes `GlobalKeyDown`, so a *focused* element still sees the key first and
//!   can cancel the dialog's handler outright — the SQL editor's `on_key_down` does exactly that
//!   on several branches. Nothing here moves focus into the card. So with the editor focused,
//!   keystrokes reach the buffer under the scrim. Fixing it properly means focusing the dialog on
//!   open and restoring focus on dismiss (Freya's own `Popup` sets `a11y_role`/auto-focus); until
//!   then, treat the barrier as covering global listeners only.
//!
//! Dismiss and confirm are `EventHandler<()>` rather than the usual `Event<T>` props, because they
//! are *outcomes*, not events: dismiss arrives from Esc **or** the backdrop, and confirm from Enter
//! **or** the caller's own button. Freya types its own semantic actions the same way
//! (`Popup::on_close_request`, `Menu::on_close`).
//!
//! Mount it only while the dialog is open — it renders no "closed" state of its own.

use freya::components::{use_theme, PopupBackground};
use freya::prelude::*;

use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::ACTION_HEIGHT;

/// The comps' card width — 420 for every confirm in the design.
const DEFAULT_WIDTH: f32 = 420.;

/// The header chip's box and its glyph. One size for every dialog — see the module doc.
const CHIP: f32 = 38.;
const CHIP_RADIUS: f32 = 8.;
const CHIP_ICON: f32 = 19.;
/// Alpha of the chip's fill, tinted from the dialog's tone (≈13%, the comps' figure).
const CHIP_TINT: u8 = 33;

/// A dialog's header: a `tone`-tinted chip carrying `icon`, beside the title run.
///
/// The tone is the dialog's *character* and the only thing that varies — `warning` for a question
/// about work in flight, `error` for a destructive one — and it colours the glyph and its fill
/// together, so a dialog can't end up with a red icon in an amber chip.
#[derive(PartialEq)]
pub struct DialogHeader {
    icon: IconName,
    tone: Color,
    child: Element,
}

impl DialogHeader {
    /// `child` is the title run: a single `Title`, a stacked title + subtitle, or a `paragraph()`
    /// of mixed spans — whatever the comp calls for. It is given the width beside the chip.
    pub fn new(icon: IconName, tone: Color, child: impl IntoElement) -> Self {
        Self {
            icon,
            tone,
            child: child.into_element(),
        }
    }
}

impl Component for DialogHeader {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(12.)
            .child(
                rect()
                    .width(Size::px(CHIP))
                    .height(Size::px(CHIP))
                    .corner_radius(CHIP_RADIUS)
                    .background(self.tone.with_a(CHIP_TINT))
                    .main_align(Alignment::Center)
                    .cross_align(Alignment::Center)
                    .child(Icon::new(self.icon).color(self.tone).size(CHIP_ICON)),
            )
            .child(
                rect()
                    .width(Size::flex(1.))
                    .vertical()
                    .child(self.child.clone()),
            )
    }
}

#[derive(PartialEq)]
pub struct Dialog {
    header: Option<Element>,
    body: Element,
    actions: Vec<Button>,
    on_dismiss: Option<EventHandler<()>>,
    on_confirm: Option<EventHandler<()>>,
}

impl Default for Dialog {
    fn default() -> Self {
        Self::new()
    }
}

impl Dialog {
    pub fn new() -> Self {
        Self {
            header: None,
            body: rect().into_element(),
            actions: Vec::new(),
            on_dismiss: None,
            on_confirm: None,
        }
    }

    /// The chip-and-title row above the body — normally a [`DialogHeader`].
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_element());
        self
    }

    /// The card body — full width under the header, already inset by the card's padding. Pass a
    /// `rect()` with whatever direction and spacing the comp calls for.
    ///
    /// Named `body`, not `child`: it **replaces** rather than appends, so borrowing the builder
    /// spelling would make `Dialog::new().child(a).child(b)` silently drop `a`.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = body.into_element();
        self
    }

    /// Append a button to the action strip, left to right (so the confirming action goes last —
    /// it ends up nearest the corner).
    ///
    /// Takes the `Button` itself, not an `Element`, so the strip can size it: pass the variant
    /// (`.flat()` / `.outline()` / `.filled()`), its colours and its handler, and leave the box to
    /// the dialog.
    pub fn action(mut self, action: Button) -> Self {
        self.actions.push(action);
        self
    }

    /// Esc, or a press on the backdrop.
    pub fn on_dismiss(mut self, on_dismiss: impl Into<EventHandler<()>>) -> Self {
        self.on_dismiss = Some(on_dismiss.into());
        self
    }

    /// Enter. Omit it and Enter is merely swallowed by the barrier, which is right for a dialog
    /// with no single obvious action.
    pub fn on_confirm(mut self, on_confirm: impl Into<EventHandler<()>>) -> Self {
        self.on_confirm = Some(on_confirm.into());
        self
    }
}

impl Component for Dialog {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let c = theme.read().colors().clone();

        let dismiss = self.on_dismiss.clone();
        let backdrop_dismiss = self.on_dismiss.clone();
        let confirm = self.on_confirm.clone();

        // A message-only dialog has no strip — and so no hairline either, otherwise the card ends
        // in a rule under an empty 58px band.
        let strip = (!self.actions.is_empty()).then(|| {
            rect()
                .width(Size::fill())
                .horizontal()
                .main_align(Alignment::End)
                .cross_align(Alignment::Center)
                .spacing(8.)
                .padding((12., 24.))
                .background(c.surface_secondary)
                .children(self.actions.iter().map(|action| {
                    // The design system's action height, layered over whatever layout theme the
                    // action arrived with, so a variant's padding and radius still apply. A
                    // caller who set a height deliberately keeps it — the stamp is a default,
                    // not an override, or `.compact()` would stop meaning what it means
                    // everywhere else.
                    let layout = action.get_theme_layout().cloned().unwrap_or_default();
                    let layout = match layout.height {
                        Some(_) => layout,
                        None => layout.height(Size::px(ACTION_HEIGHT)),
                    };
                    action.clone().theme_layout(layout).into_element()
                }))
        });

        let card = rect()
            .width(Size::px(DEFAULT_WIDTH))
            // Never wider than the window on a small screen.
            .max_width(Size::window_percent(92.))
            .corner_radius(14.)
            .background(c.surface_tertiary)
            .border(Border::new().width(1.).fill(c.border))
            .shadow(Shadow::new().y(30.).blur(80.).color(c.shadow))
            .overflow(Overflow::Clip)
            // Announced as a dialog rather than an anonymous group, like Freya's own `Popup`.
            .a11y_role(AccessibilityRole::Dialog)
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(12.)
                    .padding((24., 24., 16., 24.))
                    .maybe_child(self.header.clone())
                    .child(self.body.clone()),
            )
            .maybe_child(
                strip
                    .as_ref()
                    .map(|_| Divider::horizontal().color(c.border).into_element()),
            )
            .maybe_child(strip);

        rect()
            // The overlay layer + global position lift the whole dialog above the window content
            // (the same wrapper `Popup` puts around `PopupBackground`).
            .layer(Layer::Overlay)
            .position(Position::new_global())
            .on_global_key_down(move |e: Event<KeyboardEventData>| {
                match &e.key {
                    Key::Named(NamedKey::Escape) => {
                        if let Some(dismiss) = &dismiss {
                            dismiss.call(());
                        }
                    }
                    Key::Named(NamedKey::Enter) => {
                        if let Some(confirm) = &confirm {
                            confirm.call(());
                        }
                    }
                    _ => {}
                }
                // Consumed either way — that is what makes this modal.
                e.prevent_default();
            })
            .child(PopupBackground::new(
                card.into(),
                move |_| {
                    if let Some(dismiss) = &backdrop_dismiss {
                        dismiss.call(());
                    }
                },
                c.overlay,
            ))
    }
}
