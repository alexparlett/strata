//! Chart vocabulary: the **request** a chart makes of a snapshot ([`ChartQuery`]), the chart-ready
//! **answer** ([`ChartData`]), and the persisted **intent** ([`ChartConfig`]).
//! `docs/CHART_SPEC.md` §5.
//!
//! **Renderer-first** (spec §1.2): the chart computes nothing SQL can say. A request names columns
//! and a cap — never an aggregate, a bucket, or an order. The engine-side aggregation vocabulary
//! that used to live here was built, reviewed and withdrawn, and it must not come back.
//!
//! [`ChartQuery`] is freya-query **cache identity**, which is why every field is hashable and
//! comparable — no floats. [`ChartData`] carries no "was it capped" flag beside a half-filled
//! payload: a refusal has nothing to draw.

use serde::{Deserialize, Deserializer, Serialize};

/// Which mark the chart draws (`docs/CHART_SPEC.md` §4).
///
/// The **renderer's** choice, not the engine's, so switching between two marks over the same query
/// is a repaint rather than a re-read.
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
    /// A matrix of two category columns, the measure as cell colour ([`ChartQuery::Rows`] with a
    /// required series — the long→wide pivot **is** the matrix).
    Heatmap,
    /// A line with the span between two bound columns tinted — `y` / `y_lo` / `y_hi` are the user's
    /// own SQL, mapped.
    Band,
    /// A box plot per category: median, quartile and whisker columns the user's SQL computes, never
    /// an engine percentile.
    Box,
}

impl ChartMark {
    /// Every mark, in the order the picker offers them (three rows of three).
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

/// **How the user left the chart** (`docs/CHART_SPEC.md` §6), persisted per tab.
///
/// *Intent*, never a resolved read: every channel can say "I have not chosen" and takes the
/// schema-derived default. Resolving intent + schema into the read is the surface's job (the one
/// `encode` site), and a reference this result cannot answer falls back at read time rather than
/// being written out of the config, so it comes back if the column does.
///
/// [`sort`](Self::sort), [`hidden`](Self::hidden) and [`log_y`](Self::log_y) are **view
/// transforms** over the settled [`ChartData`] — a repaint, never cache identity.
/// [`bins`](Self::bins) is the opposite, because the engine does the counting.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ChartConfig {
    /// The chosen mark, or `None` to take the default for X's role (line over a temporal X, bar
    /// otherwise). Deserialized tolerantly: an unknown mark name reads as **unchosen**, so a tab
    /// written by a newer build cannot fail the whole session.
    #[serde(default, deserialize_with = "mark_compat")]
    pub mark: Option<ChartMark>,
    /// The category axis.
    #[serde(default)]
    pub x: ChartX,
    /// The value columns, each its own series. `None` takes the default (the leading measures);
    /// `Some(vec![])` is the user having unpicked them all, which is a chart with nothing to plot
    /// and says so.
    #[serde(default)]
    pub ys: Option<Vec<String>>,
    /// The column the long→wide pivot splits on. `None` is *no split* — both the default and the
    /// explicit choice, except for a heatmap, whose matrix requires one and derives the default at
    /// resolve time.
    #[serde(default)]
    pub series: Option<String>,
    /// A band's lower bound, and a box plot's low whisker. Deliberately **no schema-derived
    /// default**: a bound is a column the user's SQL computed, and guessing one from a name is what
    /// the role invariant rules out.
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
    /// How many bins a histogram is cut into, or `None` for the engine's own `√n` choice. The one
    /// channel here that *is* part of the read, so it reaches [`ChartQuery::Histogram`].
    #[serde(default)]
    pub bins: Option<u16>,
    /// The series the legend has pressed out of the chart, **by name**.
    ///
    /// Not pruned against the result: a name this result has no series for matches nothing, and
    /// keeping it is what brings the choice back when the column does. A [`ChartSeries::name`] is a
    /// label and not a key, so a NULL-valued series and a literal `(null)` one hide together.
    #[serde(default)]
    pub hidden: Vec<String>,
    /// Draw the value axis logarithmically — a repaint, never a re-read.
    #[serde(default)]
    pub log_y: bool,
    /// Draw a least-squares trendline over the scatter. Kept for every mark and honoured only where
    /// a fit means something, so switching marks never spends the choice. The fit itself is
    /// [`Trend`], a separate read.
    #[serde(default)]
    pub trend: bool,
    /// How the settled rows are ordered on the way to the marks.
    #[serde(default)]
    pub sort: ChartSort,
}

/// [`ChartConfig::mark`]'s tolerant read: an unknown mark name is an unchosen mark.
///
/// `session.json` is one document, so a strict enum read would let a single tab written by a newer
/// build fail the whole session into `session.json.corrupt`. That one tab charts its default mark
/// instead, and everything else restores untouched.
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
/// Three states rather than an `Option<String>`, because "not chosen" and "chosen to be nothing"
/// are different answers: an X set to [`RowIndex`](Self::RowIndex) must stay that way when the next
/// result happens to have a date column in it.
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
    /// The order the user's own query produced, which is the snapshot ordinal's. The default, and
    /// the only one the engine has any part in.
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

    /// How this order reads in the picker — short, because three of them share a 232px strip.
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
        /// The value columns — **each is its own series**, named by column. Must not be empty.
        ys: Vec<String>,
        /// Splits rows into one series per distinct value (the long→wide pivot). Requires `x`;
        /// combined with several `ys`, series are named `value: column`.
        series: Option<String>,
        /// How many result rows the chart will draw before it refuses. Not a truncation point:
        /// over it, nothing is drawn (spec §7).
        cap: usize,
    },
    /// Raw points (scatter).
    Raw { x: String, y: String, cap: usize },
    /// Uniform-width bins over one numeric column, counted engine-side — the one mark that
    /// computes, because DataFusion has no `width_bucket` and a raw column cannot be binned
    /// without a min/max pass. `bins` of `None` lets the engine pick from the row count.
    Histogram { col: String, bins: Option<usize> },
}

/// The category axis of a [`ChartData::Table`]: what each position is labelled, and — when
/// X has an order of its own — where it truly sits.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    /// One label per category, in draw order, rendered through the engine's display config so a
    /// value reads the way it reads in the grid. A NULL X is `(null)` — a **label, not a key**.
    pub labels: Vec<String>,
    /// `Some` when X is numeric or temporal (epoch milliseconds; clock times in their own ticks),
    /// so a line or scatter renderer can place marks truly rather than equally spaced. `None` for a
    /// categorical X, and per-entry `None` for a NULL, which has no position.
    pub positions: Option<Vec<Option<f64>>>,
}

/// One drawn series: a legend entry and its value at every category.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartSeries {
    /// How this series reads in a legend: the Y column's name, the distinct series value, or
    /// `value: column` when both split.
    ///
    /// A **label, not a key** — two series can carry the same name, so a consumer addresses a
    /// series by position and never by this string.
    pub name: String,
    /// One value per entry of [`Axis::labels`], in that order. `None` is **no value in that
    /// cell** — a NULL Y, or a (category, series) pair the data never contained. A renderer draws
    /// it as a gap and never interpolates across it.
    pub values: Vec<Option<f64>>,
}

/// One raw point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartPoint {
    pub x: f64,
    pub y: f64,
}

/// The least-squares fit over a scatter's finite points — the one sanctioned engine computation
/// beside the histogram, because the overlay is a function of the **encoding** rather than of the
/// query, and templating it into SQL would rewrite the user's query on every encoder gesture.
///
/// Deliberately **not** part of [`ChartQuery`]: its own read, keyed by the two columns, so toggling
/// the overlay never re-reads the points.
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
    /// Refused: the read would have exceeded `cap` of `unit`. Carries no data at all — the chart
    /// is not drawn, and the surface says to aggregate in SQL.
    OverCap { unit: CapUnit, cap: usize },
    /// Refused: the long→wide pivot found two rows in one (X, series) cell. Aggregating them is
    /// the user's own `GROUP BY`, which the surface names and offers no control behind. Carries
    /// the encoding's column names so the message can say which.
    Duplicates { x: String, series: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **An unknown mark degrades one tab's mark, never the session.**
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
