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
//!   beside the chip.
//! - **Action strip** — a `surface_secondary` band under a hairline, buttons end-aligned, **58px
//!   tall**. The strip stamps [`ACTION_HEIGHT`] onto its own actions, so a dialog cannot ship a
//!   squashed button — and that number belongs to the design system, because every committing
//!   button in the app wears it.
//! - **Modal semantics** — Esc dismisses, Enter confirms, every other key is consumed at the global
//!   layer. The open/closed half is the shared [`Modal`] base, which this wraps its card in; Enter
//!   is the *dialog's* semantic and lives on the card, in the slot the base leaves open. The
//!   barrier is why dialogs mount early at the window root: same-name global listeners fire in
//!   document order.
//!
//!   **It is not yet focus containment.** `KeyDown` outranks `GlobalKeyDown` and its cancel set
//!   includes it, so a *focused* element sees the key first and can cancel the dialog's handler —
//!   the SQL editor does exactly that on several branches, and nothing here moves focus into the
//!   card. Fixing it properly means focusing the dialog on open and restoring focus on dismiss;
//!   until then the barrier covers global listeners only.
//!
//! Dismiss and confirm are `EventHandler<()>` rather than `Event<T>` because they are *outcomes*:
//! dismiss arrives from Esc **or** the backdrop, confirm from Enter **or** the caller's button.
//! Freya types its own semantic actions the same way.
//!
//! Mount it only while the dialog is open — it renders no "closed" state of its own.

use freya::prelude::*;

use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::ACTION_HEIGHT;
use crate::components::metrics::{R_2, R_4, SP_3, SP_4, SP_5, SP_6};
use crate::components::modal::Modal;
use crate::theme::{use_roles, Role};

/// The comps' card width — 420 for every confirm in the design.
const DEFAULT_WIDTH: f32 = 420.;

/// The header chip's box and its glyph. One size for every dialog — see the module doc.
const CHIP: f32 = 38.;
const CHIP_RADIUS: f32 = R_2;
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
            .spacing(SP_4)
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
    modal: bool,
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
            modal: true,
        }
    }

    /// Whether the dialog's key barrier consumes **every** key (the default, and right for
    /// a confirm raised over live features — see the module doc). Pass `false` for a dialog
    /// that *is* the window's whole content, with nothing behind it to protect: Esc and
    /// Enter keep their dialog meaning, and every other chord stays the window's — which is
    /// what keeps ⌘O and ⌘, alive on the project-load fault, whose menubar items arrive as
    /// synthesized key presses a barrier would swallow.
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
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
        let roles = use_roles();

        let dismiss = self.on_dismiss.clone();
        let confirm = self.on_confirm.clone();

        let strip = (!self.actions.is_empty()).then(|| {
            rect()
                .width(Size::fill())
                .horizontal()
                .main_align(Alignment::End)
                .cross_align(Alignment::Center)
                .spacing(SP_3)
                .padding((SP_4, SP_6))
                .background(roles.get(Role::SurfaceRaised))
                .children(self.actions.iter().map(|action| {
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
            .max_width(Size::window_percent(92.))
            .corner_radius(R_4)
            .background(roles.get(Role::ElevatedSurface))
            .border(Border::new().width(1.).fill(roles.get(Role::Border)))
            .shadow(
                Shadow::new()
                    .y(30.)
                    .blur(80.)
                    .color(roles.get(Role::Shadow)),
            )
            .overflow(Overflow::Clip)
            .a11y_role(AccessibilityRole::Dialog)
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(SP_4)
                    .padding((SP_6, SP_6, SP_5, SP_6))
                    .maybe_child(self.header.clone())
                    .child(self.body.clone()),
            )
            .maybe_child(strip.as_ref().map(|_| {
                Divider::horizontal()
                    .color(roles.get(Role::Border))
                    .into_element()
            }))
            .maybe_child(strip)
            .child(
                rect().on_global_key_down(move |e: Event<KeyboardEventData>| {
                    if matches!(&e.key, Key::Named(NamedKey::Enter)) {
                        if let Some(confirm) = &confirm {
                            confirm.call(());
                        }
                        e.prevent_default();
                    }
                }),
            );

        let mut modal = Modal::new(card).barrier(self.modal);
        if let Some(dismiss) = dismiss {
            modal = modal.on_close_request(dismiss);
        }
        modal
    }
}
