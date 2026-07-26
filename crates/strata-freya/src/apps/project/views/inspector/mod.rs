//! The right **column inspector** shell (P3-01) — the frame the facts box (P3-08) and profiling
//! (P3-09) grow into. It renders the "COLUMN INSPECTOR" header + a collapse (×) over
//! `surface_secondary`; the body is empty until its content task lands. Its left border is the
//! resize handle between it and the workbench, so the shell draws none. Reopening the inspector
//! is a column selection (P3-08); P3-01 only builds it open + closable.

use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::use_radio;

use crate::apps::project::state::{Chan, SessionState};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Eyebrow;

#[derive(PartialEq)]
pub struct Inspector;

impl Inspector {
    pub fn new() -> Self {
        Self
    }
}

impl Component for Inspector {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Layout);
        let theme = use_theme();
        let (bg, border, label_color) = {
            let t = theme.read();
            (
                t.colors().surface_secondary,
                t.colors().border,
                t.colors().text_placeholder,
            )
        };

        rect()
            .expanded()
            .background(bg)
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(40.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::SpaceBetween)
                    .padding((0., 12.))
                    .child(Eyebrow::new("COLUMN INSPECTOR").color(label_color))
                    .child(
                        Button::new()
                            .flat()
                            .width(Size::px(24.))
                            .height(Size::px(24.))
                            .on_press(move |_| {
                                let mut radio = radio;
                                radio.write_channel(Chan::Layout).close_inspector();
                            })
                            .child(Icon::new(IconName::Close).size(13.)),
                    ),
            )
            .child(Divider::horizontal().color(border))
            // Empty body — the facts box (P3-08) + profiling zone (P3-09) fill it.
            .child(rect().expanded())
    }
}
