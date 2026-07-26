//! The FORMAT row — the four output formats as cards.
//!
//! Four, not five: the canvas dropped the Clipboard tile (2026-07-12) once the grid grew its
//! own copy controls, so "export" here always means a file on disk.
//!
//! Each card is its own press target rather than a [`SegmentedToggle`]: they carry a glyph, a
//! name and a description on three lines, which is a card, not a segment.
//!
//! [`SegmentedToggle`]: crate::components::segmented_toggle::SegmentedToggle

use freya::components::use_theme;
use freya::prelude::*;

use crate::apps::export::{ExportCtx, ExportThemePartial, ExportThemePreference, FormatId};
use crate::components::icon::{Icon, IconName};
use crate::components::type_palette::{kind_color, type_palette};
use crate::components::typography::{Eyebrow, Meta, Strong};
use strata_model::Kind;

#[derive(PartialEq)]
pub struct Formats;

impl Component for Formats {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let ctx = use_consume::<ExportCtx>();
        let selected = ctx.draft.read().format;

        let mut row = rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .spacing(8.);
        for format in FormatId::ALL {
            row = row.child(
                FormatCard {
                    format,
                    selected: format == selected,
                    key: DiffKey::None,
                }
                .key(format.name()),
            );
        }

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(12.)
            .child(Eyebrow::new("FORMAT").color(theme.label_color))
            .child(row)
    }
}

/// One format card: glyph, name, one-line description. The selected card wears the accent
/// border + tint; the rest sit on the panel surface.
#[derive(PartialEq)]
struct FormatCard {
    format: FormatId,
    selected: bool,
    key: DiffKey,
}

impl KeyExt for FormatCard {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for FormatCard {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ExportThemePartial>, ExportThemePreference, "export");
        let mut ctx = use_consume::<ExportCtx>();
        let mut hovered = use_state(|| false);
        let accent = use_theme().read().colors().primary;
        let palette = type_palette();

        let format = self.format;
        let selected = self.selected;

        // The glyph takes the type palette's hue for the shape of data the format holds — the
        // canvas's per-format stroke colours are that same ramp, so they are named once here
        // rather than restated as four theme fields.
        let glyph = match format {
            FormatId::Csv => kind_color(Kind::Str, &palette),
            FormatId::Json => kind_color(Kind::Ts, &palette),
            FormatId::Parquet => accent,
            FormatId::Arrow => kind_color(Kind::Struct, &palette),
        };

        let (background, border) = if selected {
            (theme.card_active_background, theme.card_active_border_fill)
        } else if hovered() {
            (theme.panel_background, accent.with_a(120))
        } else {
            (theme.panel_background, theme.control_border_fill)
        };

        rect()
            .width(Size::flex(1.))
            .vertical()
            .spacing(8.)
            .padding((12., 12.))
            .corner_radius(8.)
            .background(background)
            .border(Border::new().width(1.).fill(border))
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |_| {
                // A format switch never discards the other formats' options — the draft keeps
                // them side by side (see `ExportDraft`).
                ctx.draft.write().format = format;
            })
            .child(Icon::new(IconName::File).size(17.).color(glyph))
            .child(Strong::new(format.name()).color(theme.card_color))
            .child(
                Meta::new(format.description())
                    .color(theme.label_color)
                    .wrap(),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
