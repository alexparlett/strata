//! The results pane's **Chart** body (Rz2, `docs/CHART_SPEC.md`): the shared results toolbar
//! over a control strip and a plot of the current result.
//!
//! ## What it charts
//!
//! The snapshot the grid is paging — never the source files (spec §1.1). The read is
//! [`ChartSpec`], a freya-query entry on the page read's terms, and the request it carries is
//! the tab's persisted [`ChartConfig`] resolved against the **result's own schema** — see
//! [`config`], which owns both halves: the defaults an unset channel takes, and the one
//! `encode` site that turns the resolved encoding into a [`ChartQuery`].
//!
//! The **sort** never reaches that request. It is a view transform over the settled answer
//! (spec §6, [`sort`]), so flipping it permutes what is already in hand instead of paying for
//! a second read of the same rows.
//!
//! ## What each state renders
//!
//! A drawable answer becomes a [`ChartCanvas`]; everything else becomes a [`Notice`] in place
//! of the canvas — an encoding the columns cannot satisfy, a read that failed, the engine's two
//! refusals (over the row cap, and a pivot that found two rows in one cell), a shape the chosen
//! mark cannot draw honestly, and an answer with nothing in it at all. That last group is not
//! politeness: without it the pane is *blank* — no axes, no message — which is indistinguishable
//! from a bug. [`notice`] is the one place that decides, so a state cannot be drawable in one
//! reading and blank in another.
//!
//! The two engine refusals are stated but not yet *offered a way out*: the *Aggregate in SQL*
//! scaffold that turns them into an editable `GROUP BY` tab is Chart 04's, and it lands as the
//! CTA under these same messages (AGENTS.md §5).

mod axis;
mod config;
mod marks;
mod paint;
#[cfg(test)]
mod preview;
mod sort;
mod strip;

use freya::components::{define_theme, get_theme, CircularLoader};
use freya::prelude::*;
use freya::query::{use_query, QueryStateData};
use freya::radio::use_radio;
use strata_core::engine::config::display_subset;
use strata_core::util::fmt_int;
use strata_model::{
    CapUnit, ChartData, ChartMark, ChartQuery, ChartSeries, ColumnInfo, SnapshotId, TabId,
};

use self::config::{encode, resolve, Roles};
use self::paint::{ChartCanvas, Dress, Frame};
use self::strip::{ControlStrip, LegendEntry};
use super::find::FindState;
use super::toolbar::ResultsToolbar;
use crate::apps::export::ExportLaunch;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::ChartSpec;
use crate::apps::project::state::{Chan, SessionState};
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
    }
);

// ---- the body ----

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
        }
    }
}

impl Component for ChartView {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let engine = use_consume::<EngineCtx>();
        let roles = Roles::of(&self.columns);

        // The tab's encoding, on its own channel — so the strip's edits re-chart this body
        // and wake nothing else, and so they survive a re-run and a restart. What is *drawn*
        // is that intent resolved against this result's columns: unset channels take the
        // schema's defaults, and a column this result no longer has falls back to one rather
        // than reaching the read.
        let session = use_radio::<SessionState, Chan>(Chan::Chart(self.tab));
        let config = session.read().chart(self.tab);
        let encoding = resolve(&config, &roles);
        let mark_now = encoding.mark;
        let encoded = encode(&encoding, &roles);

        // The engine's display config rides in the key: axis labels render through it, and it
        // changes without a restart (see `ChartSpec`). Subscribed, not peeked — a format
        // change in Settings has to re-label the chart there and then.
        let settings = use_config(ConfigChan::Settings);
        let display = display_subset(&settings.read().settings.engine);
        let spec = ChartSpec {
            snapshot: self.snapshot.unwrap_or(SnapshotId(0)),
            // A disabled read never reaches the engine, so the placeholder is only ever a
            // cache key that nothing runs.
            query: encoded.clone().unwrap_or(ChartQuery::Histogram {
                col: String::new(),
                bins: None,
            }),
            display,
        };
        let readable = self.snapshot.is_some() && encoded.is_ok();
        let chart = use_query(spec.query(&engine, readable));

        let typography = scale();
        let dress = Dress::new(&theme, &typography);
        // What the plot's colours mean, resolved beside the body that draws them so the two
        // read the same settled answer — the strip renders it, because that is what scrolls.
        let mut key: Vec<LegendEntry> = Vec::new();
        let body: Element = match (self.snapshot, &encoded) {
            (None, _) => Notice::new(
                "Nothing to chart",
                "This query returned no rows.".to_string(),
                theme.note_color,
            )
            .into(),
            (_, Err((title, body))) => {
                Notice::new(title, (*body).to_string(), theme.note_color).into()
            }
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
                    // The one clone per render, with the strip's order applied on the way
                    // through — the notice, the legend and the frame all read the rows in
                    // the order they will be drawn in.
                    let data = sort::sorted(data.clone(), encoding.sort);
                    match notice(&data, mark_now) {
                        Some((title, body)) => Notice::new(title, body, theme.note_color).into(),
                        None => {
                            key = legend(&data, mark_now, &dress);
                            ChartCanvas::new(Frame {
                                data,
                                mark: mark_now,
                                dress: dress.clone(),
                            })
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
            .child(ResultsToolbar::new(
                self.tab,
                self.find,
                self.export.clone(),
            ))
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .horizontal()
                    .content(Content::Flex)
                    .background(theme.background)
                    .child(ControlStrip::new(self.tab, config, encoding, roles).legend(key))
                    .child(
                        rect()
                            .width(Size::flex(1.))
                            .height(Size::fill())
                            .padding((8., 12.))
                            .child(body),
                    ),
            )
    }
}

/// Why this answer is not a chart, or `None` when it is one.
///
/// Three groups, and the surface treats them alike because the user does: the engine's two
/// **refusals** (spec §7 — both carry no data at all, so there is no half-drawn chart to put a
/// message beside), a shape a mark cannot honestly draw, and an answer that simply has nothing
/// in it. The last group matters because the alternative is not a worse chart, it is a *blank
/// pane* — no axes, no message, indistinguishable from a bug.
fn notice(data: &ChartData, mark: ChartMark) -> Option<(&'static str, String)> {
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
        // The engine bins over finite values only, so an empty set of bins is a column with
        // none — which draws no axes at all rather than an empty plot.
        ChartData::Bins(bins) if bins.is_empty() => Some((
            NOTHING,
            "This column has no finite values to put in a bin.".to_string(),
        )),
        // One bin with no width is a column with a single distinct value: the engine answers
        // it honestly, and a zero-width rectangle paints nothing at all.
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
        // A table with no series at all draws axes and nothing else — and for a pie, not even
        // those. `encode` cannot produce it today, but `ChartQuery::Rows` only *documents*
        // that `ys` is non-empty, and Chart 03 hands `ys` to the user.
        ChartData::Table { series, .. } if series.is_empty() => {
            Some((NOTHING, "No column is being plotted.".to_string()))
        }
        ChartData::Table { series, .. } if mark == ChartMark::Pie => pie_notice(series),
        _ => None,
    }
}

/// What each colour on the plot means, in the order the plot draws them — the strip's legend.
///
/// Only the marks that draw in **more than one** colour have anything to key: a scatter and a
/// histogram are one colour by construction, so a legend over them would be a swatch beside the
/// only thing on screen.
///
/// The pie's rows come out of [`marks::pie_slices`], the same walk the wedges are drawn from,
/// so the legend cannot name a colour the plot gave to a different category.
fn legend(data: &ChartData, mark: ChartMark, dress: &Dress) -> Vec<LegendEntry> {
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
            })
            .collect();
    }
    series
        .iter()
        .enumerate()
        .map(|(i, one)| LegendEntry {
            swatch: dress.series(i),
            label: one.name.clone(),
            detail: None,
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

/// What stands in for the plot when there is nothing honest to draw.
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
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .vertical()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .spacing(8.)
            .padding(24.)
            .child(Title::new(self.title).color(self.color))
            .child(
                Prose::new(self.body.clone())
                    .color(self.color)
                    .max_width(Size::px(380.))
                    .wrap()
                    .align(TextAlign::Center),
            )
    }
}

#[cfg(test)]
mod tests {
    use strata_model::Axis;

    use super::config::ROWS_CAP;
    use super::*;

    fn series(name: &str, values: &[Option<f64>]) -> ChartSeries {
        ChartSeries {
            name: name.into(),
            values: values.to_vec(),
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

    /// The refusals say what they counted, in the app's own figures.
    #[test]
    fn the_over_cap_refusal_names_the_cap_the_way_every_other_count_is_written() {
        let (title, body) = notice(
            &ChartData::OverCap {
                unit: CapUnit::Rows,
                cap: ROWS_CAP,
            },
            ChartMark::Bar,
        )
        .expect("over cap refuses");
        assert_eq!(title, "Too much data to chart honestly");
        assert!(body.contains("1,000 rows"), "{body}");
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
                notice(&data, mark).is_some(),
                "{mark:?} over {data:?} would have painted nothing at all"
            );
        }
    }

    /// The legend is the one place a colour is explained, so its rows have to be the rows the
    /// plot actually drew — for a pie that means the *surviving* slices, in draw order, keyed
    /// off the same walk `pie` uses.
    #[test]
    fn the_legend_keys_the_colours_the_plot_drew() {
        let dress = Dress {
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
            label: ("mono".into(), 10.),
        };

        // A series per Y column, in the ramp's own order.
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
        let key = legend(&two, ChartMark::Bar, &dress);
        assert_eq!(
            key.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["revenue", "cost"]
        );
        assert_eq!((key[0].swatch, key[1].swatch), (Color::RED, Color::GREEN));
        assert!(key[0].detail.is_none(), "a series reads off the axis");

        // A pie keys the *drawn* slices: the zero and the gap have no wedge, so the second
        // colour belongs to `c` and the legend has to say so.
        let pie = ChartData::Table {
            axis: Axis {
                labels: vec!["a".into(), "skipped".into(), "gone".into(), "c".into()],
                positions: None,
            },
            series: vec![series("n", &[Some(3.), Some(0.), None, Some(1.)])],
        };
        let key = legend(&pie, ChartMark::Pie, &dress);
        assert_eq!(
            key.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!((key[0].swatch, key[1].swatch), (Color::RED, Color::GREEN));
        assert_eq!(key[0].detail.as_deref(), Some("75%"));

        // Nothing to key where the plot draws in one colour.
        assert!(legend(&ChartData::Bins(Vec::new()), ChartMark::Histogram, &dress).is_empty());
        assert!(legend(&ChartData::Points(Vec::new()), ChartMark::Scatter, &dress).is_empty());
    }

    /// A pie refuses a negative value rather than dropping it: every percentage on the chart is
    /// read against a total, and quietly leaving a row out of that total is the silent
    /// truncation spec §1.4 rules out. A zero or a NULL is not the same thing — a zero-area
    /// slice is arithmetic, and the rest of the pie is still true.
    #[test]
    fn a_pie_refuses_negative_values_but_draws_around_zeroes_and_gaps() {
        let (title, body) = notice(&table(&[Some(3.), Some(-1.)]), ChartMark::Pie)
            .expect("a negative slice has no wedge");
        assert_eq!(title, "A pie cannot show negative values");
        assert!(body.contains("'amount'"), "{body}");

        assert_eq!(
            notice(&table(&[Some(3.), Some(0.), None]), ChartMark::Pie),
            None
        );
        // …and the same values under a mark that *can* draw a negative are not refused.
        assert_eq!(notice(&table(&[Some(3.), Some(-1.)]), ChartMark::Bar), None);
    }
}
