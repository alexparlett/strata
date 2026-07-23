//! The results pane while a query executes — the comp's running state: the accent
//! spinner over "Running query…", a live mono elapsed readout, and the error-tinted
//! Cancel control ("Cancel · Esc"; Esc cancels globally while the run is up). Cancel is
//! the caller's action — the body only reports the press (see `ResultsBody`'s wiring:
//! engine-side abort + clearing the Run trigger back to the empty state).

use std::time::{Duration, Instant};

use async_io::Timer;
use freya::components::{use_theme, CircularLoader};
use freya::prelude::*;

use strata_core::config::{Command, Settings};

use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Body, Control, Path};

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
        Self { on_cancel: on_cancel.into(), theme: None }
    }
}

impl Component for Running {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let (title_color, sub_color, background) = {
            let c = &theme.read().colors;
            (c.text_secondary, c.text_placeholder, c.surface_secondary)
        };
        // The Cancel control's own theme component — authored per theme in the file's
        // `components.cancel_button`, whose values track `run_button`'s `running_*` set (the
        // same cancel dress the toolbar's Run→Cancel flip wears, P2-15).
        let cancel = get_theme!(&self.theme, CancelButtonThemePreference, "cancel_button");

        // Ticks from mount — the body is keyed on the press's nonce, so a new Run always
        // restarts from zero. The ticker task is scope-bound: settling unmounts it.
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
        let settings = use_consume::<State<Settings>>();
        // Derived even though Cancel is fixed — one source for every glyph.
        let esc_hint = crate::keymap::use_hint(Command::Cancel);

        rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .spacing(16.)
            .background(background)
            // Esc = Cancel while the run is up. This body sits after the tab strip in
            // document order, so an open menu or an in-progress rename claims the Esc
            // first; when it reaches here it's consumed.
            .on_global_key_down(crate::keymap::on_command(settings, Command::Cancel, move || {
                on_esc.call(());
                true
            }))
            .child(CircularLoader::new().size(30.))
            .child(Body::new("Running query…").color(title_color))
            .child(Path::new(fmt_elapsed(elapsed())).color(sub_color))
            .child(
                rect()
                    .height(Size::px(30.))
                    .padding((0., 12.))
                    .corner_radius(8.)
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(8.)
                    .background(if hovered() { cancel.hover_background } else { cancel.background })
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
