//! The window's **Cancel / Apply** footer.
//!
//! Apply is the only thing in the window that writes anything: it commits the draft to the
//! app-global config, which publishes it to every window and persists it, and closes. Cancel
//! just closes — nothing was committed, so there is nothing to undo but the live theme
//! preview, which the root drops on the way out.
//!
//! Apply is disabled while the draft matches the seed it was made from, so the button says
//! whether there is anything to apply rather than being a no-op that closes the window.

use freya::prelude::*;

use crate::apps::settings::{settings_theme, SettingsCtx};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::tones::tones;
use crate::components::typography::Control;

/// The strip's inset (canvas `padding: var(--sp-4) var(--sp-5)`) and its buttons' height.
const FOOTER_PADDING: Gaps = Gaps::new(12., 16., 12., 16.);
const BUTTON_HEIGHT: f32 = 34.;

#[derive(PartialEq)]
pub struct Footer;

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let ctx = use_consume::<SettingsCtx>();
        let platform = use_hook(Platform::get);
        let dirty = ctx.dirty();
        // Why Apply is off while the draft *is* dirty. Said out loud, because a button that is
        // disabled for a reason the user cannot see reads as a broken button.
        let blocker = ctx.blocker();
        // The same strip states a failed Apply (P4-15), and the blocker wins it — a blocker is a
        // live reason the press won't run, where a failure describes one that already did.
        //
        // Deliberately a *second* binding rather than folding the failure into `blocker`: that
        // value is also the enable gate below, so a failure folded into it would disable Apply
        // the instant it was reported — the retry the open window exists to offer, taken away by
        // the message offering it. Only a blocker may disable the button.
        let message = blocker.clone().or_else(|| ctx.failure());
        let error = tones().error;

        let cancel = {
            let platform = platform.clone();
            Button::new()
                .height(Size::px(BUTTON_HEIGHT))
                .on_press(move |_: Event<PressEventData>| platform.close_current_window())
                .child(Control::new("Cancel"))
        };
        // **Off while one is already running.** `apply` puts the keystore's blocking half on a
        // worker so the window stays live — and a live window is a pressable one, so without this
        // a second press would run a concurrent `commit` over the same typed keys, both seeing no
        // marker and each minting one for the same secret.
        //
        // Not folded into `blocker`: that value is also the strip's message, and "an Apply is
        // running" is not a reason the press *won't* work, which is what a blocker states.
        let applying = ctx.applying();
        let apply = Button::new()
            .filled()
            .height(Size::px(BUTTON_HEIGHT))
            .enabled(dirty && blocker.is_none() && !applying)
            .on_press(move |_: Event<PressEventData>| {
                let platform = platform.clone();
                spawn(async move {
                    // Closing on a failed write would look exactly like success — the settings
                    // are live in every window either way; only the durable copy is missing.
                    if ctx.apply().await {
                        platform.close_current_window();
                    }
                });
            })
            .child(Control::new("Apply"));

        rect()
            .width(Size::fill())
            .vertical()
            .child(Divider::horizontal().color(theme.border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(12.)
                    .padding(FOOTER_PADDING)
                    .background(theme.background)
                    .child(
                        rect()
                            .width(Size::flex(1.))
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(6.)
                            .maybe_child(message.map(|message| {
                                rect()
                                    .horizontal()
                                    .cross_align(Alignment::Center)
                                    .spacing(6.)
                                    .child(Icon::new(IconName::Alert).size(14.).color(error))
                                    .child(Control::new(message).color(error))
                            })),
                    )
                    .child(cancel)
                    .child(apply),
            )
    }
}
