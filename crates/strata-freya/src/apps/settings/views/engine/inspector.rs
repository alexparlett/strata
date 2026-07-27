//! The **selection inspector** — what the catalogue knows about the selected property.
//!
//! Only real facts, the same rule the column inspector holds (P3-08): the description and the
//! default come from [`ENGINE_KEYS`](strata_core::engine::config::ENGINE_KEYS), so a custom key
//! gets the one sentence that is true of it — that nothing here recognises it — rather than a
//! `Default: —` row pretending the catalogue had an answer.
//!
//! Absent for a row with no name yet, because there is nothing to say about it that its own empty
//! box does not already say.

use freya::prelude::*;

use crate::apps::settings::views::engine::model::PropRows;
use crate::apps::settings::{SettingsTheme, SettingsThemePartial, SettingsThemePreference};
use crate::components::badge::Badge;
use crate::components::typography::{Meta, Prose, Strong};

/// The strip's inset (canvas `padding: var(--sp-4) var(--sp-5)`) and the gap above it.
const INSET: Gaps = Gaps::new(12., 16., 12., 16.);
const GAP_ABOVE: f32 = 12.;
/// The gaps within it (canvas `var(--sp-3)` between the title's pills, `var(--sp-2)` between rows).
const PILL_GAP: f32 = 8.;
const ROW_GAP: f32 = 4.;

/// What a custom key can honestly be told about itself.
const CUSTOM: &str =
    "Custom property. Not a recognised DataFusion option, so the engine may decline it.";

#[derive(PartialEq)]
pub struct Inspector {
    pub rows: State<PropRows>,
}

impl Component for Inspector {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let warning = use_theme().read().colors().warning;

        let list = self.rows.read();
        let Some(row) = list.selected_row().filter(|row| !row.key().is_empty()) else {
            return rect();
        };
        let def = row.def();
        let restart = strata_core::engine::config::is_restart_key(row.key());

        rect()
            .width(Size::fill())
            .vertical()
            .margin(Gaps::new(GAP_ABOVE, 0., 0., 0.))
            .padding(INSET)
            .spacing(ROW_GAP)
            .background(theme.card_background)
            .corner_radius(CornerRadius::new_all(6.))
            .border(
                Border::new()
                    .fill(theme.card_border_fill)
                    .width(1.)
                    .alignment(BorderAlignment::Inner),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(PILL_GAP)
                    .child(Strong::new(row.key()).color(theme.item_active_color))
                    .maybe_child(restart.then(|| Badge::tag("RESTART", warning)))
                    .maybe_child(
                        def.is_none()
                            .then(|| Badge::tag("CUSTOM", theme.hint_color).outlined()),
                    ),
            )
            .child(
                Prose::new(def.map_or(CUSTOM, |def| def.desc))
                    .color(theme.hint_color)
                    .wrap(),
            )
            .maybe_child(def.map(|def| {
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(4.)
                    .child(Meta::new("Default:").color(theme.chevron_color))
                    .child(
                        Meta::new(match def.default {
                            "" => "(empty)",
                            default => default,
                        })
                        .color(theme.hint_color),
                    )
            }))
    }
}

fn settings_theme() -> SettingsTheme {
    get_theme!(
        &None::<SettingsThemePartial>,
        SettingsThemePreference,
        "settings"
    )
}
