//! The close-while-running confirm (T2), built to the Strata canvas's
//! close-confirmation comp: a 420px elevated card — warning chip + title + name header,
//! body copy, a "Don't ask again" checkbox writing `confirm_close_running` — over a
//! footer strip with a ghost keep button and the red stop action. All copy is the
//! canvas's, per variant (`closeConfirmTitle`/`Body`/`Keep`/`Btn`). The close
//! *mechanics* (the winit `on_close` bridge, `CloseGuard`, `TabCloser`) live in
//! `crate::apps::project::close`.

use freya::components::{get_theme, use_theme};
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station};
use strata_core::config::Settings;

use crate::apps::project::close::CloseTarget;
use crate::apps::project::state::{Chan, ProjChan, ProjectState, SessionState};
use crate::apps::project::views::{CancelButtonThemePartial, CancelButtonThemePreference};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Control, Prose, Title};

/// Mounted right after `ContextMenuViewer` at the window root: while open, its key
/// handler precedes every feature listener in document order and consumes every press —
/// Esc = keep (canvas `_onKey`), Enter = stop, everything else is the modal barrier.
/// So a ⌘W under the dialog can't close the very tab being confirmed, and Esc never
/// falls through to "cancel the query".
#[derive(PartialEq)]
pub struct CloseConfirm {
    pub confirm: State<Option<CloseTarget>>,
}

impl Component for CloseConfirm {
    fn render(&self) -> impl IntoElement {
        let mut confirm = self.confirm;
        let target = *confirm.read();
        let mut radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let project = use_radio_station::<ProjectState, ProjChan>();
        let mut settings = use_consume::<State<Settings>>();
        let theme = use_theme();
        // The action wears the shared `cancel_button` dress (the running body's Cancel)
        // — the themes' authored stop-the-query tone, not a hardcoded red.
        let cancel = get_theme!(
            &None::<CancelButtonThemePartial>,
            CancelButtonThemePreference,
            "cancel_button"
        );

        // Stop & close / Stop & exit — shared by the button and the Enter key. The
        // rebinds keep the closure `Fn` + `Copy` (both handles are `Copy`), so it can
        // live in both handlers.
        let close_anyway = move || {
            let mut radio = radio;
            let mut confirm = confirm;
            match *confirm.peek() {
                Some(CloseTarget::Tab(id)) => {
                    // The root's tab-diff funnel cancels/retires the tab's engine state.
                    radio.write().close_one(id);
                    confirm.set(None);
                }
                Some(CloseTarget::Window) => {
                    // Bypasses the on_close veto — this *is* the confirmed close.
                    Platform::get().close_current_window();
                }
                None => {}
            }
        };

        let Some(target) = target else {
            return rect().into_element();
        };

        // The canvas copy, per variant (`ccIsProject`).
        let is_project = matches!(target, CloseTarget::Window);
        let (title, body, keep, action, action_icon) = if is_project {
            (
                "Confirm exit",
                "Queries are running. Are you sure you want to stop them and exit?",
                "Cancel",
                "Stop & exit",
                IconName::LogOut,
            )
        } else {
            (
                "Confirm close",
                "A query is running. Are you sure you want to stop it and close this tab?",
                "Keep tab open",
                "Stop & close",
                IconName::Stop,
            )
        };
        let name = match target {
            CloseTarget::Tab(id) => radio
                .read()
                .tabs
                .get(&id)
                .map(|t| t.name.clone())
                .unwrap_or_default(),
            CloseTarget::Window => project.read().name.clone(),
        };

        let c = theme.read().colors.clone();
        // The comp's warning chip: the warning tone at 14% over the card.
        let chip_bg = c.warning.with_a(36);

        // Checked = don't ask = the `confirm_close_running` setting off. Toggling writes
        // the reactive global (the close guard mirrors it immediately) and persists —
        // the comp's checkbox edits the setting directly, not a local draft.
        let dont_ask = !settings.read().confirm_close_running;
        let toggle_dont_ask = move |_: Event<PressEventData>| {
            let now = !settings.peek().confirm_close_running;
            settings.write().confirm_close_running = now;
            let mut cfg = strata_core::config::load();
            cfg.settings = settings.peek().clone();
            strata_core::config::save(&cfg);
        };

        let header = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(12.)
            .child(
                rect()
                    .width(Size::px(34.))
                    .height(Size::px(34.))
                    .corner_radius(8.)
                    .background(chip_bg)
                    .main_align(Alignment::Center)
                    .cross_align(Alignment::Center)
                    .child(Icon::new(IconName::Warning).color(c.warning).size(18.)),
            )
            .child(
                rect()
                    .vertical()
                    .child(Title::new(title).color(c.text_primary))
                    .child(
                        Prose::new(name)
                            .color(c.text_placeholder)
                            .text_overflow(TextOverflow::Ellipsis),
                    ),
            );

        let checkbox_row = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding((4., 8.))
            .corner_radius(8.)
            .on_press(toggle_dont_ask)
            .child(Checkbox::new().selected(dont_ask).size(16.))
            .child(Prose::new("Don't ask again").color(c.text_placeholder));

        let footer = rect()
            .width(Size::fill())
            .horizontal()
            .main_align(Alignment::End)
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding((12., 24.))
            .background(c.surface_secondary)
            .child(
                Button::new()
                    .flat()
                    .on_press(move |_| confirm.set(None))
                    .child(Control::new(keep)),
            )
            .child(
                Button::new()
                    .filled()
                    .theme_colors(
                        ButtonColorsThemePartial::default()
                            .background(cancel.background)
                            .hover_background(cancel.hover_background)
                            .border_fill(cancel.border_fill)
                            .hover_border_fill(cancel.border_fill)
                            .color(cancel.color)
                            .hover_color(cancel.color),
                    )
                    .on_press(move |_| close_anyway())
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Icon::new(action_icon).size(13.))
                            .child(Control::new(action)),
                    ),
            );

        let card = rect()
            .width(Size::px(420.))
            .corner_radius(14.)
            .background(c.surface_tertiary)
            .border(Border::new().width(1.).fill(c.border))
            .shadow(Shadow::new().y(30.).blur(80.).color(c.shadow))
            .overflow(Overflow::Clip)
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(12.)
                    .padding((24., 24., 16., 24.))
                    .child(header)
                    .child(Prose::new(body).color(c.text_secondary).wrap())
                    .child(checkbox_row),
            )
            .child(Divider::horizontal().color(c.border))
            .child(footer);

        rect()
            // The overlay layer + global position lift the whole dialog above the
            // window content (the same wrapper `Popup` puts around `PopupBackground`).
            .layer(Layer::Overlay)
            .position(Position::new_global())
            // The modal barrier + the canvas's keys: Esc keeps, Enter stops, everything
            // else is consumed before the feature listeners deeper in document order.
            .on_global_key_down(move |e: Event<KeyboardEventData>| {
                match &e.key {
                    Key::Named(NamedKey::Escape) => confirm.set(None),
                    Key::Named(NamedKey::Enter) => close_anyway(),
                    _ => {}
                }
                e.prevent_default();
            })
            // A backdrop press keeps working (canvas `onCloseConfirmBackdrop`).
            .child(PopupBackground::new(
                card.into(),
                move |_| confirm.set(None),
                c.overlay,
            ))
            .into_element()
    }
}
