//! The Chart body's **control strip** — the fixed-width column down the left of the canvas
//! (canvas `Strata.dc.html`, the chart view's first child).
//!
//! It carries the **mark picker** and the **legend**. X / Y / Series and the sort toggle are
//! the encoder strip's (Chart 03), which owns `ChartConfig` and drops its controls in between
//! them. What is here is real: pressing a tile writes the mark, and switching between two
//! marks that read the same columns is a repaint of the settled data rather than a re-read
//! (spec §1.2).
//!
//! Six tiles, three to a row, each a glyph over a name — a tile, not a segment, for the same
//! reason the Export window's format cards aren't segments: a `SegmentedToggle` holding six
//! labelled options in 232px would give each one 33px.
//!
//! **The legend lives here rather than on the canvas**, which is a deliberate divergence from
//! the design (whose canvas draws a key inside the plot, for the pie). A plot-overlay legend
//! has nowhere to go when it outgrows its box: plotters sizes the box to its entries and draws
//! it inside the plotting area, so four long column names push it over the edge of the pane,
//! and a 24-slice pie has no honest layout at all. The strip already scrolls, so the legend
//! grows down instead of over — and the plot keeps its whole width for data.

use freya::components::get_theme;
use freya::components::ScrollView;
use freya::prelude::*;
use strata_model::ChartMark;

use super::{ChartTheme, ChartThemePartial, ChartThemePreference};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Caption, Eyebrow, Meta};

/// The strip's own width (canvas: `width: 232px`) and inset.
pub const STRIP_WIDTH: f32 = 232.;
const STRIP_PADDING: f32 = 12.;
/// The gap between the tiles, and between one section and the next.
const TILE_GAP: f32 = 8.;
const SECTION_GAP: f32 = 16.;
/// Tiles to a row (canvas: `grid-template-columns: 1fr 1fr 1fr`).
const TILES_PER_ROW: usize = 3;

/// The strip's one rule: down its right edge, between it and the canvas.
fn strip_rule() -> BorderWidth {
    BorderWidth {
        top: 0.,
        right: 1.,
        bottom: 0.,
        left: 0.,
    }
}

/// The glyph for a mark. The mapping only, never the colour — the tile paints it, at rest or
/// selected.
fn glyph(mark: ChartMark) -> IconName {
    match mark {
        ChartMark::Bar => IconName::MarkBar,
        ChartMark::Line => IconName::MarkLine,
        ChartMark::Area => IconName::MarkArea,
        ChartMark::Scatter => IconName::MarkScatter,
        ChartMark::Histogram => IconName::MarkHistogram,
        ChartMark::Pie => IconName::MarkPie,
    }
}

/// One row of the legend: what a colour on the plot means.
#[derive(Clone, PartialEq)]
pub struct LegendEntry {
    pub swatch: Color,
    pub label: String,
    /// A slice's share of the whole. `None` for a series, whose values are on the axis
    /// already.
    pub detail: Option<String>,
}

/// A legend swatch's box.
const SWATCH: f32 = 10.;

/// The control strip: the mark picker and the legend over their own scroll, so a strip taller
/// than the pane scrolls rather than squashing its controls.
#[derive(PartialEq)]
pub struct ControlStrip {
    mark: State<ChartMark>,
    legend: Vec<LegendEntry>,
}

impl ControlStrip {
    pub fn new(mark: State<ChartMark>) -> Self {
        Self {
            mark,
            legend: Vec::new(),
        }
    }

    /// What the plot's colours mean, in the order they are drawn. Empty for a mark that draws
    /// in one colour (there is nothing to key), and for a state that draws nothing at all.
    pub fn legend(mut self, legend: Vec<LegendEntry>) -> Self {
        self.legend = legend;
        self
    }
}

impl Component for ControlStrip {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let selected = *self.mark.read();

        let mut tiles = rect()
            .width(Size::fill())
            .vertical()
            .spacing(TILE_GAP)
            .child(Eyebrow::new("CHART TYPE").color(theme.label_color));
        for row in ChartMark::ALL.chunks(TILES_PER_ROW) {
            let mut line = rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .spacing(TILE_GAP);
            for mark in row {
                line = line.child(
                    MarkTile {
                        mark: *mark,
                        selected: *mark == selected,
                        target: self.mark,
                        key: DiffKey::None,
                    }
                    .key(mark.label()),
                );
            }
            tiles = tiles.child(line);
        }

        let legend = (!self.legend.is_empty()).then(|| {
            let mut section = rect()
                .width(Size::fill())
                .vertical()
                .spacing(TILE_GAP)
                .child(Eyebrow::new("LEGEND").color(theme.label_color));
            for (nth, entry) in self.legend.iter().enumerate() {
                section = section.child(
                    LegendRow {
                        entry: entry.clone(),
                        key: DiffKey::None,
                    }
                    .key(nth),
                );
            }
            section
        });

        // `ScrollView` takes no padding of its own, so the inset lives on a wrapper inside it
        // — which also keeps the scrollbar flush to the strip's edge. The right border is
        // painted rather than laid out, so the padding covers it (AGENTS.md §3). The scroll is
        // also what lets the legend be as long as the plot has colours.
        rect()
            .width(Size::px(STRIP_WIDTH))
            .height(Size::fill())
            .background(theme.panel_background)
            .border(Border::new().width(strip_rule()).fill(theme.border_fill))
            .child(
                ScrollView::new().child(
                    rect()
                        .width(Size::fill())
                        .vertical()
                        .padding(STRIP_PADDING)
                        .spacing(SECTION_GAP)
                        .child(tiles)
                        .maybe_child(legend),
                ),
            )
    }
}

/// One legend row: the swatch a mark is drawn in, the name it carries, and — for a slice —
/// its share, which a pie has no axis to read off.
#[derive(PartialEq)]
struct LegendRow {
    entry: LegendEntry,
    key: DiffKey,
}

impl KeyExt for LegendRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for LegendRow {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(8.)
            .child(
                rect()
                    .width(Size::px(SWATCH))
                    .height(Size::px(SWATCH))
                    .corner_radius(2.)
                    .background(self.entry.swatch),
            )
            .child(
                // Flexing and ellipsizing, so a long column name gives up its own width
                // rather than pushing the share off the strip (AGENTS.md §3).
                Caption::new(self.entry.label.clone())
                    .color(theme.legend_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(
                self.entry
                    .detail
                    .clone()
                    .map(|detail| Meta::new(detail).color(theme.tick_color)),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// One mark's tile: glyph over name, accent-tinted while it is the chosen mark.
#[derive(PartialEq)]
struct MarkTile {
    mark: ChartMark,
    selected: bool,
    target: State<ChartMark>,
    key: DiffKey,
}

impl KeyExt for MarkTile {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for MarkTile {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let mut hovered = use_state(|| false);

        let (background, border, color) = tile_dress(&theme, self.selected, hovered());
        let mark = self.mark;
        let mut target = self.target;

        rect()
            .width(Size::flex(1.))
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .spacing(4.)
            .padding((8., 4.))
            .corner_radius(8.)
            .background(background)
            .border(Border::new().width(1.).fill(border))
            .color(color)
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |_| target.set(mark))
            .child(Icon::new(glyph(mark)).size(17.))
            .child(Caption::new(mark.label()))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// A tile's three colours, by state — selected wins over hover, which is the accent edge on
/// an otherwise resting tile.
fn tile_dress(theme: &ChartTheme, selected: bool, hovered: bool) -> (Color, Color, Color) {
    if selected {
        (
            theme.tile_active_background,
            theme.tile_active_border_fill,
            theme.tile_active_color,
        )
    } else if hovered {
        (
            theme.panel_background,
            theme.tile_active_border_fill.with_a(120),
            theme.tile_active_color,
        )
    } else {
        (
            theme.panel_background,
            theme.tile_border_fill,
            theme.tile_color,
        )
    }
}
