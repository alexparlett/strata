//! The results pane while a query executes — the comp's running state: the accent
//! spinner over "Running query…", a live mono elapsed readout, and the error-tinted
//! Cancel control ("Cancel · Esc"; Esc cancels globally while the run is up). Cancel is
//! the caller's action — the body only reports the press (see `ResultsBody`'s wiring:
//! engine-side abort + clearing the Run trigger back to the empty state).

use std::time::{Duration, Instant};

use async_io::Timer;
use freya::components::CircularLoader;
use freya::prelude::*;

use crate::state::use_config_station;
use strata_core::config::Command;

use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_2, SP_3, SP_4, SP_5};
use crate::components::typography::{Body, Control, Path};
use crate::keymap::{on_command, use_hint};
use crate::theme::{use_roles, Role};

define_theme!(
    %[no_ext]
    %[component]
    pub CancelButton {
        %[fields]
        background: Color,
        hover_background: Color,
        border_fill: Color,
        color: Color,
    }
);

/// A run's elapsed time in the readout's dress: tenths under a minute, `Nm SSs` past it.
fn fmt_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{:.1}s", elapsed.as_secs_f64())
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// The results pane while a query executes.
#[derive(PartialEq)]
pub struct Running {
    on_cancel: EventHandler<()>,
    theme: Option<CancelButtonThemePartial>,
}

impl Running {
    pub fn new(on_cancel: impl Into<EventHandler<()>>) -> Self {
        Self {
            on_cancel: on_cancel.into(),
            theme: None,
        }
    }
}

impl Component for Running {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        let (title_color, sub_color, background) = (
            roles.get(Role::TextMuted),
            roles.get(Role::TextPlaceholder),
            roles.get(Role::SurfaceRaised),
        );
        let cancel = get_theme!(&self.theme, CancelButtonThemePreference, "cancel_button");

        let mut elapsed = use_state(Duration::default);
        use_hook(move || {
            let start = Instant::now();
            spawn(async move {
                loop {
                    Timer::after(Duration::from_millis(100)).await;
                    elapsed.set(start.elapsed());
                }
            });
        });

        let mut hovered = use_state(|| false);
        let on_cancel = self.on_cancel.clone();
        let on_esc = on_cancel.clone();
        let config = use_config_station();
        let esc_hint = use_hint(Command::Cancel);

        rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .spacing(SP_5)
            .background(background)
            .on_global_key_down(on_command(config, Command::Cancel, move || {
                on_esc.call(());
                true
            }))
            .child(CircularLoader::new().size(30.))
            .child(Body::new("Running query…").color(title_color))
            .child(Path::new(fmt_elapsed(elapsed())).color(sub_color))
            .child(
                rect()
                    .height(Size::px(30.))
                    .padding((0., SP_4))
                    .corner_radius(R_2)
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
                    .background(if hovered() {
                        cancel.hover_background
                    } else {
                        cancel.background
                    })
                    .border(Border::new().width(1.).fill(cancel.border_fill))
                    .on_pointer_enter(move |_| hovered.set(true))
                    .on_pointer_leave(move |_| hovered.set(false))
                    .on_press(move |_| on_cancel.call(()))
                    .child(Icon::new(IconName::Stop).color(cancel.color).size(12.))
                    .child(Control::new(format!("Cancel · {esc_hint}")).color(cancel.color)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_reads_as_tenths_then_minutes() {
        assert_eq!(fmt_elapsed(Duration::from_millis(0)), "0.0s");
        assert_eq!(fmt_elapsed(Duration::from_millis(2340)), "2.3s");
        assert_eq!(fmt_elapsed(Duration::from_secs(59)), "59.0s");
        assert_eq!(fmt_elapsed(Duration::from_secs(63)), "1m 03s");
        assert_eq!(fmt_elapsed(Duration::from_secs(600)), "10m 00s");
    }
}
