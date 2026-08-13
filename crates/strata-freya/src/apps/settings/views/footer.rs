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
use crate::components::metrics::{ACTION_HEIGHT, SP_3, SP_4, SP_5};
use crate::components::tones::tones;
use crate::components::typography::Control;

/// The strip's inset (canvas `padding: var(--sp-4) var(--sp-5)`); its buttons take the design
/// system's own [`ACTION_HEIGHT`], like every other committing pair.
const FOOTER_PADDING: Gaps = Gaps::new(SP_4, SP_5, SP_4, SP_5);

#[derive(PartialEq)]
pub struct Footer;

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let ctx = use_consume::<SettingsCtx>();
        let platform = use_hook(Platform::get);
        let dirty = ctx.dirty();
        let blocker = ctx.blocker();
        let message = blocker.clone().or_else(|| ctx.failure());
        let error = tones().error;

        let cancel = {
            let platform = platform.clone();
            Button::new()
                .height(Size::px(ACTION_HEIGHT))
                .on_press(move |_: Event<PressEventData>| platform.close_current_window())
                .child(Control::new("Cancel"))
        };
        let applying = ctx.applying();
        let apply = Button::new()
            .filled()
            .height(Size::px(ACTION_HEIGHT))
            .enabled(dirty && blocker.is_none() && !applying)
            .on_press(move |_: Event<PressEventData>| {
                let platform = platform.clone();
                spawn(async move {
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
                    .spacing(SP_4)
                    .padding(FOOTER_PADDING)
                    .background(theme.background)
                    .child(
                        rect()
                            .width(Size::flex(1.))
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(SP_3)
                            .maybe_child(message.map(|message| {
                                rect()
                                    .horizontal()
                                    .cross_align(Alignment::Center)
                                    .spacing(SP_3)
                                    .child(Icon::new(IconName::Alert).size(14.).color(error))
                                    .child(Control::new(message).color(error))
                            })),
                    )
                    .child(cancel)
                    .child(apply),
            )
    }
}
