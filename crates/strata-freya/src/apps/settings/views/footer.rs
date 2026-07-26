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

use crate::apps::settings::{SettingsCtx, SettingsThemePartial, SettingsThemePreference};
use crate::components::divider::Divider;
use crate::components::typography::Control;

/// The strip's inset (canvas `padding: var(--sp-4) var(--sp-5)`) and its buttons' height.
const FOOTER_PADDING: Gaps = Gaps::new(12., 16., 12., 16.);
const BUTTON_HEIGHT: f32 = 34.;

#[derive(PartialEq)]
pub struct Footer;

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );
        let ctx = use_consume::<SettingsCtx>();
        let platform = use_hook(Platform::get);
        let dirty = ctx.dirty();

        let cancel = {
            let platform = platform.clone();
            Button::new()
                .height(Size::px(BUTTON_HEIGHT))
                .on_press(move |_: Event<PressEventData>| platform.close_current_window())
                .child(Control::new("Cancel"))
        };
        let apply = Button::new()
            .filled()
            .height(Size::px(BUTTON_HEIGHT))
            .enabled(dirty)
            .on_press(move |_: Event<PressEventData>| {
                ctx.apply();
                platform.close_current_window();
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
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::End)
                    .spacing(12.)
                    .padding(FOOTER_PADDING)
                    .background(theme.background)
                    .child(cancel)
                    .child(apply),
            )
    }
}
