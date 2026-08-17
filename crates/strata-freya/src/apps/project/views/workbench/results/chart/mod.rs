//! The results pane's **Chart** body (Rz2, `docs/CHART_SPEC.md`): the shared results toolbar
//! over a control strip and a plot of the current result.
//!
//! **What it charts** is the snapshot the grid is paging, never the source files. The read is
//! [`ChartSpec`], a freya-query entry on the page read's terms, carrying the tab's persisted
//! [`ChartConfig`] resolved against the **result's own schema** — see [`config`], which owns both
//! the defaults an unset channel takes and the one `encode` site.
//!
//! **Three of the strip's controls never reach that request**: [`sort`], the legend's hidden set
//! ([`hide`]) and the log value axis are view transforms over the settled answer, so flipping them
//! repaints rather than re-reading. The bin count is the one that does reach it, because the engine
//! is what counts. The two data transforms apply **sorted, then hidden** — hiding blanks a series'
//! values and `ByYDesc` sorts on the first series, so hiding first would let a legend press
//! reshuffle the category axis.
//!
//! **What each state renders.** A drawable answer becomes a [`ChartCanvas`]; everything else
//! becomes a [`Notice`] in its place. That last group is not politeness: without it the pane is
//! *blank*, which is indistinguishable from a bug. [`notice`] is the one place that decides, so a
//! state cannot be drawable in one reading and blank in another. The two engine refusals name
//! aggregating in SQL, which is the user's own `GROUP BY`.
//!
//! A drawable answer can still be *unreadable*, which is a third thing: past [`CROWDED`] categories
//! the axis has more labels than it can draw, so the canvas takes a non-blocking [`Banner`] above
//! it and renders underneath unaltered. The same banner carries the log axis's fallback
//! ([`log_fallback`]) — a display preference never costs the user their chart.

mod axis;
mod capture;
mod config;
mod hide;
mod marks;
mod paint;
#[cfg(test)]
mod preview;
mod sort;
mod strip;

use std::rc::Rc;

use freya::components::{define_theme, get_theme, CircularLoader};
use freya::prelude::*;
use freya::query::{use_query, QueryStateData};
use freya::radio::use_radio;
use strata_arrow::config::display_subset;
use strata_core::util::fmt_int;
use strata_model::{
    CapUnit, ChartData, ChartMark, ChartQuery, ChartSeries, ColumnInfo, SnapshotId, TabId,
};

pub use self::capture::ChartCapture;

use self::config::{encode, resolve, Roles};
use self::paint::{ChartCanvas, Dress, Frame};
use self::strip::{ControlStrip, LegendEntry};
use super::find::FindState;
use super::shape::{ShapeSeed, ShapeTarget};
use super::toolbar::ResultsToolbar;
use crate::apps::export::ExportLaunch;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{ChartSpec, TrendSpec};
use crate::apps::project::state::{Chan, LogCtx, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_1, R_3, SP_3, SP_4, SP_6};
use crate::components::typography::{scale, Prose, Title};
use crate::state::{use_config, ConfigChan};

define_theme!(
    %[no_ext]
    %[component]
    pub Chart {
        %[fields]
        /// The canvas pane, and what separates a pie's slices.
        background: Color,
        /// The control strip's raised surface.
        panel_background: Color,
        /// The rule between the strip and the canvas.
        border_fill: Color,
        /// A strip section's eyebrow.
        label_color: Color,
        tile_color: Color,
        tile_border_fill: Color,
        /// A mark tile's edge while hovered: the emphasized outline every hovered card in the
        /// app wears, rather than an alpha computed off the selected edge.
        tile_hover_border_fill: Color,
        tile_active_background: Color,
        tile_active_border_fill: Color,
        tile_active_color: Color,
        /// The plot's own furniture.
        grid_fill: Color,
        axis_fill: Color,
        tick_color: Color,
        legend_color: Color,
        /// The prose that stands in for a plot the data cannot support.
        note_color: Color,
        /// The tinted box a non-blocking warning sits in — the Export window's banner, same
        /// tone (its glyph and text take the sheet's semantic `warning`, which is app-wide).
        warning_background: Color,
        warning_border_fill: Color,
        /// The categorical ramp, in order (spec §4 — a series is named by column or by value,
        /// and coloured by position).
        series_1: Color,
        series_2: Color,
        series_3: Color,
        series_4: Color,
        series_5: Color,
        series_6: Color,
        series_7: Color,
        series_8: Color,
        series_9: Color,
        series_10: Color,
        /// The heatmap's sequential ramp (Chart 10): a cell's value blends the low end
        /// toward the high. Distinct from the categorical series ramp on purpose — a
        /// sequential scale reads as one hue getting stronger.
        heat_low: Color,
        heat_high: Color,
    }
);

/// The chart body: the shared toolbar on top, then the control strip beside the plot.
#[derive(PartialEq)]
pub struct ChartView {
    tab: TabId,
    find: FindState,
    /// What Download would export — the same run, whichever body is showing it (P4-10).
    export: Option<ExportLaunch>,
    /// The result to chart. `None` when the run produced no rows, so nothing was
    /// materialized and there is nothing to read.
    snapshot: Option<SnapshotId>,
    /// The result's schema, which is what the encoding is derived from.
    columns: Vec<ColumnInfo>,
    /// What the toolbar's Shape press composes over (Chart 09) — arriving unseeded; this
    /// body seeds it from the resolved encoding, which only it knows.
    shape: Option<ShapeTarget>,
}

impl ChartView {
    pub fn new(
        tab: TabId,
        find: FindState,
        export: Option<ExportLaunch>,
        snapshot: Option<SnapshotId>,
        columns: Vec<ColumnInfo>,
    ) -> Self {
        Self {
            tab,
            find,
            export,
            snapshot,
            columns,
            shape: None,
        }
    }

    /// What the Shape press composes over (see the field).
    pub fn shape(mut self, shape: Option<ShapeTarget>) -> Self {
        self.shape = shape;
        self
    }
}

impl Component for ChartView {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let engine = use_consume::<EngineCtx>();
        let log = use_consume::<LogCtx>();
        let roles = Roles::of(&self.columns);

        let session = use_radio::<SessionState, Chan>(Chan::Chart(self.tab));
        let config = session.read().chart(self.tab);
        let encoding = resolve(&config, &roles);
        let mark_now = encoding.mark;
        let encoded = encode(&encoding, &roles);

        let settings = use_config(ConfigChan::Settings);
        let display = display_subset(&settings.read().settings.engine);
        let spec = ChartSpec {
            snapshot: self.snapshot.unwrap_or(SnapshotId(0)),
            query: encoded.clone().unwrap_or(ChartQuery::Histogram {
                col: String::new(),
                bins: None,
            }),
            display,
        };
        let readable = self.snapshot.is_some() && encoded.is_ok();
        let chart = use_query(spec.query(&engine, readable));

        let points_settled = matches!(
            &*chart.read().state(),
            QueryStateData::Settled {
                res: Ok(ChartData::Points(_)),
                ..
            }
        );
        let fit_wanted = readable && encoding.trend && points_settled;
        let trend_spec = TrendSpec {
            snapshot: self.snapshot.unwrap_or(SnapshotId(0)),
            x: encoding.x.clone().unwrap_or_default(),
            y: encoding.ys.first().cloned().unwrap_or_default(),
        };
        let trend = use_query(trend_spec.query(&engine, fit_wanted));
        let fit = fit_wanted
            .then(|| match &*trend.read().state() {
                QueryStateData::Settled { res: Ok(fit), .. } => *fit,
                _ => None,
            })
            .flatten();

        let typography = scale();
        let dress = Dress::new(&theme, &typography);
        let mut key: Vec<LegendEntry> = Vec::new();
        let mut snap: Option<ChartCapture> = None;
        let body: Element = match (self.snapshot, &encoded) {
            (None, _) => Notice::new(
                "Nothing to chart",
                "This query returned no rows.".to_string(),
                theme.note_color,
            )
            .into(),
            (_, Err((title, body))) => Notice::new(title, body.clone(), theme.note_color).into(),
            (Some(_), Ok(_)) => match &*chart.read().state() {
                QueryStateData::Pending | QueryStateData::Loading { .. } => rect()
                    .width(Size::fill())
                    .height(Size::fill())
                    .center()
                    .child(CircularLoader::new().size(22.))
                    .into(),
                QueryStateData::Settled { res: Err(err), .. } => {
                    Notice::new("The chart could not be read", err.clone(), theme.note_color).into()
                }
                QueryStateData::Settled { res: Ok(data), .. } => {
                    let sorted = sort::sorted(data.clone(), encoding.sort);
                    let all_hidden = hide::all_hidden(&sorted, &encoding.hidden);
                    let data = hide::applied(sorted, &encoding.hidden);
                    let reason = notice(&data, mark_now, all_hidden);
                    if reason.is_none() || all_hidden {
                        key = legend(&data, mark_now, &dress, &encoding.hidden);
                    }
                    match reason {
                        Some((title, body)) => Notice::new(title, body, theme.note_color).into(),
                        None => {
                            let fallback = encoding.log_y.then(|| log_fallback(&data)).flatten();
                            let banner = fallback.map(str::to_string).or_else(|| crowded(&data));
                            let frame = Rc::new(Frame {
                                data,
                                mark: mark_now,
                                log_y: encoding.log_y && fallback.is_none(),
                                trend: fit,
                                dress,
                            });
                            snap = Some(ChartCapture::new(Rc::clone(&frame), log));
                            let plot = ChartCanvas::new(frame);
                            rect()
                                .width(Size::fill())
                                .height(Size::fill())
                                .vertical()
                                .content(Content::Flex)
                                .spacing(SP_3)
                                .maybe_child(banner.map(Banner::new))
                                .child(
                                    rect()
                                        .width(Size::fill())
                                        .height(Size::flex(1.))
                                        .child(plot),
                                )
                                .into()
                        }
                    }
                }
            },
        };

        rect()
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .child(
                ResultsToolbar::new(self.tab, self.find, self.export.clone())
                    .copy_image(snap)
                    .shape(self.shape.clone().map(|target| {
                        ShapeTarget {
                            seed: Some(ShapeSeed {
                                groups: encoding
                                    .x
                                    .iter()
                                    .filter(|x| roles.categories().contains(x))
                                    .chain(encoding.series.iter())
                                    .cloned()
                                    .collect(),
                                measures: encoding.ys.clone(),
                            }),
                            ..target
                        }
                    })),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .horizontal()
                    .content(Content::Flex)
                    .background(theme.background)
                    .child(ControlStrip::new(self.tab, config, encoding, roles).legend(key))
                    .child(canvas_pane(body)),
            )
    }
}

/// The pane the plot — or the notice standing in for it — is laid out in: everything the strip
/// leaves, down to nothing.
///
/// **No floor.** This is the middle pane, and the middle pane collapses to nothing and clips —
/// `PANE_BODY_MIN_W` is the *side* panels' rule (`views::shell`), where a floor keeps the
/// resize handle grabbable. Nothing here needs grabbing. What the collapse must not do is let
/// the content reflow into the gap, which is what [`Notice`] sizes against.
///
/// It lives here rather than inline because the preview harness lays out this same pane, and a
/// second copy of it is a copy that goes stale.
fn canvas_pane(body: impl IntoElement) -> impl IntoElement {
    rect()
        .width(Size::flex(1.))
        .height(Size::fill())
        .padding((SP_3, SP_4))
        .overflow(Overflow::Clip)
        .child(body)
}

/// Why this answer is not a chart, or `None` when it is one.
///
/// Three groups, and the surface treats them alike because the user does: the engine's two
/// **refusals** (spec §7 — both carry no data at all, so there is no half-drawn chart to put a
/// message beside), a shape a mark cannot honestly draw, and an answer that simply has nothing
/// in it. The last group matters because the alternative is not a worse chart, it is a *blank
/// pane* — no axes, no message, indistinguishable from a bug.
///
/// **The refusals name the fix in prose, and there is no control behind it.** Aggregating is
/// the answer to both of them, and it is the user's own `GROUP BY` — V1 says so and stops
/// there. Writing that query *for* the user was built and cut (spec §8): the capability is
/// well precedented, but every tool that has it puts it in a menu or a surface of its own
/// rather than beside the encoders, and the chart-side aggregation it was standing in for is
/// the thing actually worth revisiting.
fn notice(data: &ChartData, mark: ChartMark, all_hidden: bool) -> Option<(&'static str, String)> {
    const NOTHING: &str = "Nothing to chart";
    match data {
        ChartData::OverCap { unit, cap } => {
            let unit = match unit {
                CapUnit::Rows => "rows",
                CapUnit::Points => "points",
            };
            Some((
                "Too much data to chart honestly",
                format!(
                    "This result has more than {} {unit}. Aggregate it in SQL so the chart \
                     draws a compact result.",
                    fmt_int(*cap as u64)
                ),
            ))
        }
        ChartData::Duplicates { x, series } => Some((
            "More than one row per category",
            format!(
                "Splitting '{x}' by '{series}' puts several rows in one cell. Aggregate them in \
                 SQL so each category has one value."
            ),
        )),
        ChartData::Bins(bins) if bins.is_empty() => Some((
            NOTHING,
            "This column has no finite values to put in a bin.".to_string(),
        )),
        ChartData::Bins(bins) if bins.iter().all(|bin| bin.hi <= bin.lo) => Some((
            "Every value is the same",
            "This column has one distinct value, so there is no range to spread over bins."
                .to_string(),
        )),
        ChartData::Points(points) if points.is_empty() => Some((
            NOTHING,
            "No row of this result has a finite value on both axes.".to_string(),
        )),
        ChartData::Table { axis, .. } if axis.labels.is_empty() => {
            Some((NOTHING, "This result has no rows.".to_string()))
        }
        ChartData::Table { series, .. } if series.is_empty() => {
            Some((NOTHING, "No column is being plotted.".to_string()))
        }
        ChartData::Table { series, .. }
            if mark == ChartMark::Heatmap && marks::heat_bounds(series).is_none() =>
        {
            Some((NOTHING, "Every cell of this matrix is empty.".to_string()))
        }
        ChartData::Table { series, .. }
            if mark == ChartMark::Band && !complete_rows(series, config::BAND_YS) =>
        {
            Some((
                NOTHING,
                "No row of this result has the centre and both bounds.".to_string(),
            ))
        }
        ChartData::Table { series, .. }
            if mark == ChartMark::Box && !complete_rows(series, config::BOX_YS) =>
        {
            Some((
                NOTHING,
                "No category of this result has all five measures.".to_string(),
            ))
        }
        ChartData::Table { .. } if all_hidden => Some((
            "Every series is hidden",
            "Press a legend entry to show it again.".to_string(),
        )),
        ChartData::Table { series, .. } if mark == ChartMark::Pie => pie_notice(series),
        _ => None,
    }
}

/// Whether any index has a present, finite value in **each** of the first `need` series —
/// the "is there anything to draw" question for the marks that read several roles by
/// position (a band's centre and bounds, a box plot's five).
fn complete_rows(series: &[ChartSeries], need: usize) -> bool {
    if series.len() < need {
        return false;
    }
    let len = series[0].values.len();
    (0..len).any(|i| {
        series[..need].iter().all(|one| {
            one.values
                .get(i)
                .copied()
                .flatten()
                .is_some_and(f64::is_finite)
        })
    })
}

/// Why the values this chart is drawing cannot sit on a logarithmic axis, or `None` when they
/// can — and the text is the banner's, because a fallback the user is not told about is a
/// control that silently does nothing.
///
/// **A log axis never refuses.** Both arms draw linearly under the non-blocking `Banner`: this
/// is a *display* preference, and a preference must never cost the user their chart.
///
/// Two reasons, and they are genuinely different facts:
///
/// - A logarithm has no zero and no negative half, so a value at or below zero has no place on
///   one. **A histogram's empty bins are not such a value**: a zero count has no bar on either
///   axis — a zero-height rectangle paints nothing — so treating it as a blocker would take the
///   log axis away from exactly the long-tailed distributions it exists for, and a count cannot
///   be negative, so zero is the only case that arm has to answer.
/// - A span whose *ratio* overflows ([`axis::log_span`]) is one plotters cannot derive key
///   points for without hanging the render thread. Its own note has the arithmetic.
fn log_fallback(data: &ChartData) -> Option<&'static str> {
    match data {
        ChartData::Table { series, .. } => log_reason(
            || {
                series
                    .iter()
                    .flat_map(|one| one.values.iter().flatten())
                    .copied()
            },
            true,
        ),
        ChartData::Points(points) => log_reason(|| points.iter().map(|p| p.y), true),
        ChartData::Bins(bins) => log_reason(|| bins.iter().map(|bin| bin.count as f64), false),
        ChartData::OverCap { .. } | ChartData::Duplicates { .. } => None,
    }
}

/// [`log_fallback`]'s two questions over one mark's values, told apart.
///
/// `values` is a **factory** rather than an iterator because each question needs its own walk,
/// and handing [`axis::log_span`] a `&mut dyn Iterator` — which is already an `Iterator` — only
/// tempted an intermediate `Vec` of every plotted value.
///
/// `zero_blocks` says whether a zero is a value this mark would have drawn. It is false only
/// for a histogram's counts (see [`log_fallback`]).
fn log_reason<I: Iterator<Item = f64>>(
    values: impl Fn() -> I,
    zero_blocks: bool,
) -> Option<&'static str> {
    const NON_POSITIVE: &str = "Values at or below zero are shown on a linear axis.";
    const TOO_WIDE: &str = "This data spans too many orders of magnitude for a log axis.";

    if zero_blocks && values().any(|v| v.is_finite() && v <= 0.) {
        return Some(NON_POSITIVE);
    }
    if !values().any(|v| v.is_finite() && v > 0.) {
        return None;
    }
    axis::log_span(values()).is_none().then_some(TOO_WIDE)
}

/// How many categories an axis draws before its labels stop being readable (spec §7).
const CROWDED: usize = 60;

/// What the banner says over a crowded axis, or `None` while the axis is readable.
///
/// **`axis.labels.len()`, not a distinct count**: the number is already in hand, so the nudge
/// costs no second query — and it is the honest figure anyway, because the categories on the
/// axis are exactly what the chart drew. Only a table has an axis; a scatter and a histogram
/// have nothing this could count.
fn crowded(data: &ChartData) -> Option<String> {
    let ChartData::Table { axis, .. } = data else {
        return None;
    };
    (axis.labels.len() > CROWDED).then(|| {
        format!(
            "{} categories on the axis. Only some labels are drawn.",
            fmt_int(axis.labels.len() as u64)
        )
    })
}

/// What each colour on the plot means, in the order the plot draws them — the strip's legend.
///
/// Only the marks that draw in **more than one** colour have anything to key: a scatter and a
/// histogram are one colour by construction, so a legend over them would be a swatch beside the
/// only thing on screen.
///
/// The pie's rows come out of [`marks::pie_slices`], the same walk the wedges are drawn from,
/// so the legend cannot name a colour the plot gave to a different category.
/// A row also carries what pressing it means: the series name to toggle, or `None` where the
/// row is inert. A pie's rows are the inert ones — hiding a slice would silently recompute
/// every remaining percentage, which is the chart telling a story the data does not.
fn legend(data: &ChartData, mark: ChartMark, dress: &Dress, hidden: &[String]) -> Vec<LegendEntry> {
    let ChartData::Table { axis, series } = data else {
        return Vec::new();
    };
    if mark == ChartMark::Pie {
        let Some(one) = series.first() else {
            return Vec::new();
        };
        let drawn = marks::pie_slices(one);
        let total: f64 = drawn.iter().map(|(_, value)| value).sum();
        return drawn
            .iter()
            .enumerate()
            .map(|(nth, (i, value))| LegendEntry {
                swatch: dress.slice(nth),
                label: axis.labels.get(*i).cloned().unwrap_or_default(),
                detail: (total > 0.).then(|| format!("{:.0}%", value / total * 100.)),
                series: None,
                hidden: false,
            })
            .collect();
    }
    if mark == ChartMark::Heatmap {
        let Some((lo, hi)) = marks::heat_bounds(series) else {
            return Vec::new();
        };
        let stops: Vec<(f32, f64)> = if hi > lo {
            vec![(0., lo), (0.5, f64::midpoint(lo, hi)), (1., hi)]
        } else {
            vec![(0.5, lo)]
        };
        return stops
            .into_iter()
            .map(|(t, value)| LegendEntry {
                swatch: dress.heat_at(t),
                label: axis::readout(value),
                detail: None,
                series: None,
                hidden: false,
            })
            .collect();
    }
    if mark == ChartMark::Band {
        return series
            .first()
            .map(|one| LegendEntry {
                swatch: dress.series(0),
                label: one.name.clone(),
                detail: None,
                series: None,
                hidden: false,
            })
            .into_iter()
            .collect();
    }
    if mark == ChartMark::Box {
        return Vec::new();
    }
    let toggles = config::hideable(mark);
    series
        .iter()
        .enumerate()
        .map(|(i, one)| LegendEntry {
            swatch: dress.series(i),
            detail: None,
            series: toggles.then(|| one.name.clone()),
            hidden: hidden.contains(&one.name),
            label: one.name.clone(),
        })
        .collect()
}

/// What a pie cannot do with the values it was handed.
///
/// A **negative** value has no wedge: dropping one would quietly change the total every
/// percentage is read against, which is the silent truncation spec §1.4 rules out — so the pie
/// refuses rather than drawing a plausible lie. A missing or zero value is different in kind: a
/// zero-area slice is arithmetic, not a truncation, and leaving it out changes nothing on
/// screen. It is only worth saying when *nothing* is left.
fn pie_notice(series: &[ChartSeries]) -> Option<(&'static str, String)> {
    let one = series.first()?;
    let values = || one.values.iter().flatten().filter(|v| v.is_finite());
    if values().any(|v| *v < 0.) {
        return Some((
            "A pie cannot show negative values",
            format!(
                "'{}' goes below zero, and a slice has no way to be negative. Chart it as a bar \
                 instead.",
                one.name
            ),
        ));
    }
    if !values().any(|v| *v > 0.) {
        return Some((
            "Nothing to chart",
            format!("Every value of '{}' is zero or missing.", one.name),
        ));
    }
    None
}

/// The tile a notice leads with, the width its copy wraps at, and its inset (canvas
/// `Strata.dc.html`, the chart pane's guardrail overlay).
const TILE: f32 = 46.;
const TILE_RADIUS: f32 = R_3;
const COPY_WIDTH: f32 = 380.;
const NOTICE_PAD: f32 = SP_6;

/// What stands in for the plot when there is nothing honest to draw: a glyph tile over the
/// condition, centred in the pane the plot would have filled.
#[derive(PartialEq)]
struct Notice {
    title: &'static str,
    body: String,
    color: Color,
}

impl Notice {
    fn new(title: &'static str, body: String, color: Color) -> Self {
        Self { title, body, color }
    }
}

impl Component for Notice {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .child(
                rect()
                    .width(Size::px(COPY_WIDTH + 2. * NOTICE_PAD))
                    .vertical()
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
                    .padding(NOTICE_PAD)
                    .child(
                        rect()
                            .width(Size::px(TILE))
                            .height(Size::px(TILE))
                            .corner_radius(TILE_RADIUS)
                            .center()
                            .background(theme.panel_background)
                            .border(Border::new().width(1.).fill(theme.tile_border_fill))
                            .child(Icon::new(IconName::MarkBar).color(self.color).size(22.)),
                    )
                    .child(Title::new(self.title).color(self.color))
                    .child(
                        Prose::new(self.body.clone())
                            .color(self.color)
                            .width(Size::fill())
                            .wrap()
                            .align(TextAlign::Center),
                    ),
            )
    }
}

/// A **non-blocking** warning across the top of the canvas: the chart still renders beneath it,
/// because what it says is that the plot is crowded, not that it is wrong.
///
/// It wears the Export window's banner rather than a second warning tone — the tinted box from
/// the `chart` theme, the glyph and text from the sheet's semantic `warning`, which is app-wide
/// and must stay so.
#[derive(PartialEq)]
struct Banner {
    message: String,
}

impl Banner {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl Component for Banner {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let warning = use_theme().read().colors().warning;
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .padding((SP_3, SP_4))
            .corner_radius(R_1)
            .background(theme.warning_background)
            .border(Border::new().width(1.).fill(theme.warning_border_fill))
            .child(Icon::new(IconName::Warning).size(14.).color(warning))
            .child(Prose::new(self.message.clone()).color(warning).wrap())
    }
}

#[cfg(test)]
mod tests {
    use freya_testing::TestingRunner;
    use strata_core::theme::load;
    use strata_model::Axis;

    use super::config::ROWS_CAP;
    use super::*;
    use crate::theme::strata_theme;

    fn series(name: &str, values: &[Option<f64>]) -> ChartSeries {
        ChartSeries {
            name: name.into(),
            values: values.to_vec(),
        }
    }

    /// A ramp whose entries are told apart at a glance, so a colour assertion names a colour.
    fn dress() -> Dress {
        Dress {
            background: Color::BLACK,
            grid: Color::BLACK,
            axis: Color::BLACK,
            tick: Color::BLACK,
            series: [
                Color::RED,
                Color::GREEN,
                Color::BLUE,
                Color::CYAN,
                Color::MAGENTA,
                Color::YELLOW,
                Color::WHITE,
                Color::GRAY,
                Color::DARK_GRAY,
                Color::LIGHT_GRAY,
            ],
            heat: (Color::BLACK, Color::WHITE),
            label: ("mono".into(), 10.),
        }
    }

    fn table(values: &[Option<f64>]) -> ChartData {
        ChartData::Table {
            axis: Axis {
                labels: values.iter().map(|_| "x".to_string()).collect(),
                positions: None,
            },
            series: vec![series("amount", values)],
        }
    }

    /// The refusals say what they counted, in the app's own figures, and they name the fix —
    /// which is the user's own `GROUP BY`, in prose, because V1 offers no control behind it.
    #[test]
    fn the_over_cap_refusal_names_the_cap_the_way_every_other_count_is_written() {
        let (title, body) = notice(
            &ChartData::OverCap {
                unit: CapUnit::Rows,
                cap: ROWS_CAP,
            },
            ChartMark::Bar,
            false,
        )
        .expect("over cap refuses");
        assert_eq!(title, "Too much data to chart honestly");
        assert!(body.contains("1,000 rows"), "{body}");
        assert!(body.contains("Aggregate it in SQL"), "{body}");
    }

    /// **The banner counts the labels already in hand, and only a table has any.** It is a
    /// nudge over a chart that still draws, so it never becomes a reason not to draw one.
    #[test]
    fn a_crowded_axis_is_a_banner_and_an_uncrowded_one_is_nothing() {
        let axis_of = |n: usize| ChartData::Table {
            axis: Axis {
                labels: (0..n).map(|i| i.to_string()).collect(),
                positions: None,
            },
            series: vec![series("v", &vec![Some(1.); n])],
        };
        assert_eq!(crowded(&axis_of(CROWDED)), None, "at the threshold");
        assert_eq!(
            crowded(&axis_of(CROWDED + 1)).as_deref(),
            Some("61 categories on the axis. Only some labels are drawn.")
        );
        assert_eq!(crowded(&ChartData::Bins(Vec::new())), None);
        assert_eq!(crowded(&ChartData::Points(Vec::new())), None);
    }

    /// **A collapsed pane clips the notice; it never reflows it.** The middle pane gives its
    /// width away entirely and goes to nothing — that is the shell's collapse model, and it is
    /// deliberately *not* the side panels' floored one (`PANE_BODY_MIN_W` is theirs, and it
    /// exists to keep a resize handle grabbable; nothing here needs grabbing). What the
    /// collapse must not do is squeeze the copy: prose with less room than a word wraps **one
    /// character per line**, a column of letters down the pane, which reads as a rendering
    /// fault rather than a narrow window. So the notice is a fixed block and the pane cuts it
    /// off.
    ///
    /// Laid out for real rather than asserted on the builder — the reflow this guards against
    /// is a parent's doing, so only a parent can prove it gone.
    #[test]
    fn a_collapsed_pane_clips_the_notice_rather_than_reflowing_it() {
        let app = || {
            use_init_theme(|| strata_theme(&load("midnight")));
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .child(
                    rect()
                        .width(Size::px(strip::STRIP_WIDTH))
                        .height(Size::fill()),
                )
                .child(canvas_pane(Notice::new(
                    "Too much data to chart honestly",
                    "This result has more than 24 rows. Aggregate it in SQL so the chart draws \
                     a compact result."
                        .to_string(),
                    Color::WHITE,
                )))
        };
        let (mut runner, ()) = TestingRunner::new(app, (260., 600.).into(), |_| {}, 1.);
        for _ in 0..4 {
            runner.sync_and_update();
        }

        let widths: Vec<f32> = runner.find_many(|node: freya_testing::TestingNode, element| {
            Label::try_downcast(element).map(|_| node.layout().area.width())
        });
        assert!(!widths.is_empty(), "the notice rendered no text at all");
        assert!(
            widths.contains(&COPY_WIDTH),
            "no text run kept the copy width — the pane reflowed the notice instead of \
             clipping it: {widths:?}"
        );
    }

    /// **An answer with nothing in it is a message, never a blank pane.** Each of these draws
    /// no axes at all — the mark bails out before `ChartBuilder` — so without a notice the user
    /// gets an empty rectangle that looks exactly like a bug.
    #[test]
    fn an_answer_with_nothing_to_draw_says_so_rather_than_painting_a_blank_pane() {
        for (data, mark) in [
            (ChartData::Bins(Vec::new()), ChartMark::Histogram),
            (ChartData::Points(Vec::new()), ChartMark::Scatter),
            (table(&[]), ChartMark::Bar),
            (table(&[None, Some(0.)]), ChartMark::Pie),
        ] {
            assert!(
                notice(&data, mark, false).is_some(),
                "{mark:?} over {data:?} would have painted nothing at all"
            );
        }
    }

    /// The legend is the one place a colour is explained, so its rows have to be the rows the
    /// plot actually drew — for a pie that means the *surviving* slices, in draw order, keyed
    /// off the same walk `pie` uses.
    #[test]
    fn the_legend_keys_the_colours_the_plot_drew() {
        let dress = dress();

        let two = ChartData::Table {
            axis: Axis {
                labels: vec!["a".into(), "b".into()],
                positions: None,
            },
            series: vec![
                series("revenue", &[Some(1.), Some(2.)]),
                series("cost", &[Some(3.), Some(4.)]),
            ],
        };
        let key = legend(&two, ChartMark::Bar, &dress, &[]);
        assert_eq!(
            key.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["revenue", "cost"]
        );
        assert_eq!((key[0].swatch, key[1].swatch), (Color::RED, Color::GREEN));
        assert!(key[0].detail.is_none(), "a series reads off the axis");

        let pie = ChartData::Table {
            axis: Axis {
                labels: vec!["a".into(), "skipped".into(), "gone".into(), "c".into()],
                positions: None,
            },
            series: vec![series("n", &[Some(3.), Some(0.), None, Some(1.)])],
        };
        let key = legend(&pie, ChartMark::Pie, &dress, &[]);
        assert_eq!(
            key.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!((key[0].swatch, key[1].swatch), (Color::RED, Color::GREEN));
        assert_eq!(key[0].detail.as_deref(), Some("75%"));

        assert!(legend(
            &ChartData::Bins(Vec::new()),
            ChartMark::Histogram,
            &dress,
            &[]
        )
        .is_empty());
        assert!(legend(
            &ChartData::Points(Vec::new()),
            ChartMark::Scatter,
            &dress,
            &[]
        )
        .is_empty());
    }

    /// **Every series hidden is a message, for the same reason every other empty state is** —
    /// and it is the only one the user can undo from the control that caused it. It sits
    /// *after* the empty shapes, because "this result has no rows" is the truer thing to say
    /// about a result with no rows.
    #[test]
    fn hiding_every_series_says_so_rather_than_painting_a_blank_pane() {
        let (title, body) = notice(&table(&[Some(1.)]), ChartMark::Bar, true)
            .expect("a chart with nothing showing says so");
        assert_eq!(title, "Every series is hidden");
        assert!(body.contains("legend"), "{body}");

        assert_eq!(notice(&table(&[Some(1.)]), ChartMark::Bar, false), None);
        assert_eq!(notice(&table(&[None, None]), ChartMark::Bar, false), None);

        assert_eq!(
            notice(&table(&[]), ChartMark::Bar, true).map(|(title, _)| title),
            Some("Nothing to chart")
        );
    }

    /// **A log axis never refuses; it says why it could not and draws linearly.** The rule is
    /// about values a logarithm has no place for — and a histogram's empty bins are not among
    /// them, because a zero-count bin paints nothing on either axis and blocking on one would
    /// take the log scale away from the long tails it exists for.
    #[test]
    fn a_value_a_logarithm_has_no_place_for_falls_back_to_a_linear_axis() {
        use strata_model::{ChartBin, ChartPoint};

        assert_eq!(log_fallback(&table(&[Some(1.), Some(400.)])), None);
        assert_eq!(
            log_fallback(&table(&[Some(1.), None])),
            None,
            "a gap is not a zero"
        );
        for values in [[Some(1.), Some(0.)], [Some(1.), Some(-4.)]] {
            let why = log_fallback(&table(&values)).expect("no place on a log axis");
            assert!(why.contains("linear axis"), "{why}");
        }

        assert_eq!(
            log_fallback(&ChartData::Points(vec![ChartPoint { x: -5., y: 5. }])),
            None,
            "only the value axis is logarithmic"
        );
        assert!(log_fallback(&ChartData::Points(vec![ChartPoint { x: 5., y: 0. }])).is_some());

        let bin = |count| ChartBin {
            lo: 0.,
            hi: 1.,
            count,
        };
        assert_eq!(
            log_fallback(&ChartData::Bins(vec![bin(900), bin(0), bin(1)])),
            None,
            "an empty bin must not cost a histogram its log axis"
        );
    }

    /// **A span too wide for a log axis is a *hang*, not a bad-looking axis** — plotters
    /// derives a bold-tick count from the bounds' ratio, and an overflowed ratio saturates
    /// that count to `usize::MAX`, which its key-point loop then counts down one at a time on
    /// the render thread. It falls back like any other value a logarithm cannot take, and the
    /// banner says which of the two happened rather than blaming zeros that are not there.
    #[test]
    fn a_span_whose_ratio_overflows_falls_back_and_says_which_reason_it_was() {
        let wide = table(&[Some(1e-300), Some(1e300)]);
        let why = log_fallback(&wide).expect("a 600-decade span is not a log axis");
        assert!(why.contains("orders of magnitude"), "{why}");
        assert!(!why.contains("zero"), "the wrong reason: {why}");
        assert_eq!(axis::log_span([1e-300, 1e300].into_iter()), None);
    }

    /// **A result with nothing positive in it gets no banner at all.** `log_span` answers
    /// `None` for an unusable ratio *and* for a result it found no positive value in, and
    /// reporting the ratio's message for the second told a user whose every value was NULL
    /// that their data spanned too many orders of magnitude. An all-NULL table is not caught
    /// by `notice` — it has labels and it has series — so the plot really is drawn under it.
    #[test]
    fn a_result_with_no_positive_value_gets_no_log_banner_rather_than_the_wrong_one() {
        use strata_model::ChartBin;

        let empty = table(&[None, None]);
        assert_eq!(notice(&empty, ChartMark::Line, false), None, "it is drawn");
        assert_eq!(log_fallback(&empty), None);

        let zeroed = table(&[Some(0.), None]);
        assert!(log_fallback(&zeroed).is_some_and(|why| why.contains("zero")));

        let bin = |count| ChartBin {
            lo: 0.,
            hi: 1.,
            count,
        };
        assert_eq!(log_fallback(&ChartData::Bins(vec![bin(0), bin(0)])), None);
    }

    /// **The notice's fix has to still be on screen.** Hiding the last visible series replaces
    /// the plot with "press a legend entry to show it again" — so the legend has to be built
    /// on that path too. Built only on the drawable path it vanished exactly when its own
    /// message named it, and `hidden` is persisted, so the tab carried the dead end across a
    /// re-run and a restart.
    ///
    /// It is also the **only** notice that keeps one: every other one draws no plot and offers
    /// no way back through the legend, so keying colours beside it would name colours nothing
    /// on screen is wearing. Both halves of that rule are exercised here.
    ///
    /// Laid out for real: the failure was a *branch*, and only mounting the strip proves the
    /// rows are there.
    #[test]
    fn hiding_the_last_series_leaves_the_legend_that_undoes_it_on_screen() {
        use datafusion::arrow::datatypes::{DataType, Field};
        use freya::radio::RadioStation;
        use freya_testing::TestingRunner;
        use strata_arrow::column_info;
        use strata_model::{ChartConfig, Origin};

        use super::strip::ControlStrip;
        use crate::theme::strata_theme;

        let mut session = SessionState::default();
        let tab = session.open_named("chart", "SELECT 1".into(), Origin::Scratch);
        session.set_chart(
            tab,
            ChartConfig {
                mark: Some(ChartMark::Bar),
                hidden: vec!["amount".into()],
                ..ChartConfig::default()
            },
        );
        let app = move || {
            use_init_theme(|| strata_theme(&load("midnight")));
            let store = use_radio::<SessionState, Chan>(Chan::Chart(tab));
            let config = store.read().chart(tab);
            let columns = [column_info(&Field::new("amount", DataType::Float64, true))];
            let roles = Roles::of(&columns);
            let encoding = resolve(&config, &roles);
            let data = hide::applied(table(&[Some(1.), Some(2.)]), &encoding.hidden);
            let all_hidden = hide::all_hidden(&data, &encoding.hidden);
            let reason = notice(&data, encoding.mark, all_hidden);
            let key = if reason.is_none() || all_hidden {
                legend(&data, encoding.mark, &dress(), &encoding.hidden)
            } else {
                Vec::new()
            };
            let body = reason.map(|(title, text)| Notice::new(title, text, Color::WHITE));
            rect()
                .expanded()
                .horizontal()
                .child(ControlStrip::new(tab, config, encoding, roles).legend(key))
                .maybe_child(body)
        };
        let (mut runner, _) = TestingRunner::new(
            app,
            (700., 900.).into(),
            move |r| r.provide_root_context(|| RadioStation::<SessionState, Chan>::create(session)),
            1.,
        );
        for _ in 0..4 {
            runner.sync_and_update();
        }

        let seen: Vec<String> =
            runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()));
        assert!(
            seen.iter().any(|t| t == "Every series is hidden"),
            "the notice did not render: {seen:?}"
        );
        assert!(
            seen.iter().any(|t| t == "LEGEND") && seen.iter().any(|t| t == "amount"),
            "the notice names a legend that is not on screen: {seen:?}"
        );

        let refused = table(&[Some(3.), Some(-1.)]);
        let reason = notice(&refused, ChartMark::Pie, false).expect("a pie refuses a negative");
        assert_eq!(reason.0, "A pie cannot show negative values");
        assert!(
            !legend(&refused, ChartMark::Pie, &dress(), &[]).is_empty(),
            "the legend itself still has rows to offer — the body is what withholds them"
        );
    }

    /// **A mark whose legend cannot un-hide never has the hidden set applied to it.** A pie's
    /// Y is an ordinary measure that a bar may have hidden earlier, and a pie's rows are inert
    /// — so honouring the set there would empty the chart with no control on screen to bring
    /// it back. `hideable` is the gate, and it is the caller's, so `hide` stays a pure
    /// transform.
    #[test]
    fn a_mark_whose_legend_cannot_unhide_ignores_the_hidden_set() {
        let hidden = ["amount".to_string()];
        let data = table(&[Some(3.), Some(1.)]);

        assert!(config::hideable(ChartMark::Bar));
        assert!(hide::all_hidden(&data, &hidden));

        for mark in [ChartMark::Pie, ChartMark::Scatter, ChartMark::Histogram] {
            assert!(!config::hideable(mark), "{mark:?}");
        }
        assert_eq!(hide::applied(data.clone(), &[]), data);
        assert!(!hide::all_hidden(&data, &[]));
        assert_eq!(notice(&data, ChartMark::Pie, false), None);
    }

    /// The legend says which colour is off and what pressing a row means — and a pie's rows
    /// mean nothing, because hiding a slice would silently recompute every percentage.
    #[test]
    fn a_legend_row_carries_the_series_a_press_toggles_and_a_pie_s_carries_none() {
        let dress = dress();
        let two = ChartData::Table {
            axis: Axis {
                labels: vec!["a".into()],
                positions: None,
            },
            series: vec![series("revenue", &[Some(1.)]), series("cost", &[Some(3.)])],
        };
        let key = legend(&two, ChartMark::Bar, &dress, &["cost".to_string()]);
        assert_eq!(key[0].series.as_deref(), Some("revenue"));
        assert!(!key[0].hidden);
        assert_eq!(key[1].series.as_deref(), Some("cost"));
        assert!(key[1].hidden, "the hidden row says so");
        assert_eq!((key[0].swatch, key[1].swatch), (Color::RED, Color::GREEN));

        let pie = ChartData::Table {
            axis: Axis {
                labels: vec!["a".into(), "b".into()],
                positions: None,
            },
            series: vec![series("n", &[Some(3.), Some(1.)])],
        };
        for row in legend(&pie, ChartMark::Pie, &dress, &["a".to_string()]) {
            assert_eq!(row.series, None, "a pie's rows are inert");
            assert!(!row.hidden);
        }
    }

    /// A pie refuses a negative value rather than dropping it: every percentage on the chart is
    /// read against a total, and quietly leaving a row out of that total is the silent
    /// truncation spec §1.4 rules out. A zero or a NULL is not the same thing — a zero-area
    /// slice is arithmetic, and the rest of the pie is still true.
    #[test]
    fn a_pie_refuses_negative_values_but_draws_around_zeroes_and_gaps() {
        let (title, body) = notice(&table(&[Some(3.), Some(-1.)]), ChartMark::Pie, false)
            .expect("a negative slice has no wedge");
        assert_eq!(title, "A pie cannot show negative values");
        assert!(body.contains("'amount'"), "{body}");

        assert_eq!(
            notice(&table(&[Some(3.), Some(0.), None]), ChartMark::Pie, false),
            None
        );
        assert_eq!(
            notice(&table(&[Some(3.), Some(-1.)]), ChartMark::Bar, false),
            None
        );
    }
}
