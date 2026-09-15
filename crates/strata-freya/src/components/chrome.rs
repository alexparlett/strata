//! **The window buttons, where they are ours to draw** — minimize, maximize/restore and close, for
//! every platform whose frame `platform::window_attributes` takes off.
//!
//! On macOS this renders nothing at all. AppKit's traffic lights are still real buttons, inset into
//! our strip by the fork's `with_traffic_light_inset`, and
//! [`TRAFFIC_LIGHT_GUTTER`](super::metrics::TRAFFIC_LIGHT_GUTTER) is the room they sit in. Anywhere
//! else there is no such thing as a half-decorated window — the WM either draws a frame or it does
//! not — so the decoration comes off outright and these three buttons are the replacement.
//!
//! **Trailing edge, not leading.** macOS puts its buttons top-left; Windows and every mainstream
//! Linux desktop put them top-right. Following each platform is the whole point of drawing them
//! ourselves, so the gutter is two metrics rather than one:
//! [`TRAFFIC_LIGHT_GUTTER`](super::metrics::TRAFFIC_LIGHT_GUTTER) reserves at the leading edge for
//! AppKit and [`WINDOW_CONTROLS_GUTTER`](super::metrics::WINDOW_CONTROLS_GUTTER) at the trailing
//! edge for these, and exactly one of the two is non-zero on any given platform.
//!
//! **Absolutely positioned, flush to the corner.** The buttons are pinned to their bar's top-right
//! rather than laid out in its flow, for two reasons: flush-to-the-corner is what those platforms
//! do (there is no inset between the close button and the window edge), and a bar whose content is
//! a flex row — the project header, with its spacer and its trailing cluster — would otherwise have
//! to grow a slot for them. The bar reserves the room in its padding; these float in it.
//!
//! **The buttons themselves are the fork's**, not ours. `TitlebarButton` carries the glyphs and
//! knows which of its actions is destructive; this module supplies placement, Strata's theme roles
//! and what each press does. Its theme grew `color`, `close_hover_background` and
//! `close_hover_color` for this — a titlebar whose close button highlights like minimize is one
//! people mis-click, and that belonged in the component rather than in a copy of it here.
//!
//! Stopping a press from also dragging the window is [`TitlebarButton`]'s own job now — a titlebar
//! button is inside a drag region by definition, so every caller needed the same guard.
//!
//! **Close goes through the veto, never straight to the window.** [`request_close`] queues the same
//! `CloseRequested` the OS close button and ⌘Q raise, so a window holding itself open — a project
//! with a query running (T2), an unsaved Configure form — raises its confirm instead of being torn
//! down under it. `close_current_window` would have made our button the one press in the app that
//! skips every guard.
//!
//! **Maximize is the title bar's fill, not fullscreen**, matching the double-press the bars already
//! carry: `set_maximized`, which the WM honours, and which `use_maximized` reads back so the glyph
//! can say *restore* while the window is filled. Native fullscreen stays unbound, as it is on macOS
//! where it belongs to the green button.

use freya::borderless::use_maximized;
use freya::prelude::*;

use super::metrics::WINDOW_CONTROL;
use crate::platform::request_close;
use crate::theme::{use_roles, Role};

/// The three window buttons, or nothing on macOS.
///
/// `height` is the host bar's own height — the buttons fill it and are [`WINDOW_CONTROL`] wide, so
/// one component serves the launcher's 38px strip, the project header's 48px and the child
/// windows' 50px without each of them restating the arithmetic.
///
/// **The buttons are the fork's [`TitlebarButton`]**, themed rather than redrawn. It already knows
/// which of its four actions is the destructive one and carries the glyphs; what this adds is where
/// they sit, what they do, and Strata's roles over its theme — which is the whole of what an app
/// should be adding to a framework component.
#[derive(PartialEq)]
pub struct WindowControls {
    pub height: f32,
}

impl WindowControls {
    pub fn new(height: f32) -> Self {
        Self { height }
    }
}

impl Component for WindowControls {
    fn render(&self) -> impl IntoElement {
        // macOS keeps AppKit's buttons, so there is nothing to draw. An early return rather than a
        // `#[cfg]` on the module: the component stays one type on every platform, so no title bar
        // needs a `cfg` of its own to place it.
        if cfg!(target_os = "macos") {
            return rect().width(Size::px(0.)).height(Size::px(0.));
        }

        let roles = use_roles();
        let filled = use_maximized();

        // The fork's default is Windows' 46px-wide, full-height button and a palette this app does
        // not use. Strata's own roles, and a square that fits a 38px strip.
        let theme = TitlebarButtonThemePartial::default()
            .background(Color::TRANSPARENT)
            .hover_background(roles.get(Role::ElementHover))
            .color(roles.get(Role::TextControl))
            .close_hover_color(roles.get(Role::TextOnAccent))
            .width(Size::px(WINDOW_CONTROL))
            .height(Size::fill());

        let button = |action: TitlebarAction| {
            TitlebarButton::new(action)
                .theme(theme.clone())
                .on_press(move |_| match action {
                    TitlebarAction::Minimize => Platform::get().with_window(None, |window| {
                        window.set_minimized(true);
                    }),
                    TitlebarAction::Maximize | TitlebarAction::Restore => {
                        let filling = !filled();
                        Platform::get()
                            .with_window(None, move |window| window.set_maximized(filling));
                    }
                    TitlebarAction::Close => request_close(),
                })
        };

        rect()
            .position(Position::new_absolute().top(0.).right(0.))
            .height(Size::px(self.height))
            .horizontal()
            .child(button(TitlebarAction::Minimize))
            .child(button(if filled() {
                TitlebarAction::Restore
            } else {
                TitlebarAction::Maximize
            }))
            .child(button(TitlebarAction::Close))
    }
}
