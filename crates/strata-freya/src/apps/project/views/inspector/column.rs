//! The inspector's body once a column has resolved: its title, the note a derived column
//! carries, the shape of a nested type, and the STATISTICS zone (the facts box + the
//! completeness bar).
//!
//! Built to the `Strata.dc.html` inspector canvas. The zone's **scan** half — the age /
//! view-as-query / re-scan controls, the distribution bars, the running state, and the action
//! behind `Profile table` — is P3-09's. The card that offers it is rendered here in full, with
//! no press handler: the affordance ships inert, the capability ships with the task that owns it
//! (AGENTS.md §5).

use freya::prelude::*;

use super::model::{
    completeness, fact_rows, nested_fields, ColumnFacts, NestedField, SourceFormat,
};
use super::{InspectorTheme, PANEL_PAD};
use crate::components::badge::Badge;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::type_palette::{kind_color, type_palette, TypePaletteTheme};
use crate::components::typography::{Control, Eyebrow, Meta, MonoValue, Path, Prose};
use crate::components::ACTION_HEIGHT;

/// Corner radius of the facts box and the profile card (canvas `--r-3`); the smaller boxes use
/// `--r-2`, and the badges `--r-xs`.
const BOX_RADIUS: f32 = 10.;
const PANEL_RADIUS: f32 = 8.;
const BADGE_RADIUS: f32 = 4.;
/// A nested field row's height, and the indent one nesting level adds.
const FIELD_HEIGHT: f32 = 27.;
const FIELD_INDENT: f32 = 13.;
/// The completeness track.
const TRACK_HEIGHT: f32 = 8.;
/// The profile card's icon tile, and the alpha its accent tint carries (canvas: 13%).
const TILE_SIZE: f32 = 36.;
const TILE_TINT: u8 = 33;
/// How wide the profile card's copy may run before it wraps (canvas `max-width: 230px`).
const CARD_COPY_WIDTH: f32 = 230.;

/// A 1px bottom-edge-only rule — the hairline *between* two rows of a box.
fn row_rule() -> BorderWidth {
    BorderWidth {
        top: 0.,
        right: 0.,
        bottom: 1.,
        left: 0.,
    }
}

/// A 1px top-edge-only rule — the STATISTICS zone's boundary.
fn zone_rule() -> BorderWidth {
    BorderWidth {
        top: 1.,
        right: 0.,
        bottom: 0.,
        left: 0.,
    }
}

#[derive(PartialEq)]
pub struct ColumnPanel {
    pub facts: ColumnFacts,
    pub theme: InspectorTheme,
}

impl Component for ColumnPanel {
    fn render(&self) -> impl IntoElement {
        // The palette is resolved **here**, once, and passed down: it is a theme read, which is
        // a hook, and two of the three sections below are conditional — reading it inside
        // `nested_box` would make the hook count depend on whether the selected column happens
        // to be nested. Every other colour comes off `self.theme`, which needs no hook at all.
        let palette = type_palette();
        let swatch = kind_color(self.facts.kind, &palette);
        let fields = nested_fields(&self.facts.children);

        rect()
            .width(Size::fill())
            .vertical()
            .child(self.title(swatch))
            // A view's column is defined by a query, not by a file — so the panel says why the
            // facts box below it has only a type in it, rather than leaving the emptiness to
            // read as a bug.
            .maybe_child(self.facts.derived.then(|| self.derived_note()))
            .maybe_child((!fields.is_empty()).then(|| self.nested_box(fields, &palette)))
            .child(self.statistics())
    }
}

impl ColumnPanel {
    /// The column's identity: its swatch and name, then its dtype, where it came from, and
    /// which of the two owns it.
    fn title(&self, swatch: Color) -> Element {
        let t = &self.theme;
        rect()
            .width(Size::fill())
            .vertical()
            .padding(Gaps::new(16., PANEL_PAD, 16., PANEL_PAD))
            .spacing(8.)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(8.)
                    .child(Dot::new(swatch).size(9.).square())
                    .child(
                        MonoValue::new(self.facts.name.clone())
                            .color(t.name_color)
                            .width(Size::flex(1.))
                            .text_overflow(TextOverflow::Ellipsis),
                    ),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    // The three runs wrap rather than truncate: a long dtype (`Timestamp`,
                    // `Decimal`) beside a long owner name would otherwise push "from …" out of
                    // a narrow panel.
                    .content(Content::wrap_spacing(8.))
                    .spacing(8.)
                    .child(Badge::value(self.facts.dtype.clone(), swatch).radius(BADGE_RADIUS))
                    .child(
                        Badge::tag(self.facts.format.label(), self.format_color())
                            .radius(BADGE_RADIUS)
                            // The format badge hugs like a value run, not like a tag: it sits
                            // beside the dtype and the two must read as one pair.
                            .padding(Gaps::new(2., 8., 2., 8.)),
                    )
                    .child(Path::new(format!("from {}", self.facts.owner)).color(t.meta_color)),
            )
            .into_element()
    }

    /// The badge tone for this column's source format. An unknown format keeps the recessive
    /// tone rather than borrowing a colour that means something else.
    fn format_color(&self) -> Color {
        let t = &self.theme;
        match self.facts.format {
            SourceFormat::Parquet => t.format_parquet_color,
            SourceFormat::Csv => t.format_csv_color,
            SourceFormat::Json => t.format_json_color,
            SourceFormat::Arrow => t.format_arrow_color,
            SourceFormat::View => t.format_view_color,
            SourceFormat::Other(_) => t.meta_color,
        }
    }

    fn derived_note(&self) -> Element {
        let t = &self.theme;
        rect()
            .width(Size::fill())
            .margin(Gaps::new(0., PANEL_PAD, 0., PANEL_PAD))
            .padding(PANEL_PAD)
            .corner_radius(PANEL_RADIUS)
            .background(t.box_background)
            .border(Border::new().width(1.).fill(t.border_fill))
            .child(
                Path::new(
                    "Derived column, defined by the view's query. There are no files under it, \
                     so the source reports no statistics.",
                )
                .color(t.note_color)
                .wrap(),
            )
            .into_element()
    }

    /// NESTED FIELDS — the shape of a struct / list / map column, at every depth.
    fn nested_box(&self, fields: Vec<NestedField>, palette: &TypePaletteTheme) -> Element {
        let t = &self.theme;
        let last = fields.len().saturating_sub(1);
        rect()
            .width(Size::fill())
            .vertical()
            .padding(Gaps::new(16., PANEL_PAD, 8., PANEL_PAD))
            .spacing(8.)
            .child(Eyebrow::new("NESTED FIELDS").color(t.label_color))
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .corner_radius(PANEL_RADIUS)
                    .overflow(Overflow::Clip)
                    .border(Border::new().width(1.).fill(t.border_fill))
                    .children(fields.into_iter().enumerate().map(|(i, f)| {
                        let hue = kind_color(f.kind, palette);
                        rect()
                            .width(Size::fill())
                            .height(Size::px(FIELD_HEIGHT))
                            .horizontal()
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .padding((0., PANEL_PAD))
                            .background(t.field_background)
                            .child(rect().width(Size::px(f.depth as f32 * FIELD_INDENT)))
                            .child(Dot::new(hue).size(6.).square())
                            .child(
                                MonoValue::new(f.name)
                                    .color(t.field_color)
                                    .width(Size::flex(1.))
                                    .text_overflow(TextOverflow::Ellipsis),
                            )
                            .child(Meta::new(f.dtype).color(hue))
                            // The rule sits *between* rows: on the last one it would double up
                            // with the box's own bottom edge.
                            .maybe(i < last, |el| {
                                el.border(Border::new().width(row_rule()).fill(t.divider_fill))
                            })
                            .into()
                    })),
            )
            .into_element()
    }

    /// The STATISTICS zone: every fact the source reported, then the completeness bar, then the
    /// (P3-09) offer to compute what a footer can't.
    fn statistics(&self) -> Element {
        let t = &self.theme;
        let rows = fact_rows(&self.facts);
        let last = rows.len().saturating_sub(1);

        rect()
            .width(Size::fill())
            .vertical()
            .margin(Gaps::new(4., 0., 0., 0.))
            .padding(Gaps::new(16., PANEL_PAD, 24., PANEL_PAD))
            .spacing(12.)
            .border(Border::new().width(zone_rule()).fill(t.divider_fill))
            .child(Eyebrow::new("STATISTICS").color(t.label_color))
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .corner_radius(BOX_RADIUS)
                    .overflow(Overflow::Clip)
                    .border(Border::new().width(1.).fill(t.border_fill))
                    .children(rows.into_iter().enumerate().map(|(i, row)| {
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .spacing(PANEL_PAD)
                            .padding((8., PANEL_PAD))
                            .background(t.box_background)
                            .child(Eyebrow::new(row.label).color(t.label_color))
                            // The value takes the slack and right-aligns, so a long Min/Max
                            // (a timestamp, a truncated string bound) truncates at the panel
                            // edge instead of pushing its own key off the row.
                            .child(
                                MonoValue::new(row.value)
                                    .color(t.value_color)
                                    .align(TextAlign::Right)
                                    .width(Size::flex(1.))
                                    .text_overflow(TextOverflow::Ellipsis),
                            )
                            .maybe(i < last, |el| {
                                el.border(Border::new().width(row_rule()).fill(t.divider_fill))
                            })
                            .into()
                    })),
            )
            .maybe_child(self.completeness_bar())
            .child(self.profile_card())
            .into_element()
    }

    /// The completeness bar — present only when a **real** null count exists (see
    /// [`completeness`]). It is never computed off the result page, which is what it used to be.
    fn completeness_bar(&self) -> Option<Element> {
        let t = &self.theme;
        let fill = completeness(&self.facts)?;
        let filled = fill.filled as f32;
        let nulls = 1. - filled;

        let track = rect()
            .width(Size::fill())
            .height(Size::px(TRACK_HEIGHT))
            .corner_radius(TRACK_HEIGHT / 2.)
            .overflow(Overflow::Clip)
            .background(t.border_fill)
            .horizontal()
            .content(Content::Flex)
            // Flex weights rather than percentage widths, so the two shares divide the track
            // exactly however the panel is resized. A zero share contributes no segment at all.
            .maybe(filled > 0., |el| {
                el.child(
                    rect()
                        .width(Size::flex(filled))
                        .height(Size::fill())
                        .background(t.fill_color),
                )
            })
            .maybe(nulls > 0., |el| {
                el.child(
                    rect()
                        .width(Size::flex(nulls))
                        .height(Size::fill())
                        .background(t.null_color),
                )
            });

        Some(
            rect()
                .width(Size::fill())
                .vertical()
                .spacing(8.)
                .child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .main_align(Alignment::SpaceBetween)
                        .child(Meta::new("Completeness").color(t.note_color))
                        .child(Meta::new(fill.label()).color(t.emphasis_color)),
                )
                // The bar carries no numbers, so the numbers are its tooltip: how many rows are
                // null, out of how many, and which side of the split is which.
                .child(
                    TooltipContainer::new(Tooltip::new(fill.detail()))
                        .position(AttachedPosition::Bottom)
                        .width(Size::fill())
                        .child(track),
                )
                .into_element(),
        )
    }

    /// The scan offer (**P3-09**). The card is the canvas's, in its full dress — the action
    /// simply has no handler yet, so pressing it does nothing. The copy names what a scan *would*
    /// do rather than promising when, because the press has to route through P3-10's cost confirm
    /// and neither exists.
    fn profile_card(&self) -> Element {
        let t = &self.theme;
        let (copy, action) = if self.facts.derived {
            (
                "Running the view's query in full would compute distinct counts, means and \
                 distributions.",
                "Profile view",
            )
        } else {
            (
                "Reading every file would compute distinct counts, means and distributions.",
                "Profile table",
            )
        };

        rect()
            .width(Size::fill())
            .vertical()
            .cross_align(Alignment::Center)
            .spacing(12.)
            .corner_radius(BOX_RADIUS)
            .padding((24., 16.))
            .background(t.box_background)
            .border(Border::new().width(1.).fill(t.border_fill))
            .child(
                rect()
                    .width(Size::px(TILE_SIZE))
                    .height(Size::px(TILE_SIZE))
                    .corner_radius(PANEL_RADIUS)
                    .center()
                    .background(t.tile_color.with_a(TILE_TINT))
                    .child(Icon::new(IconName::Chart).color(t.tile_color).size(17.)),
            )
            .child(
                Prose::new(copy)
                    .color(t.note_color)
                    .max_width(Size::px(CARD_COPY_WIDTH))
                    .align(TextAlign::Center)
                    .wrap(),
            )
            .child(
                Button::new()
                    // The stock **filled** dress: the accent over inverse text, which is both
                    // the canvas's `background: var(--accent); color: var(--c-onaccent)` and the
                    // Run control's idle state. No `theme_colors` override — a call site that
                    // restates colours the variant already resolves is a second copy of them.
                    //
                    // **No `on_press` — that is the inert part.** The card is the canvas's
                    // primary call to action and keeps its full dress; P3-09 adds the handler
                    // and nothing else here changes.
                    .filled()
                    // A committing action is the design system's 34px everywhere — one number,
                    // in `components`. The rest of the layout (padding, radius) stays the
                    // `button_layout` theme's.
                    .theme_layout(
                        ButtonLayoutThemePartial::default().height(Size::px(ACTION_HEIGHT)),
                    )
                    .child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .child(Icon::new(IconName::Chart).size(14.))
                            .child(Control::new(action)),
                    ),
            )
            .into_element()
    }
}
