//! Chart vocabulary: the **request** a chart makes of a snapshot ([`ChartQuery`]) and the
//! chart-ready **answer** it gets back ([`ChartData`]). Produced by
//! `strata_core::engine::Engine::chart`, consumed by the results Chart surface
//! (`docs/CHART_SPEC.md` §5).
//!
//! **Renderer-first** (spec §1.2): the chart computes nothing SQL can say. A request names
//! columns and a cap — never an aggregate, a bucket, or an order — and the answer is the
//! result's own rows reshaped for marks: in result order, pivoted long→wide when a series
//! column splits them, capped and refused rather than truncated. The engine-side aggregation
//! vocabulary that used to live here (`AggFn`, `Measure`, `Bucket`, `Stride`, `Width`) was
//! built, reviewed and withdrawn — `docs/reference/INVARIANTS.md` (the chart entry) has the
//! history; do not reintroduce it.
//!
//! [`ChartQuery`] is freya-query **cache identity**, which is why every field is hashable
//! and comparable — column names and caps, no floats. [`ChartData`] carries no "was it
//! capped" flag beside a half-filled payload: a refusal is [`ChartData::OverCap`] or
//! [`ChartData::Duplicates`], with nothing to draw, because "honest boundaries" (spec §1.4)
//! means there is no such thing as a truncated chart to render.
//!
//! [`ChartConfig`] is the third thing here and the only **persisted** one: what the user
//! asked for, as opposed to what the engine was asked and what it answered. It holds
//! intent, never a resolved read — see its own note.

use serde::{Deserialize, Deserializer, Serialize};

/// Which mark the chart draws (`docs/CHART_SPEC.md` §4).
///
/// The mark is the **renderer's** choice, not the engine's: it decides which
/// [`ChartQuery`] shape the surface asks for and how the answer is painted, and switching
/// between two marks over the same query (bar ↔ line ↔ area, and pie over the same
/// columns) is a repaint rather than a re-read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartMark {
    /// Grouped bars over the categories, one run per series ([`ChartQuery::Rows`]).
    Bar,
    /// A line per series, with a gap wherever a value is missing ([`ChartQuery::Rows`]).
    Line,
    /// [`Line`](Self::Line) with the span down to the zero baseline tinted.
    Area,
    /// Raw points over two measures ([`ChartQuery::Raw`]).
    Scatter,
    /// Engine-binned counts over one measure ([`ChartQuery::Histogram`]).
    Histogram,
    /// One slice per category over a single measure ([`ChartQuery::Rows`], capped).
    Pie,
    /// A matrix of two category columns, the measure as cell colour ([`ChartQuery::Rows`]
    /// with a required series — the long→wide pivot **is** the matrix, Chart 10).
    Heatmap,
    /// A line with the span between two bound columns tinted (`docs/CHART_SPEC.md` §10:
    /// `y` / `y_lo` / `y_hi` are the user's own SQL, mapped).
    Band,
    /// A box plot per category: median, quartile and whisker columns the user's SQL
    /// computes (spec §10) — never an engine percentile.
    Box,
}

impl ChartMark {
    /// Every mark, in the order the picker offers them (the design's tile grid — nine is
    /// three clean rows of three).
    pub const ALL: [ChartMark; 9] = [
        ChartMark::Bar,
        ChartMark::Line,
        ChartMark::Area,
        ChartMark::Scatter,
        ChartMark::Histogram,
        ChartMark::Pie,
        ChartMark::Heatmap,
        ChartMark::Band,
        ChartMark::Box,
    ];

    /// How this mark reads in the picker.
    pub fn label(self) -> &'static str {
        match self {
            ChartMark::Bar => "Bar",
            ChartMark::Line => "Line",
            ChartMark::Area => "Area",
            ChartMark::Scatter => "Scatter",
            ChartMark::Histogram => "Histogram",
            ChartMark::Pie => "Pie",
            ChartMark::Heatmap => "Heatmap",
            ChartMark::Band => "Band",
            ChartMark::Box => "Box",
        }
    }
}

/// **How the user left the chart** (`docs/CHART_SPEC.md` §6): a mark, three column
/// assignments, a bin count and three view preferences. Persisted per tab
/// (`TabSnapshot::chart`).
///
/// This is *intent*, never a resolved read. Every channel can say "I have not chosen" —
/// [`None`] on the mark and the Ys, [`ChartX::Auto`] on X — and an unchosen channel takes the
/// schema-derived default, so a result with different columns charts sensibly without the
/// user touching anything. Resolving intent + schema into the read is the surface's job (the
/// one `encode` site), which is also what keeps a column name the result no longer has from
/// ever reaching a [`ChartQuery`]: a reference that no longer resolves falls back to the
/// default rather than being written out of the config, so it comes back if the column does.
///
/// [`sort`](Self::sort), [`hidden`](Self::hidden) and [`log_y`](Self::log_y) are the odd ones
/// out: **view transforms** over the settled [`ChartData`], not part of the read (spec §6), so
/// flipping any of them repaints without re-querying and never touches cache identity.
/// [`bins`](Self::bins) is the opposite — the engine does the counting, so it rides in the
/// request.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ChartConfig {
    /// The chosen mark, or `None` to take the default for X's role (line over a temporal X,
    /// bar otherwise).
    ///
    /// Deserialized tolerantly: a mark name this build does not know reads as **unchosen**,
    /// never as a parse error. `session.json` is one document, so a strict read here would
    /// let one tab's mark from a newer build set the whole session aside as corrupt — the
    /// tab degrades to its default mark instead, alone (Chart 10's serde-compat check).
    #[serde(default, deserialize_with = "mark_compat")]
    pub mark: Option<ChartMark>,
    /// The category axis.
    #[serde(default)]
    pub x: ChartX,
    /// The value columns, each its own series. `None` takes the default (the leading
    /// measures); `Some(vec![])` is the user having deliberately unpicked them all, which is
    /// a chart with nothing to plot and says so.
    #[serde(default)]
    pub ys: Option<Vec<String>>,
    /// The column the long→wide pivot splits on. `None` is *no split* — both the default and
    /// the explicit choice, which are the same thing here, so there is nothing to tell apart —
    /// except for a heatmap, whose matrix requires one and derives the default at resolve
    /// time.
    #[serde(default)]
    pub series: Option<String>,
    /// A band's lower bound, and a box plot's low whisker (Chart 10). Intent like every
    /// reference here — a column this result cannot answer falls back at read time — and
    /// deliberately **no schema-derived default**: a bound is a column the user's SQL
    /// computed, and guessing one from a name is what the role invariant rules out.
    #[serde(default)]
    pub y_lo: Option<String>,
    /// A band's upper bound, and a box plot's high whisker.
    #[serde(default)]
    pub y_hi: Option<String>,
    /// A box plot's first quartile.
    #[serde(default)]
    pub q1: Option<String>,
    /// A box plot's third quartile.
    #[serde(default)]
    pub q3: Option<String>,
    /// How many bins a histogram is cut into, or `None` for the engine's own `√n` choice.
    ///
    /// The one channel here that *is* part of the read: a bin count changes what the engine
    /// counts, so it reaches [`ChartQuery::Histogram`] and a new value is a new entry. An
    /// integer rather than a count of any width, because a [`ChartQuery`] is cache identity
    /// and identity has no floats in it.
    #[serde(default)]
    pub bins: Option<u16>,
    /// The series the legend has pressed out of the chart, **by name**.
    ///
    /// A view preference in [`sort`](Self::sort)'s class: it never reaches a [`ChartQuery`],
    /// and it is not pruned against the result — a name this result has no series for matches
    /// nothing and is harmless, and keeping it is what brings the choice back when the column
    /// does. A [`ChartSeries::name`] is a label and not a key, so a NULL-valued series and a
    /// literal `"(null)"` one are hidden and shown together; that is accepted coarseness
    /// rather than a reason to key a user-facing legend by position.
    #[serde(default)]
    pub hidden: Vec<String>,
    /// Draw the value axis logarithmically. A display transform, like
    /// [`sort`](Self::sort) — a repaint, never a re-read.
    #[serde(default)]
    pub log_y: bool,
    /// Draw a least-squares trendline over the scatter. Kept for every mark and honoured only
    /// where a fit means something (a scatter), the way [`bins`](Self::bins) is kept for the
    /// histogram — switching marks never spends the choice. The fit itself is [`Trend`], a
    /// separate read: flipping this never re-reads the points.
    #[serde(default)]
    pub trend: bool,
    /// How the settled rows are ordered on the way to the marks.
    #[serde(default)]
    pub sort: ChartSort,
}

/// [`ChartConfig::mark`]'s tolerant read: an unknown mark name is an unchosen mark.
///
/// The mark names ride in `session.json`, one document per project, so a strict enum read
/// would let a single tab written by a newer build — one that knows a mark this build does
/// not — fail the whole session into `session.json.corrupt`. Deserializing the raw string
/// first and then trying the enum keeps the failure where it belongs: that one tab charts
/// its default mark, and everything else restores untouched.
fn mark_compat<'de, D>(de: D) -> Result<Option<ChartMark>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::value::{Error as PlainError, StrDeserializer};
    use serde::de::IntoDeserializer;

    let Some(raw) = Option::<String>::deserialize(de)? else {
        return Ok(None);
    };
    let named: StrDeserializer<PlainError> = raw.as_str().into_deserializer();
    Ok(ChartMark::deserialize(named).ok())
}

/// What sits on the category axis.
///
/// Three states rather than an `Option<String>`, because "not chosen" and "chosen to be
/// nothing" are different answers: an unchosen X takes the schema's default, while an X the
/// user set to [`RowIndex`](Self::RowIndex) charts against the row number and must stay that
/// way when the next result happens to have a date column in it.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartX {
    /// Take the default: the first temporal column, else the first dimension, else the row
    /// index.
    #[default]
    Auto,
    /// Chart against the row number — what "X: none" means (spec §4).
    RowIndex,
    /// This column, when the result still has it.
    Column(String),
}

/// The order the rows draw in — a **view transform** over the settled data (spec §6), so it
/// re-renders rather than re-reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartSort {
    /// The order the user's own query produced, which is the snapshot ordinal's
    /// (`SNAPSHOT_SPEC.md` §9). The default, and the only one the engine has any part in.
    #[default]
    ResultOrder,
    /// By the category axis, ascending — by true position where X has one, else by label.
    ByX,
    /// By the first series' value, descending.
    ByYDesc,
}

impl ChartSort {
    /// Every order, in the order the picker offers them.
    pub const ALL: [ChartSort; 3] = [ChartSort::ResultOrder, ChartSort::ByX, ChartSort::ByYDesc];

    /// How this order reads in the picker — short, because three of them share a strip
    /// 232px wide.
    pub fn label(self) -> &'static str {
        match self {
            ChartSort::ResultOrder => "Result",
            ChartSort::ByX => "X",
            ChartSort::ByYDesc => "Value",
        }
    }

    /// What the short label means, for the segment's tooltip.
    pub fn title(self) -> &'static str {
        match self {
            ChartSort::ResultOrder => "Result order",
            ChartSort::ByX => "Sort by X, ascending",
            ChartSort::ByYDesc => "Sort by value, descending",
        }
    }
}

/// One read of a snapshot, shaped for a chart. Resolved from the chart config + the result
/// schema, and carrying no UI types — this is what the engine answers and what the
/// freya-query entry is keyed by.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ChartQuery {
    /// The renderer-first read behind bar / line / area / pie: the referenced columns, in
    /// result order, up to `cap + 1` rows.
    Rows {
        /// The category axis. `None` charts against the row index.
        x: Option<String>,
        /// The value columns — **each is its own series**, named by column, so
        /// `SELECT month, revenue, cost …` is two lines with no configuration. Must not be
        /// empty.
        ys: Vec<String>,
        /// Splits rows into one series per distinct value (the long→wide pivot). Requires
        /// `x`; combined with several `ys`, series are named `value: column`.
        series: Option<String>,
        /// How many result rows the chart will draw before it refuses. Not a truncation
        /// point: over it, nothing is drawn (spec §7).
        cap: usize,
    },
    /// Raw points (scatter).
    Raw { x: String, y: String, cap: usize },
    /// Uniform-width bins over one numeric column, counted engine-side — the one mark that
    /// computes (spec §1.2: DataFusion has no `width_bucket`, and a raw column cannot be
    /// binned without a min/max pass). `bins` of `None` lets the engine pick from the row
    /// count.
    Histogram { col: String, bins: Option<usize> },
}

/// The category axis of a [`ChartData::Table`]: what each position is labelled, and — when
/// X has an order of its own — where it truly sits.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    /// One label per category, in draw order (= result order), rendered through the
    /// engine's display config so a value reads the way it reads in the grid. A NULL X is
    /// `(null)` — a **label, not a key**: a NULL and a literal `"(null)"` string are two
    /// categories.
    pub labels: Vec<String>,
    /// `Some` when X is numeric or temporal (epoch milliseconds; clock times in their own
    /// ticks): one entry per category, so a line or scatter renderer can place marks truly
    /// rather than equally spaced. `None` for a categorical X, and per-entry `None` for a
    /// NULL, which has no position.
    pub positions: Option<Vec<Option<f64>>>,
}

/// One drawn series: a legend entry and its value at every category.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartSeries {
    /// How this series reads in a legend: the Y column's name, the distinct series value,
    /// or `value: column` when both split.
    ///
    /// A **label, not a key**. Two series can carry the same name for the same reason two
    /// categories can — a NULL and a literal `(null)` render alike — so a consumer
    /// addresses a series by position, never by this string.
    pub name: String,
    /// One value per entry of [`Axis::labels`], in that order. `None` is **no value in
    /// that cell** — a NULL Y, or a (category, series) pair the data never contained. A
    /// renderer draws it as a gap and never interpolates across it.
    pub values: Vec<Option<f64>>,
}

/// One raw point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartPoint {
    pub x: f64,
    pub y: f64,
}

/// The least-squares fit over a scatter's finite points (`docs/CHART_SPEC.md` §10 — the one
/// sanctioned engine computation beside the histogram). Computed engine-side because the
/// overlay is a function of the **encoding** — which two columns the scatter currently plots —
/// not of the query: templating it into SQL would rewrite the user's query on every encoder
/// gesture, which "config is intent" forbids.
///
/// Deliberately **not** part of [`ChartQuery`]: the fit is its own read, keyed by the two
/// columns, so toggling the overlay never re-reads the points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trend {
    /// `k` in `y = kx + b`.
    pub slope: f64,
    /// `b` in `y = kx + b`.
    pub intercept: f64,
    /// The fit's coefficient of determination, in `0..=1`.
    pub r2: f64,
    /// How many finite pairs the fit covered.
    pub n: i64,
}

/// One histogram bin: the half-open interval `[lo, hi)` and how many rows fell in it. The
/// last bin of a set is closed at `hi`, so the maximum value has somewhere to land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartBin {
    pub lo: f64,
    pub hi: f64,
    pub count: u64,
}

/// What a refusal counted (spec §7) — the noun its message names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CapUnit {
    /// Result rows, for the [`ChartQuery::Rows`] read.
    Rows,
    /// Raw points, for scatter.
    Points,
}

/// A chart-ready read of one snapshot.
#[derive(Clone, Debug, PartialEq)]
pub enum ChartData {
    /// The rows, reshaped for marks: the axis in result order, and every
    /// [`ChartSeries::values`] exactly as long as it.
    Table {
        axis: Axis,
        series: Vec<ChartSeries>,
    },
    /// Raw points. **In no particular order** — a scatter draws marks, not a sequence;
    /// anything that needs one must sort for itself.
    Points(Vec<ChartPoint>),
    /// Histogram bins, ascending and contiguous.
    Bins(Vec<ChartBin>),
    /// Refused: the read would have exceeded `cap` of `unit`. Carries no data at all —
    /// the chart is not drawn (spec §1.4, §7), and the surface says to aggregate in SQL.
    OverCap { unit: CapUnit, cap: usize },
    /// Refused: the long→wide pivot found two rows in one (X, series) cell. Aggregating
    /// them is SQL's job, not the chart's (spec §1.2), and the user's own `GROUP BY` —
    /// the surface names it and offers no control behind it (spec §8).
    /// Carries the encoding's column names so the message can say which.
    Duplicates { x: String, series: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **An unknown mark degrades one tab's mark, never the session.** `session.json` is one
    /// document, so a strict enum read would let a newer build's mark name set the whole file
    /// aside as corrupt on downgrade. It reads as unchosen instead — and every other field of
    /// the same config still lands.
    #[test]
    fn an_unknown_mark_reads_as_unchosen_and_spends_nothing_else() {
        let known: ChartConfig = serde_json::from_str(r#"{ "mark": "heatmap" }"#).unwrap();
        assert_eq!(known.mark, Some(ChartMark::Heatmap));

        let future: ChartConfig =
            serde_json::from_str(r#"{ "mark": "hexbin", "log_y": true, "bins": 12 }"#).unwrap();
        assert_eq!(future.mark, None, "an unknown mark is an unchosen one");
        assert!(future.log_y, "the rest of the config still lands");
        assert_eq!(future.bins, Some(12));

        let absent: ChartConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.mark, None);
    }
}
