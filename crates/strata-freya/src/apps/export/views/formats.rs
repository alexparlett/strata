//! The FORMAT row — the formats this engine can write, as cards.
//!
//! The list is the engine's own format registry filtered on what `COPY` can write, so a format
//! an embedder registered has a card here and one that is read-only does not. No Clipboard tile:
//! the canvas dropped it (2026-07-12) once the grid grew its own copy controls, so "export" here
//! always means a file on disk.
//!
//! Each card is its own press target rather than a [`SegmentedToggle`]: they carry a glyph, a
//! name and a description on three lines, which is a card, not a segment.
//!
//! [`SegmentedToggle`]: crate::components::segmented_toggle::SegmentedToggle

use freya::prelude::*;

use crate::apps::export::{ExportCtx, ExportThemePartial, ExportThemePreference, FormatId};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_2, SP_3, SP_4};
use crate::components::type_palette::{kind_color, type_palette};
use crate::components::typography::{Eyebrow, Meta, Strong};
use crate::theme::{use_roles, Role};
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
            .spacing(SP_3);
        for format in ctx.formats.read().iter().copied() {
            row = row.child(
                FormatCard {
                    format,
                    selected: format == selected,
                    key: DiffKey::None,
                }
                .key(format.extension()),
            );
        }

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(SP_4)
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
        let ctx = use_consume::<ExportCtx>();
        let mut hovered = use_state(|| false);
        let accent = use_roles().get(Role::Accent);
        let palette = type_palette();

        let format = self.format;
        let selected = self.selected;

        let glyph = match format {
            FormatId::Csv => kind_color(Kind::Str, &palette),
            FormatId::Json => kind_color(Kind::Ts, &palette),
            FormatId::Parquet => accent,
            FormatId::Arrow => kind_color(Kind::Struct, &palette),
            FormatId::Extension(_) => accent,
        };

        let (background, border) = if selected {
            (theme.card_active_background, theme.card_active_border_fill)
        } else if hovered() {
            (theme.panel_background, theme.card_hover_border_fill)
        } else {
            (theme.panel_background, theme.control_border_fill)
        };

        rect()
            .width(Size::flex(1.))
            .vertical()
            .spacing(SP_3)
            .padding((SP_4, SP_4))
            .corner_radius(R_2)
            .background(background)
            .border(Border::new().width(1.).fill(border))
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |_| {
                ctx.edit(|draft| draft.format = format);
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
