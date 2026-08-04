//! The Chart body's **control strip** — the fixed-width column down the left of the canvas
//! (canvas `Strata.dc.html`, the chart view's first child).
//!
//! It carries the **mark picker**, the **encoders** (X / Y / Series), the **sort** toggle and
//! the **legend**. Every control commits a whole [`ChartConfig`] through one write
//! ([`commit`]) on `Chan::Chart(tab)`, so an encoder edit re-charts and wakes nothing else.
//!
//! **The options are the constraint.** What each control offers comes from `config`'s
//! per-mark option sets, so an encoding a mark cannot take is unreachable rather than
//! reported: a pie's Y replaces instead of accumulating, a scatter and a histogram have no
//! series row at all, and no menu ever lists a column the read would refuse. The residual
//! cases — nothing valid left to offer — are the canvas's notice, not an inline error.
//!
//! Six mark tiles, three to a row, each a glyph over a name — a tile, not a segment, for the
//! same reason the Export window's format cards aren't segments: a `SegmentedToggle` holding
//! six labelled options in 232px would give each one 33px.
//!
//! **The legend lives here rather than on the canvas**, which is a deliberate divergence from
//! the design (whose canvas draws a key inside the plot, for the pie). A plot-overlay legend
//! has nowhere to go when it outgrows its box: plotters sizes the box to its entries and draws
//! it inside the plotting area, so four long column names push it over the edge of the pane,
//! and a 24-slice pie has no honest layout at all. The strip already scrolls, so the legend
//! grows down instead of over — and the plot keeps its whole width for data.
//!
//! The design's **Aggregate** toggle and its function menu are deliberately absent: the chart
//! computes nothing SQL can say (spec §1.2, §1.3), and aggregation is reached through Chart
//! 04's scaffold instead.

use freya::components::get_theme;
use freya::components::{MenuItem, ScrollView, Select, SelectThemePartial};
use freya::prelude::*;
use freya::radio::{use_radio, Radio};
use strata_model::{ChartConfig, ChartMark, ChartSort, ChartX, TabId};

use super::config::{
    allows_row_index, series_options, sortable, takes_many_ys, x_options, y_options, Encoding,
    Roles,
};
use super::{ChartTheme, ChartThemePartial, ChartThemePreference};
use crate::apps::project::state::{Chan, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{Caption, Eyebrow, Meta, MonoValue};

/// The strip's own width (canvas: `width: 232px`) and inset.
pub const STRIP_WIDTH: f32 = 232.;
const STRIP_PADDING: f32 = 12.;
/// The gap between the tiles, and between one section and the next.
const TILE_GAP: f32 = 8.;
const SECTION_GAP: f32 = 16.;
/// Tiles to a row (canvas: `grid-template-columns: 1fr 1fr 1fr`).
const TILES_PER_ROW: usize = 3;
/// What a control gets across the strip's inset — the width every encoder's trigger takes.
const CONTROL_WIDTH: f32 = STRIP_WIDTH - 2. * STRIP_PADDING;
/// How wide a trigger's own label may run before it ellipsizes: the trigger, less the
/// `Select`'s side padding (18 each) and its 10px arrow with the 8px margin in front of it.
/// Stated as the arithmetic rather than a number, because the parts are the component's.
const TRIGGER_LABEL_WIDTH: f32 = CONTROL_WIDTH - 2. * 18. - 18.;
/// The gap between a section's eyebrow and its control.
const LABEL_GAP: f32 = 6.;
/// A menu row's tick column — reserved whether or not the row is ticked, so the labels of a
/// multi-pick list line up under each other.
const TICK_WIDTH: f32 = 16.;

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

/// **The strip's one write.** Every control — a tile, a menu row, a sort segment — commits the
/// whole config, so there is one place that knows which channel an encoder edit lands on.
fn commit(mut session: Radio<SessionState, Chan>, tab: TabId, next: ChartConfig) {
    session.write_channel(Chan::Chart(tab)).set_chart(tab, next);
}

/// What "nothing on this channel" reads as, per channel. Both are real choices, not empty
/// states: charting against the row index is what "X: none" means, and a chart with no series
/// column is the ordinary case.
const ROW_INDEX: &str = "Row index";
const NO_SERIES: &str = "None";

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

/// One option in an encoder's menu: how it reads, whether it is the one in effect, and — the
/// point of the shape — **the config it commits**. Resolving what a press means happens once,
/// in the strip's own render where the encoding rules are; the row that carries it knows
/// nothing but how to write it.
#[derive(Clone, PartialEq)]
struct Choice {
    label: String,
    selected: bool,
    next: ChartConfig,
    /// Whether the list stays open afterwards — a multi-pick Y, where a press adds a series
    /// rather than answering the question.
    keep_open: bool,
}

/// The control strip: the mark picker, the encoders, the sort and the legend over their own
/// scroll, so a strip taller than the pane scrolls rather than squashing its controls.
#[derive(PartialEq)]
pub struct ControlStrip {
    tab: TabId,
    /// The stored intent — what a control mutates.
    config: ChartConfig,
    /// What is actually being drawn — what a control shows as chosen. The two differ wherever
    /// a channel is unset (it shows its default) or names a column this result cannot answer.
    encoding: Encoding,
    roles: Roles,
    legend: Vec<LegendEntry>,
}

impl ControlStrip {
    pub fn new(tab: TabId, config: ChartConfig, encoding: Encoding, roles: Roles) -> Self {
        Self {
            tab,
            config,
            encoding,
            roles,
            legend: Vec::new(),
        }
    }

    /// What the plot's colours mean, in the order they are drawn. Empty for a mark that draws
    /// in one colour (there is nothing to key), and for a state that draws nothing at all.
    pub fn legend(mut self, legend: Vec<LegendEntry>) -> Self {
        self.legend = legend;
        self
    }

    /// This config with one channel changed — every control's commit starts here, so a press
    /// on one encoder can never quietly rewrite another.
    fn with(&self, edit: impl FnOnce(&mut ChartConfig)) -> ChartConfig {
        let mut next = self.config.clone();
        edit(&mut next);
        next
    }

    /// The category-axis menu: the row index where the mark allows it, then every column the
    /// mark's X will take.
    fn x_choices(&self, mark: ChartMark) -> Vec<Choice> {
        let mut choices = Vec::new();
        if allows_row_index(mark) {
            choices.push(Choice {
                label: ROW_INDEX.to_string(),
                selected: self.encoding.x.is_none(),
                next: self.with(|c| c.x = ChartX::RowIndex),
                keep_open: false,
            });
        }
        for name in x_options(mark, &self.roles) {
            choices.push(Choice {
                selected: self.encoding.x.as_deref() == Some(name.as_str()),
                next: self.with(|c| c.x = ChartX::Column(name.clone())),
                label: name,
                keep_open: false,
            });
        }
        choices
    }

    /// The value menu. Where the mark draws several Ys a press **toggles** one — the list
    /// stays open, and the new set keeps result order so a series' colour doesn't move when
    /// another is ticked. Where it draws one, a press replaces.
    fn y_choices(&self, mark: ChartMark) -> Vec<Choice> {
        let many = takes_many_ys(mark);
        let measures = y_options(&self.roles);
        measures
            .iter()
            .cloned()
            .map(|name| {
                let selected = self.encoding.ys.contains(&name);
                let ys = if many {
                    measures
                        .iter()
                        .filter(|other| {
                            if **other == name {
                                !selected
                            } else {
                                self.encoding.ys.contains(other)
                            }
                        })
                        .cloned()
                        .collect()
                } else {
                    vec![name.clone()]
                };
                Choice {
                    selected,
                    next: self.with(|c| c.ys = Some(ys)),
                    label: name,
                    keep_open: many,
                }
            })
            .collect()
    }

    /// The series menu: no split, then every column that can carry one beside the current X.
    fn series_choices(&self, options: Vec<String>) -> Vec<Choice> {
        let mut choices = vec![Choice {
            label: NO_SERIES.to_string(),
            selected: self.encoding.series.is_none(),
            next: self.with(|c| c.series = None),
            keep_open: false,
        }];
        for name in options {
            choices.push(Choice {
                selected: self.encoding.series.as_deref() == Some(name.as_str()),
                next: self.with(|c| c.series = Some(name.clone())),
                label: name,
                keep_open: false,
            });
        }
        choices
    }
}

impl Component for ControlStrip {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let mark = self.encoding.mark;

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
            for tile in row {
                line = line.child(
                    MarkTile {
                        mark: *tile,
                        selected: *tile == mark,
                        tab: self.tab,
                        next: self.with(|c| c.mark = Some(*tile)),
                        key: DiffKey::None,
                    }
                    .key(tile.label()),
                );
            }
            tiles = tiles.child(line);
        }

        // Each encoder appears only where its channel means something for this mark: a
        // histogram has no category axis, a scatter and a pie have no series, and a result
        // with no numeric column has nothing to offer on Y.
        let x_choices = self.x_choices(mark);
        let x = (!x_choices.is_empty()).then(|| {
            Encoder {
                tab: self.tab,
                label: "X AXIS",
                current: self
                    .encoding
                    .x
                    .clone()
                    .unwrap_or_else(|| ROW_INDEX.to_string()),
                options: x_choices,
                key: DiffKey::None,
            }
            .key("x")
        });

        let y_choices = self.y_choices(mark);
        let y = (!y_choices.is_empty()).then(|| {
            Encoder {
                tab: self.tab,
                label: "Y AXIS",
                // Every plotted column, in the order the legend keys them.
                current: if self.encoding.ys.is_empty() {
                    NO_SERIES.to_string()
                } else {
                    self.encoding.ys.join(", ")
                },
                options: y_choices,
                key: DiffKey::None,
            }
            .key("y")
        });

        let series_options = series_options(mark, &self.roles, self.encoding.x.as_deref());
        let series = (!series_options.is_empty()).then(|| {
            Encoder {
                tab: self.tab,
                label: "SERIES (COLOR)",
                current: self
                    .encoding
                    .series
                    .clone()
                    .unwrap_or_else(|| NO_SERIES.to_string()),
                options: self.series_choices(series_options),
                key: DiffKey::None,
            }
            .key("series")
        });

        // The sort is a view transform over the settled rows (spec §6) — offered only for the
        // marks whose data has an order to permute.
        let sort = sortable(mark).then(|| SortToggle {
            tab: self.tab,
            current: self.encoding.sort,
            config: self.config.clone(),
        });

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
        // also what lets the encoders and the legend be as long as the result has columns.
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
                        .maybe_child(x)
                        .maybe_child(y)
                        .maybe_child(series)
                        .maybe_child(sort)
                        .maybe_child(legend),
                ),
            )
    }
}

/// One encoder: its eyebrow over the app-standard [`Select`], whose rows are the [`Choice`]s
/// the strip resolved.
///
/// A multi-pick row `prevent_default`s its press, which is what keeps the list open. The
/// `Select` closes on `on_global_pointer_press`; non-capture globals are emitted **last**
/// (`EventName::priority`), and `PointerPress`'s cancellable set includes `GlobalPointerPress`
/// (`events/name.rs`), so a prevented row press removes the close before it is ever handled.
/// Nothing else has to hold: the select's other closer is a focus-within test, and focus never
/// moved off the trigger — `MenuItem` requests focus only on the *unprevented* path, so the row
/// does not take it. Picking several Ys is therefore one gesture, and a single-pick row (which
/// answers the question) still closes on the way out. Both are pinned by tests below, because
/// the mechanism is the fork's and a comment is not evidence.
#[derive(PartialEq)]
struct Encoder {
    tab: TabId,
    label: &'static str,
    /// What the trigger reads — the channel as it is being drawn, not as it was stored.
    current: String,
    options: Vec<Choice>,
    key: DiffKey,
}

impl KeyExt for Encoder {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Encoder {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let session = use_radio::<SessionState, Chan>(Chan::Chart(self.tab));
        let tab = self.tab;

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(LABEL_GAP)
            .child(Eyebrow::new(self.label).color(theme.label_color))
            .child(
                Select::new()
                    // The strip's controls run its full inset width; the component's own
                    // default hugs its content, which would leave four ragged triggers.
                    .theme(SelectThemePartial::new().width(Size::px(CONTROL_WIDTH)))
                    .selected_item(
                        MonoValue::new(self.current.clone())
                            .max_width(Size::px(TRIGGER_LABEL_WIDTH))
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .children(self.options.iter().map(|choice| {
                        let next = choice.next.clone();
                        let keep_open = choice.keep_open;
                        MenuItem::new()
                            .selected(choice.selected)
                            .on_press(move |e: Event<PressEventData>| {
                                if keep_open {
                                    e.prevent_default();
                                }
                                commit(session, tab, next.clone());
                            })
                            .child(
                                rect()
                                    .horizontal()
                                    .cross_align(Alignment::Center)
                                    .child(
                                        rect().width(Size::px(TICK_WIDTH)).maybe_child(
                                            choice
                                                .selected
                                                .then(|| Icon::new(IconName::Check).size(12.)),
                                        ),
                                    )
                                    .child(MonoValue::new(choice.label.clone())),
                            )
                    })),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The sort control: the three orders as one segmented pill under its eyebrow.
///
/// Its own component so its `Radio` handle is a hook this strip takes once, unconditionally —
/// the pill is only shown for some marks, and a hook behind a condition is a hook count that
/// changes between renders.
#[derive(PartialEq)]
struct SortToggle {
    tab: TabId,
    current: ChartSort,
    /// The config each segment commits its own order into.
    config: ChartConfig,
}

impl Component for SortToggle {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let session = use_radio::<SessionState, Chan>(Chan::Chart(self.tab));
        let tab = self.tab;

        let mut pill = SegmentedToggle::new();
        for order in ChartSort::ALL {
            let next = ChartConfig {
                sort: order,
                ..self.config.clone()
            };
            pill = pill.child(
                ToggleSegment::text(order.label())
                    .title(order.title())
                    .selected(self.current == order)
                    .on_press(move |_| commit(session, tab, next.clone())),
            );
        }

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(LABEL_GAP)
            .child(Eyebrow::new("SORT").color(theme.label_color))
            .child(pill)
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
    tab: TabId,
    /// The config this tile commits — the whole thing, so switching mark cannot disturb the
    /// column assignments it narrows (a pie draws one of four Ys; the other three are still
    /// there when you switch back).
    next: ChartConfig,
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
        let session = use_radio::<SessionState, Chan>(Chan::Chart(self.tab));

        let (background, border, color) = tile_dress(&theme, self.selected, hovered());
        let tab = self.tab;
        let next = self.next.clone();

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
            .on_press(move |_| commit(session, tab, next.clone()))
            .child(Icon::new(glyph(self.mark)).size(17.))
            .child(Caption::new(self.mark.label()))
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

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field};
    use freya::radio::RadioStation;
    use freya_testing::TestingRunner;
    use strata_core::engine::column_info;
    use strata_core::theme::load;
    use strata_model::{ColumnInfo, Origin};

    use super::super::config::resolve;
    use super::*;
    use crate::theme::strata_theme;

    /// A date, a category and two measures — enough for every encoder to have something to
    /// offer, built from real Arrow fields through the engine's own `column_info` so the
    /// menus are the ones this result would really produce.
    fn columns() -> Vec<ColumnInfo> {
        [
            ("month", DataType::Date32),
            ("country", DataType::Utf8),
            ("revenue", DataType::Int64),
            ("cost", DataType::Float64),
        ]
        .into_iter()
        .map(|(name, dtype)| column_info(&Field::new(name, dtype, true)))
        .collect()
    }

    /// The strip over a real session store, resolving its props from that store on every
    /// render — which is what the body does, and what makes a commit visible to the next
    /// render rather than frozen in a prop the test computed once.
    fn runner() -> (TestingRunner, RadioStation<SessionState, Chan>) {
        let mut session = SessionState::default();
        let tab = session.open_named("chart", "SELECT 1".into(), Origin::Scratch);
        let app = move || {
            use_init_theme(|| strata_theme(&load("midnight")));
            let store = use_radio::<SessionState, Chan>(Chan::Chart(tab));
            let config = store.read().chart(tab);
            let roles = Roles::of(&columns());
            let encoding = resolve(&config, &roles);
            rect()
                .expanded()
                .child(ControlStrip::new(tab, config, encoding, roles))
        };
        TestingRunner::new(
            app,
            (400., 1400.).into(),
            move |r| r.provide_root_context(|| RadioStation::<SessionState, Chan>::create(session)),
            1.,
        )
    }

    /// Settle the tree and the effects those renders scheduled — several passes, because
    /// Freya only polls tasks once nothing is dirty (the catalog tests' note).
    fn settle(runner: &mut TestingRunner) {
        for _ in 0..4 {
            runner.sync_and_update();
        }
    }

    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    fn shows(runner: &TestingRunner, text: &str) -> bool {
        texts(runner).iter().any(|t| t == text)
    }

    /// Click the centre of the first text run equal to `text`. Coordinates come from the
    /// laid-out node, so these tests encode no pixel offsets.
    fn click_text(runner: &mut TestingRunner, text: &str) {
        let area = runner
            .find(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == text)
                    .map(|_| node.layout().area)
            })
            .unwrap_or_else(|| panic!("no text run {text:?} in the tree: {:?}", texts(runner)));
        let point = (
            f64::from(area.min_x() + area.width() / 2.),
            f64::from(area.min_y() + area.height() / 2.),
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        settle(runner);
    }

    /// The tab's config, as the strip has written it.
    fn config(session: &RadioStation<SessionState, Chan>) -> ChartConfig {
        let state = session.peek();
        state.chart(*state.order.first().expect("the one tab"))
    }

    /// **The strip resolves what it shows from the schema.** Nothing is stored yet, so every
    /// control reads its default: a line over a date, both measures, no series.
    #[test]
    fn an_untouched_chart_shows_the_defaults_the_schema_derived() {
        let (mut runner, _) = runner();
        settle(&mut runner);

        let seen = texts(&runner);
        for expected in ["CHART TYPE", "X AXIS", "Y AXIS", "SERIES (COLOR)", "SORT"] {
            assert!(
                seen.contains(&expected.to_string()),
                "no {expected}: {seen:?}"
            );
        }
        assert!(shows(&runner, "month"), "the derived X: {seen:?}");
        assert!(shows(&runner, "revenue, cost"), "the derived Ys: {seen:?}");
    }

    /// **Picking several Ys is one gesture.** A multi-pick row prevents the press's default,
    /// which cancels the global press the `Select` closes on — so the list is still open for
    /// the next tick, and both writes land. The set comes back in *result* order, so a
    /// series' colour doesn't move when another is ticked.
    #[test]
    fn the_y_list_stays_open_across_picks_and_keeps_result_order() {
        let (mut runner, session) = runner();
        settle(&mut runner);

        click_text(&mut runner, "revenue, cost");
        assert!(shows(&runner, "cost"), "the list opened");

        // Untick the second measure…
        click_text(&mut runner, "cost");
        assert_eq!(
            config(&session).ys.as_deref(),
            Some(["revenue".to_string()].as_ref())
        );
        // …and the list is still there to tick it back, without reopening it.
        assert!(
            shows(&runner, "cost"),
            "the multi-pick list closed on the first tick: {:?}",
            texts(&runner)
        );

        click_text(&mut runner, "cost");
        assert_eq!(
            config(&session).ys.as_deref(),
            Some(["revenue".to_string(), "cost".to_string()].as_ref()),
            "result order, not pick order"
        );
    }

    /// A single-pick encoder answers its question and closes — the ordinary `Select`
    /// behaviour, which is what makes the multi-pick above a deliberate exception rather than
    /// the default.
    #[test]
    fn a_single_pick_encoder_closes_on_the_pick() {
        let (mut runner, session) = runner();
        settle(&mut runner);

        click_text(&mut runner, "month");
        assert!(shows(&runner, ROW_INDEX), "the X list opened");

        click_text(&mut runner, "country");
        assert_eq!(config(&session).x, ChartX::Column("country".into()));
        assert!(
            !shows(&runner, ROW_INDEX),
            "the list stayed open after a single pick: {:?}",
            texts(&runner)
        );
    }

    /// A mark tile writes the mark and nothing else, and the strip re-offers itself around
    /// it: a histogram has no category axis and no series, so both rows go.
    #[test]
    fn a_mark_tile_rewrites_only_the_mark_and_the_strip_follows_it() {
        let (mut runner, session) = runner();
        settle(&mut runner);

        click_text(&mut runner, "Histogram");
        let stored = config(&session);
        assert_eq!(stored.mark, Some(ChartMark::Histogram));
        assert_eq!(stored.x, ChartX::Auto, "the tile left the other channels");
        assert_eq!(stored.ys, None);

        let seen = texts(&runner);
        assert!(!seen.contains(&"X AXIS".to_string()), "{seen:?}");
        assert!(!seen.contains(&"SERIES (COLOR)".to_string()), "{seen:?}");
        assert!(
            !seen.contains(&"SORT".to_string()),
            "bins are ordered already: {seen:?}"
        );
        assert!(
            seen.contains(&"Y AXIS".to_string()),
            "the value column: {seen:?}"
        );
    }

    /// The sort is a press, and it lands on the config the read never sees.
    #[test]
    fn the_sort_toggle_writes_the_view_transform() {
        let (mut runner, session) = runner();
        settle(&mut runner);

        click_text(&mut runner, ChartSort::ByYDesc.label());
        assert_eq!(config(&session).sort, ChartSort::ByYDesc);
    }
}
