//! The Chart body's **control strip** — the fixed-width column down the left of the canvas
//! (canvas `Strata.dc.html`, the chart view's first child).
//!
//! It carries the **mark picker**, the **encoders** (X / Y / Series), a histogram's **bin
//! count**, the **sort** and **scale** toggles, and the **legend**. Every control commits a
//! whole [`ChartConfig`] through one write ([`commit`]) on `Chan::Chart(tab)`, so an encoder
//! edit re-charts and wakes nothing else.
//!
//! **One of them is a read and the rest are repaints.** The bin count reaches the
//! [`ChartQuery`](strata_model::ChartQuery), because the engine does the counting; the sort,
//! the scale and the legend's hidden set are transforms over data already in hand.
//!
//! **The options are the constraint.** What each control offers comes from `config`'s
//! per-mark option sets, so an encoding a mark cannot take is unreachable rather than
//! reported: a pie's Y replaces instead of accumulating, a scatter and a histogram have no
//! series row at all, and no menu ever lists a column the read would refuse. The residual
//! cases — nothing valid left to offer — are the canvas's notice, not an inline error.
//!
//! Nine mark tiles, three to a row, each a glyph over a name — a tile, not a segment, for the
//! same reason the Export window's format cards aren't segments: a `SegmentedToggle` holding
//! nine labelled options in 232px would give each one 22px.
//!
//! **The legend lives here rather than on the canvas**, and it is also the control that hides
//! a series — which is a deliberate divergence from
//! the design (whose canvas draws a key inside the plot, for the pie). A plot-overlay legend
//! has nowhere to go when it outgrows its box: plotters sizes the box to its entries and draws
//! it inside the plotting area, so four long column names push it over the edge of the pane,
//! and a 24-slice pie has no honest layout at all. The strip already scrolls, so the legend
//! grows down instead of over — and the plot keeps its whole width for data.
//!
//! The design's **Aggregate** toggle and its function menu are deliberately absent, and nothing
//! stands in their place: the chart computes nothing SQL can say (spec §1.2, §1.3), so every
//! control here changes what is *drawn* and none of them changes the data. Aggregating is the
//! user's own `GROUP BY`, which the refusal overlays name in prose. A press that wrote that
//! query into a new tab was built and cut — spec §8 records why — and the surface that
//! replaced it is the **Shape panel** (Chart 09), off the results toolbar, never this strip.

use freya::components::get_theme;
use freya::components::{MenuItem, ScrollView, Select, SelectThemePartial};
use freya::prelude::*;
use freya::radio::{use_radio, Radio};
use strata_core::engine::MAX_BINS;
use strata_model::{ChartConfig, ChartMark, ChartSort, ChartX, TabId};

use super::config::{
    allows_row_index, log_axis, reads_bounds, reads_quartiles, series_options, series_required,
    sortable, takes_many_ys, trendable, x_options, y_options, Encoding, Roles,
};
use super::{ChartTheme, ChartThemePartial, ChartThemePreference};
use crate::apps::project::state::{Chan, SessionState};
use crate::components::form::ValueField;
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
        ChartMark::Heatmap => IconName::MarkHeatmap,
        ChartMark::Band => IconName::MarkBand,
        ChartMark::Box => IconName::MarkBox,
    }
}

/// **The strip's one write.** Every control — a tile, a menu row, a sort segment — commits the
/// whole config, so there is one place that knows which channel an encoder edit lands on.
fn commit(session: Radio<SessionState, Chan>, tab: TabId, next: ChartConfig) {
    edit(session, tab, move |config| *config = next);
}

/// The same write, over the config **as the store currently holds it** — for the one control
/// that commits from an effect rather than from a press.
///
/// A press handler is rebuilt on every render, so the config it captured is fresh. A
/// `use_side_effect` closure is built *once* (AGENTS.md §3), so a config captured for one is
/// the config from the first render — and committing it would undo every encoder edit made
/// since. `Radio` has no non-subscribing read, and it does not need one: the guard that writes
/// dereferences to the store, so the read and the write are the same borrow.
fn edit(mut session: Radio<SessionState, Chan>, tab: TabId, edit: impl FnOnce(&mut ChartConfig)) {
    let mut store = session.write_channel(Chan::Chart(tab));
    let mut next = store.chart(tab);
    edit(&mut next);
    store.set_chart(tab, next);
}

/// What "nothing on this channel" reads as, per channel. Both are real choices, not empty
/// states: charting against the row index is what "X: none" means, and a chart with no series
/// column is the ordinary case.
const ROW_INDEX: &str = "Row index";
const NO_SERIES: &str = "None";

/// One row of the legend: what a colour on the plot means, and — where the row is pressable —
/// which series pressing it hides.
#[derive(Clone, PartialEq)]
pub struct LegendEntry {
    pub swatch: Color,
    pub label: String,
    /// A slice's share of the whole. `None` for a series, whose values are on the axis
    /// already.
    pub detail: Option<String>,
    /// The series name a press toggles, or `None` for an inert row (a pie's slices — see
    /// [`legend`](super::legend)).
    pub series: Option<String>,
    /// Whether that series is currently hidden, so the row can read as struck out.
    pub hidden: bool,
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

    /// This config with `name` hidden or shown again — the ordinary press.
    fn toggled(&self, name: &str) -> ChartConfig {
        self.with(|c| {
            if let Some(at) = c.hidden.iter().position(|held| held == name) {
                c.hidden.remove(at);
            } else {
                c.hidden.push(name.to_string());
            }
        })
    }

    /// This config with **only** `name` showing among the series this result has — ⌥-press. On
    /// the sole visible series it shows them all again instead, so the gesture is its own undo
    /// and a user cannot ⌥-press their way into a chart with nothing on it.
    ///
    /// **It edits the set, it does not replace it.** A name for a series this result has no
    /// column for is kept on both paths, the same as an ordinary press keeps it: the config
    /// holds intent and is never pruned against the result in hand
    /// ([`ChartConfig::hidden`](strata_model::ChartConfig::hidden)), so a column that comes
    /// back brings the user's choice with it. Rebuilding the field from the current legend
    /// would quietly spend every such choice, and would make the two legend gestures disagree
    /// about the same field.
    fn isolated(&self, name: &str) -> ChartConfig {
        let here: Vec<String> = self
            .legend
            .iter()
            .filter_map(|entry| entry.series.clone())
            .collect();
        let alone = here
            .iter()
            .all(|other| other == name || self.config.hidden.contains(other));
        self.with(|c| {
            // Only the names this result can show are touched; everything else stays put.
            c.hidden.retain(|held| !here.contains(held));
            if !alone {
                c.hidden
                    .extend(here.iter().filter(|other| *other != name).cloned());
            }
        })
    }

    /// The series menu: no split, then every column that can carry one beside the current X.
    /// A mark that **requires** its series (a heatmap's matrix) offers no "None" row — an
    /// unsplittable heatmap is unreachable rather than reported.
    fn series_choices(&self, options: Vec<String>, required: bool) -> Vec<Choice> {
        let mut choices = Vec::new();
        if !required {
            choices.push(Choice {
                label: NO_SERIES.to_string(),
                selected: self.encoding.series.is_none(),
                next: self.with(|c| c.series = None),
                keep_open: false,
            });
        }
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

    /// The four band-role encoders (Chart 10), in strip order — Q1, Q3, LOWER, UPPER —
    /// each `None` where the mark does not read the role.
    fn band_encoders(&self, mark: ChartMark) -> [Option<Encoder>; 4] {
        let bounds = reads_bounds(mark);
        let quartiles = reads_quartiles(mark);
        let Encoding {
            q1, q3, y_lo, y_hi, ..
        } = &self.encoding;
        [
            quartiles
                .then(|| self.measure_role("Q1", q1, [q3, y_lo, y_hi], |c, name| c.q1 = name))
                .map(|encoder| encoder.key("q1")),
            quartiles
                .then(|| self.measure_role("Q3", q3, [q1, y_lo, y_hi], |c, name| c.q3 = name))
                .map(|encoder| encoder.key("q3")),
            bounds
                .then(|| self.measure_role("LOWER", y_lo, [y_hi, q1, q3], |c, name| c.y_lo = name))
                .map(|encoder| encoder.key("lower")),
            bounds
                .then(|| self.measure_role("UPPER", y_hi, [y_lo, q1, q3], |c, name| c.y_hi = name))
                .map(|encoder| encoder.key("upper")),
        ]
    }

    /// One measure-role encoder (LOWER / UPPER / Q1 / Q3, Chart 10): a "None" row to clear
    /// the role, then the measures this result offers, minus the Y and the other band roles
    /// — a bound that collides with another edge is unreachable, not reported. The None row
    /// is what keeps the trigger's own "None" reachable: without it a mispick could only be
    /// undone by switching marks, a state the control shows but cannot return to. It also
    /// means the row never empties, so the section stays on the strip while its refusal
    /// names it.
    fn measure_role(
        &self,
        label: &'static str,
        current: &Option<String>,
        others: [&Option<String>; 3],
        set: fn(&mut ChartConfig, Option<String>),
    ) -> Encoder {
        let mut options = vec![Choice {
            label: NO_SERIES.to_string(),
            selected: current.is_none(),
            next: self.with(|c| set(c, None)),
            keep_open: false,
        }];
        options.extend(
            y_options(&self.roles)
                .into_iter()
                .filter(|name| self.encoding.ys.first().map(String::as_str) != Some(name.as_str()))
                .filter(|name| {
                    !others
                        .iter()
                        .any(|other| other.as_deref() == Some(name.as_str()))
                })
                .map(|name| Choice {
                    selected: current.as_deref() == Some(name.as_str()),
                    next: self.with(|c| set(c, Some(name.clone()))),
                    label: name,
                    keep_open: false,
                }),
        );
        Encoder {
            tab: self.tab,
            label,
            current: current.clone().unwrap_or_else(|| NO_SERIES.to_string()),
            options,
            key: DiffKey::None,
        }
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
                // The channel is the same one everywhere; what it *means* is the mark's — a
                // heatmap's measure is its colour and a box plot's is its median, and an
                // eyebrow saying "Y AXIS" over either would name the wrong thing.
                label: match mark {
                    ChartMark::Heatmap => "VALUE (COLOR)",
                    ChartMark::Box => "MEDIAN",
                    _ => "Y AXIS",
                },
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
                // A heatmap's series channel is its second category axis, and the strip
                // says so — "SERIES (COLOR)" would promise the categorical ramp over a
                // mark whose colour is the value.
                label: if mark == ChartMark::Heatmap {
                    "Y AXIS"
                } else {
                    "SERIES (COLOR)"
                },
                current: self
                    .encoding
                    .series
                    .clone()
                    .unwrap_or_else(|| NO_SERIES.to_string()),
                options: self.series_choices(series_options, series_required(mark)),
                key: DiffKey::None,
            }
            .key("series")
        });

        // A heatmap reads X then its second category then the value, so the sections come
        // in that order — for every other mark the value axis stays second.
        let (second, third) = if mark == ChartMark::Heatmap {
            (series, y)
        } else {
            (y, series)
        };

        // The band roles (Chart 10): a band's bounds, a box plot's quartiles and whiskers.
        let [q1, q3, lower, upper] = self.band_encoders(mark);

        // The engine does the binning, so this one is part of the read — a new count is a new
        // entry rather than a repaint. Only a histogram has bins to count.
        let bins = (mark == ChartMark::Histogram).then_some(BinsField {
            tab: self.tab,
            current: self.encoding.bins,
        });

        // The sort is a view transform over the settled rows (spec §6) — offered only for the
        // marks whose data has an order to permute.
        let sort = sortable(mark).then(|| SortToggle {
            tab: self.tab,
            current: self.encoding.sort,
            config: self.config.clone(),
        });

        // As is the value axis's scale, offered only where the mark plots position rather than
        // extent (`config::log_axis`).
        let scale = log_axis(mark).then(|| ScaleToggle {
            tab: self.tab,
            log: self.encoding.log_y,
            config: self.config.clone(),
        });

        // The trendline is a scatter's own overlay (Chart 11) — its fit is a separate read
        // keyed by the encoded columns, so this toggle repaints and never re-reads the points.
        let trend = trendable(mark).then(|| TrendToggle {
            tab: self.tab,
            on: self.encoding.trend,
            config: self.config.clone(),
        });

        // ⌥ isolates a series, and a pointer event carries no modifiers (AGENTS.md §3) — so
        // the strip mirrors the key state and each row reads it at press time.
        let mut alt = use_state(|| false);
        let legend = (!self.legend.is_empty()).then(|| {
            let mut section = rect()
                .width(Size::fill())
                .vertical()
                .spacing(TILE_GAP)
                .child(Eyebrow::new("LEGEND").color(theme.label_color));
            for (nth, entry) in self.legend.iter().enumerate() {
                let next = entry
                    .series
                    .as_deref()
                    .map(|name| (self.toggled(name), self.isolated(name)));
                section = section.child(
                    LegendRow {
                        entry: entry.clone(),
                        tab: self.tab,
                        next,
                        alt,
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
            // **Mirroring ⌥, and re-reading it from every event.** A key-up lost while the
            // window is unfocused would otherwise leave the modifier stuck on, so the flag is
            // taken from each event's own `modifiers` rather than only toggled by the ⌥ key —
            // any keystroke at all resynchronizes it (FREYA_UI.md, "reset defensively").
            .on_global_key_down(move |e: Event<KeyboardEventData>| {
                let held = matches!(e.key, Key::Named(NamedKey::Alt))
                    || e.modifiers.contains(Modifiers::ALT);
                alt.set_if_modified(held);
            })
            .on_global_key_up(move |e: Event<KeyboardEventData>| {
                let held = !matches!(e.key, Key::Named(NamedKey::Alt))
                    && e.modifiers.contains(Modifiers::ALT);
                alt.set_if_modified(held);
            })
            .child(
                ScrollView::new().child(
                    rect()
                        .width(Size::fill())
                        .vertical()
                        .padding(STRIP_PADDING)
                        .spacing(SECTION_GAP)
                        .child(tiles)
                        .maybe_child(x)
                        .maybe_child(second)
                        .maybe_child(third)
                        .maybe_child(q1)
                        .maybe_child(q3)
                        .maybe_child(lower)
                        .maybe_child(upper)
                        .maybe_child(bins)
                        .maybe_child(sort)
                        .maybe_child(scale)
                        .maybe_child(trend)
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

/// The **value axis's scale**: linear or logarithmic, as one segmented pill.
///
/// A display transform in the sort's class — flipping it repaints and never re-reads. Its own
/// component for the same reason [`SortToggle`] is: it is only shown for some marks, and a hook
/// taken behind a condition is a hook count that changes between renders.
#[derive(PartialEq)]
struct ScaleToggle {
    tab: TabId,
    log: bool,
    config: ChartConfig,
}

impl Component for ScaleToggle {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let session = use_radio::<SessionState, Chan>(Chan::Chart(self.tab));
        let tab = self.tab;

        let mut pill = SegmentedToggle::new();
        for (label, title, log) in [
            ("Linear", "Linear value axis", false),
            ("Log", "Logarithmic value axis", true),
        ] {
            let next = ChartConfig {
                log_y: log,
                ..self.config.clone()
            };
            pill = pill.child(
                ToggleSegment::text(label)
                    .title(title)
                    .selected(self.log == log)
                    .on_press(move |_| commit(session, tab, next.clone())),
            );
        }

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(LABEL_GAP)
            .child(Eyebrow::new("SCALE").color(theme.label_color))
            .child(pill)
    }
}

/// The scatter's **trendline**: off, or a dashed least-squares fit over the points
/// (Chart 11).
///
/// A display-tier control in [`ScaleToggle`]'s shape — the fit is its own engine read keyed by
/// the encoded columns, so flipping this never re-reads the points — and its own component for
/// the same reason: it is only shown for a scatter, and a hook taken behind a condition is a
/// hook count that changes between renders.
#[derive(PartialEq)]
struct TrendToggle {
    tab: TabId,
    on: bool,
    config: ChartConfig,
}

impl Component for TrendToggle {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let session = use_radio::<SessionState, Chan>(Chan::Chart(self.tab));
        let tab = self.tab;

        let mut pill = SegmentedToggle::new();
        // "On", not "Linear": the scale pill beside this one already has a segment of that
        // name, and two controls answering to one word is a mispress waiting to happen.
        for (label, title, on) in [
            ("Off", "No trendline", false),
            ("On", "Least-squares trendline", true),
        ] {
            let next = ChartConfig {
                trend: on,
                ..self.config.clone()
            };
            pill = pill.child(
                ToggleSegment::text(label)
                    .title(title)
                    .selected(self.on == on)
                    .on_press(move |_| commit(session, tab, next.clone())),
            );
        }

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(LABEL_GAP)
            .child(Eyebrow::new("TRENDLINE").color(theme.label_color))
            .child(pill)
    }
}

/// How many characters the bin box will hold — the cap's own width, so a slip of the keyboard
/// cannot type an order of magnitude the read would throw away. It is a **digit** bound, not
/// the cap itself: over 200 the box still accepts 201..=999, and the clamp plus the blur echo
/// are what make those honest. Derived from [`MAX_BINS`] rather than written out, because two
/// places is where they drift.
fn bins_digits() -> usize {
    MAX_BINS.to_string().len()
}

/// A histogram's **bin count** — a box, empty for the engine's own `√n` choice.
///
/// Follows [`NumberField`](crate::components::form::NumberField)'s contract, including the half
/// that matters most here: it **publishes on every keystroke and normalizes its box when the
/// box is left** (AGENTS.md §3). Without the second half the field is the very thing this
/// change made [`MAX_BINS`] public to prevent — a control showing one number over a chart
/// drawn with another.
///
/// One deliberate difference from `NumberField`: an **empty box is a value here**, not a
/// half-typed number. "Auto" is what most histograms want, so it has to be reachable by
/// clearing the field rather than only by deleting the tab's state — which is why this owns its
/// buffer and reports `Option<u16>` instead of reusing that component.
///
/// Its own component so the strip's hook count is fixed whether or not a histogram is showing
/// (the [`SortToggle`] precedent).
#[derive(PartialEq)]
struct BinsField {
    tab: TabId,
    current: Option<u16>,
}

impl Component for BinsField {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let session = use_radio::<SessionState, Chan>(Chan::Chart(self.tab));
        let mut text = use_state({
            let seed = self.current.map(|n| n.to_string()).unwrap_or_default();
            move || seed
        });
        // What was last committed. In state rather than captured, because a
        // `use_side_effect` closure is built once and a captured comparison would freeze at
        // the first render — the field could then never be typed back to where it started
        // (`NumberField`'s own note).
        let mut reported = use_state({
            let seed = self.current;
            move || seed
        });
        // The box's id is ours, so the effect below can see it lose focus.
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);

        let tab = self.tab;
        use_side_effect(move || {
            let raw = text.read().trim().to_string();
            // An empty box **and** an unparseable one are both Auto: the box is cleared on the
            // way to typing a new number, and a keystroke that has not settled on a count yet
            // must not leave the chart on the old one. Parsed wide and *then* clamped — a
            // `u16` parse would answer `None` for anything over 65 535, so a fat-fingered
            // count would read as Auto instead of as the cap.
            let bins = raw
                .parse::<u64>()
                .ok()
                .map(|n| n.clamp(1, MAX_BINS as u64) as u16)
                .filter(|_| !raw.is_empty());
            if bins == *reported.peek() {
                return;
            }
            reported.set(bins);
            // Through `edit`, not `commit`: this closure is built once, so a captured config
            // would be the one from the first render and typing a bin count would undo every
            // encoder change made since the histogram was picked.
            edit(session, tab, |config| config.bins = bins);
        });

        // Leaving the box is when it is made to agree with what it reported. `reported` is
        // peeked, not read: this must wake on focus alone, or the echo would land mid-keystroke
        // and overwrite what is being typed.
        use_side_effect(move || {
            if focus() == Focus::Not {
                let echo = (*reported.peek()).map_or_else(String::new, |n| n.to_string());
                text.set_if_modified(echo);
            }
        });

        rect()
            .width(Size::fill())
            .vertical()
            .spacing(LABEL_GAP)
            .child(Eyebrow::new("BINS").color(theme.label_color))
            .child(
                ValueField::new(text)
                    .placeholder("Auto")
                    .max_len(bins_digits())
                    .a11y_id(a11y_id)
                    .width(Size::px(CONTROL_WIDTH)),
            )
    }
}

/// One legend row: the swatch a mark is drawn in, the name it carries, and — for a slice —
/// its share, which a pie has no axis to read off.
///
/// A row over a **series** is also the control that hides it: a press toggles, ⌥-press
/// isolates. A hidden row keeps its swatch and its slot and goes dim, because the swatch is
/// what says which colour comes back.
#[derive(PartialEq)]
struct LegendRow {
    entry: LegendEntry,
    tab: TabId,
    /// What a press and an ⌥-press commit, or `None` for an inert row. Resolved by the strip,
    /// which is where the encoding rules are — this row knows only how to write one.
    next: Option<(ChartConfig, ChartConfig)>,
    /// Whether ⌥ is down, mirrored by the strip (pointer events carry no modifiers).
    alt: State<bool>,
    key: DiffKey,
}

impl KeyExt for LegendRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

/// How far a hidden legend row is faded — enough to read as off, not so far as to be
/// unreadable, since it is also the control that brings the series back.
const HIDDEN_ALPHA: u8 = 110;

impl Component for LegendRow {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let session = use_radio::<SessionState, Chan>(Chan::Chart(self.tab));
        let mut hovered = use_state(|| false);

        let dim = |color: Color| {
            if self.entry.hidden {
                color.with_a(HIDDEN_ALPHA)
            } else {
                color
            }
        };
        let tab = self.tab;
        let alt = self.alt;
        let next = self.next.clone();
        let pressable = next.is_some();

        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(8.)
            // A row that does nothing must not light up under the pointer.
            .maybe(pressable && hovered(), |el| {
                el.background(theme.tile_active_background)
            })
            .maybe(pressable, |el| {
                el.corner_radius(4.)
                    .padding((2., 4.))
                    .on_pointer_enter(move |_| hovered.set(true))
                    .on_pointer_leave(move |_| hovered.set(false))
                    .on_press(move |_| {
                        if let Some((toggled, isolated)) = &next {
                            let next = if *alt.peek() { isolated } else { toggled };
                            commit(session, tab, next.clone());
                        }
                    })
            })
            .child(
                rect()
                    .width(Size::px(SWATCH))
                    .height(Size::px(SWATCH))
                    .corner_radius(2.)
                    .background(dim(self.entry.swatch)),
            )
            .child(
                // Flexing and ellipsizing, so a long column name gives up its own width
                // rather than pushing the share off the strip (AGENTS.md §3).
                Caption::new(self.entry.label.clone())
                    .color(dim(theme.legend_color))
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
    use freya_testing::prelude::{KeyboardEventName, PlatformEvent};
    use freya_testing::TestingRunner;
    use strata_core::engine::column_info;
    use strata_core::theme::load;
    use strata_model::{Axis, ChartData, ChartSeries, ColumnInfo, Origin};

    use super::super::config::resolve;
    use super::super::paint::Dress;
    use super::*;
    use crate::components::form::FIELD_HEIGHT;
    use crate::components::typography::scale;
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
            // The legend the body would hand over, off a stand-in result whose series are the
            // encoding's own Y columns — built through `legend` rather than by hand, so what
            // a row does here is what a row does in the app.
            let data = ChartData::Table {
                axis: Axis {
                    labels: vec!["a".into()],
                    positions: None,
                },
                series: encoding
                    .ys
                    .iter()
                    .map(|name| ChartSeries {
                        name: name.clone(),
                        values: vec![Some(1.)],
                    })
                    .collect(),
            };
            let key = super::super::legend(
                &data,
                encoding.mark,
                &Dress::new(
                    &get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart"),
                    &scale(),
                ),
                &encoding.hidden,
            );
            rect()
                .expanded()
                .child(ControlStrip::new(tab, config, encoding, roles).legend(key))
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

    /// **The bin count is the one control here that is part of the read**, so it is offered
    /// only for the mark that has bins — and clearing the box is Auto rather than a number.
    #[test]
    fn the_bin_count_is_offered_for_a_histogram_and_an_empty_box_is_auto() {
        let (mut runner, session) = runner();
        settle(&mut runner);
        assert!(!shows(&runner, "BINS"), "no other mark bins anything");

        click_text(&mut runner, "Histogram");
        assert!(shows(&runner, "BINS"), "{:?}", texts(&runner));
        assert_eq!(config(&session).bins, None, "unset until it is typed");

        type_into_bins(&mut runner, "40");
        assert_eq!(config(&session).bins, Some(40));

        // Emptying the box is Auto, not zero and not the last number typed.
        type_into_bins(&mut runner, "");
        assert_eq!(config(&session).bins, None);

        // Past the read's own cap the box commits the cap, so what it accepts and what the
        // engine counts are the same number.
        type_into_bins(&mut runner, "5000");
        assert_eq!(config(&session).bins, Some(MAX_BINS as u16));
    }

    /// **The bin count commits from a `use_side_effect`, so it must read the config rather
    /// than capture it.** That closure is built once (AGENTS.md §3); a captured config is the
    /// one from the first render, and typing a count would silently undo every encoder change
    /// made since the histogram was picked.
    #[test]
    fn a_bin_count_does_not_undo_the_encoder_edits_made_after_it_was_mounted() {
        let (mut runner, session) = runner();
        settle(&mut runner);
        click_text(&mut runner, "Histogram");

        // An edit on another channel *after* the field mounted — a histogram takes one Y, so
        // its trigger reads the single column and the pick replaces it.
        click_text(&mut runner, "revenue");
        click_text(&mut runner, "cost");
        assert_eq!(
            config(&session).ys.as_deref(),
            Some(["cost".to_string()].as_ref())
        );

        // …survives a bin count typed afterwards.
        type_into_bins(&mut runner, "12");
        let stored = config(&session);
        assert_eq!(stored.bins, Some(12));
        assert_eq!(
            stored.ys.as_deref(),
            Some(["cost".to_string()].as_ref()),
            "the bin count wrote back a stale config"
        );
    }

    /// Type `text` into the bins box, replacing whatever is there.
    ///
    /// The box is found from its own eyebrow's laid-out position rather than by element type —
    /// an `Input` renders as a `paragraph` of spans, which is not what `texts` walks — so this
    /// still encodes no pixel offsets of its own.
    fn type_into_bins(runner: &mut TestingRunner, text: &str) {
        let label = runner
            .find(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == "BINS")
                    .map(|_| node.layout().area)
            })
            .unwrap_or_else(|| panic!("no BINS section: {:?}", texts(runner)));
        let point = (
            f64::from(label.min_x() + 20.),
            f64::from(label.max_y() + LABEL_GAP + FIELD_HEIGHT / 2.),
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        settle(runner);
        // Cleared a character at a time — the box holds at most the cap's three digits, and a
        // select-all chord would be testing the fork's editable rather than this control.
        for _ in 0..6 {
            runner.press_key(Key::Named(NamedKey::Backspace));
        }
        for ch in text.chars() {
            runner.press_key(Key::Character(ch.to_string()));
        }
        settle(runner);
    }

    /// **The scale is offered where the mark plots position, not extent.** A bar is read as
    /// area from a baseline, and a log axis has none.
    #[test]
    fn the_scale_toggle_follows_the_mark_and_writes_a_repaint() {
        let (mut runner, session) = runner();
        settle(&mut runner);
        // The derived mark over this schema is a line, which plots position.
        assert!(shows(&runner, "SCALE"), "{:?}", texts(&runner));
        click_text(&mut runner, "Log");
        assert!(config(&session).log_y);
        click_text(&mut runner, "Linear");
        assert!(!config(&session).log_y);

        // A bar is read as area from a baseline, so the control goes — and the preference is
        // kept, so it comes back with the mark that can draw it.
        click_text(&mut runner, "Log");
        click_text(&mut runner, "Bar");
        assert!(!shows(&runner, "SCALE"), "{:?}", texts(&runner));
        assert!(config(&session).log_y, "the config still holds it");
        click_text(&mut runner, "Line");
        assert!(shows(&runner, "SCALE"));
    }

    /// **A heatmap's strip names its channels for what they mean on a matrix** — the second
    /// category is its Y axis and the measure is its colour — and the required series offers
    /// no "None" row, so an unsplittable heatmap is unreachable.
    #[test]
    fn a_heatmap_renames_its_channels_and_requires_the_second_category() {
        let (mut runner, session) = runner();
        settle(&mut runner);

        click_text(&mut runner, "Heatmap");
        assert_eq!(config(&session).mark, Some(ChartMark::Heatmap));
        let seen = texts(&runner);
        for expected in ["X AXIS", "Y AXIS", "VALUE (COLOR)"] {
            assert!(seen.contains(&expected.to_string()), "{seen:?}");
        }
        assert!(
            !seen.contains(&"SERIES (COLOR)".to_string()),
            "a heatmap's colour is its value, not a series ramp: {seen:?}"
        );
        assert!(
            seen.contains(&"country".to_string()),
            "the derived second category: {seen:?}"
        );

        // Open the second-category picker: no "None" row to press.
        click_text(&mut runner, "country");
        assert!(
            !shows(&runner, NO_SERIES),
            "a required series offers no None: {:?}",
            texts(&runner)
        );
    }

    /// **A box plot's strip offers its five roles by name** — and the band roles never leak
    /// onto a mark that does not read them.
    #[test]
    fn the_band_roles_are_offered_exactly_where_the_mark_reads_them() {
        let (mut runner, session) = runner();
        settle(&mut runner);
        for absent in ["LOWER", "UPPER", "Q1", "Q3"] {
            assert!(!shows(&runner, absent), "{absent} on a line");
        }

        click_text(&mut runner, "Box");
        let seen = texts(&runner);
        for expected in ["MEDIAN", "Q1", "Q3", "LOWER", "UPPER"] {
            assert!(seen.contains(&expected.to_string()), "{seen:?}");
        }

        click_text(&mut runner, "Band");
        let seen = texts(&runner);
        for expected in ["Y AXIS", "LOWER", "UPPER"] {
            assert!(seen.contains(&expected.to_string()), "{seen:?}");
        }
        for absent in ["Q1", "Q3"] {
            assert!(
                !seen.contains(&absent.to_string()),
                "a band has no quartiles: {seen:?}"
            );
        }

        // A pick writes the config's own field. The first unset trigger reads "None" and is
        // LOWER's; its menu offers the measures minus the Y, which leaves `cost`.
        click_text(&mut runner, NO_SERIES);
        click_text(&mut runner, "cost");
        assert_eq!(config(&session).y_lo.as_deref(), Some("cost"));
    }

    /// **The trendline is offered only for a scatter, and it writes a repaint** — the fit is
    /// its own read keyed by the encoded columns, so the toggle never reaches the points'
    /// `ChartQuery`. The preference is kept across marks like the scale's is.
    #[test]
    fn the_trendline_toggle_follows_the_scatter_and_keeps_the_choice() {
        let (mut runner, session) = runner();
        settle(&mut runner);
        assert!(
            !shows(&runner, "TRENDLINE"),
            "a line has no fit to offer: {:?}",
            texts(&runner)
        );

        click_text(&mut runner, "Scatter");
        assert!(shows(&runner, "TRENDLINE"), "{:?}", texts(&runner));
        click_text(&mut runner, "On");
        assert!(config(&session).trend);

        // Another mark drops the control and keeps the choice, so it comes back.
        click_text(&mut runner, "Bar");
        assert!(!shows(&runner, "TRENDLINE"), "{:?}", texts(&runner));
        assert!(config(&session).trend, "the config still holds it");
        click_text(&mut runner, "Scatter");
        assert!(shows(&runner, "TRENDLINE"));
        click_text(&mut runner, "Off");
        assert!(!config(&session).trend);
    }

    /// **A legend row is the control that hides its series**, and ⌥ isolates. The modifier
    /// comes from the strip's own key mirroring, because a pointer event carries none.
    #[test]
    fn a_legend_press_hides_a_series_and_alt_press_isolates_it() {
        let (mut runner, session) = runner();
        settle(&mut runner);
        // The default encoding plots both measures, so the legend has two rows.
        assert!(shows(&runner, "LEGEND"), "{:?}", texts(&runner));

        click_legend(&mut runner, "cost");
        assert_eq!(config(&session).hidden, ["cost"]);
        click_legend(&mut runner, "cost");
        assert!(
            config(&session).hidden.is_empty(),
            "the press is its own undo"
        );

        // ⌥ down: isolate `revenue`, so everything else goes.
        runner.send_event(alt(KeyboardEventName::KeyDown));
        settle(&mut runner);
        click_legend(&mut runner, "revenue");
        assert_eq!(config(&session).hidden, ["cost"]);

        // ⌥-pressing the sole visible series restores them all rather than emptying the chart.
        click_legend(&mut runner, "revenue");
        assert!(config(&session).hidden.is_empty());

        // ⌥ up: the ordinary toggle is back.
        runner.send_event(alt(KeyboardEventName::KeyUp));
        settle(&mut runner);
        click_legend(&mut runner, "revenue");
        assert_eq!(config(&session).hidden, ["revenue"]);
    }

    /// **⌥-press edits the hidden set; it does not rebuild it.** A name for a series this
    /// result has no column for is intent the config keeps — pruning it would spend a choice
    /// the next result might honour, and would make the two legend gestures disagree about the
    /// same field.
    #[test]
    fn alt_press_keeps_hidden_names_this_result_has_no_series_for() {
        let (mut runner, session) = runner();
        settle(&mut runner);
        // A name from an earlier result, which this one cannot answer.
        {
            let tab = *session.peek().order.first().expect("the one tab");
            let mut station = session;
            let mut store = station.write_channel(Chan::Chart(tab));
            let mut stale = store.chart(tab);
            stale.hidden = vec!["margin".into()];
            store.set_chart(tab, stale);
        }
        settle(&mut runner);

        runner.send_event(alt(KeyboardEventName::KeyDown));
        settle(&mut runner);
        click_legend(&mut runner, "revenue");
        assert_eq!(
            config(&session).hidden,
            ["margin", "cost"],
            "the stale name survived the isolate"
        );

        // …and the ⌥-press that restores everything leaves it alone too.
        click_legend(&mut runner, "revenue");
        assert_eq!(config(&session).hidden, ["margin"]);
    }

    fn alt(name: KeyboardEventName) -> PlatformEvent {
        PlatformEvent::Keyboard {
            name,
            key: Key::Named(NamedKey::Alt),
            code: Code::AltLeft,
            modifiers: if name == KeyboardEventName::KeyDown {
                Modifiers::ALT
            } else {
                Modifiers::empty()
            },
        }
    }

    /// Press the legend row for `name`. The legend's labels are the series names, and they are
    /// the *last* runs of that text in the strip — the Y encoder's trigger and its menu rows
    /// carry the same words.
    fn click_legend(runner: &mut TestingRunner, name: &str) {
        let areas: Vec<_> = runner.find_many(|node, element| {
            Label::try_downcast(element)
                .filter(|l| l.text == name)
                .map(|_| node.layout().area)
        });
        let area = areas
            .last()
            .unwrap_or_else(|| panic!("no legend row {name:?}: {:?}", texts(runner)));
        let point = (
            f64::from(area.min_x() + area.width() / 2.),
            f64::from(area.min_y() + area.height() / 2.),
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        settle(runner);
    }

    /// **Every control here changes what is drawn, and none of them leaves the chart.** The
    /// strip is the encoding and nothing else: the design's Aggregate toggle is absent because
    /// the chart aggregates nothing, and the press that would have written that `GROUP BY` into
    /// a new tab was cut with it (spec §8). Pinned because it is an absence, and an absence is
    /// the kind of decision that gets quietly undone.
    #[test]
    fn the_strip_offers_no_control_that_leaves_the_chart() {
        let (mut runner, _) = runner();
        settle(&mut runner);

        let seen = texts(&runner);
        for absent in ["Aggregate in SQL", "Aggregate", "GROUP BY"] {
            assert!(
                !seen.iter().any(|t| t == absent),
                "the strip offers {absent:?}: {seen:?}"
            );
        }
    }
}
