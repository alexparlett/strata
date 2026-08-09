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

use serde::{Deserialize, Serialize};

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
}

impl ChartMark {
    /// Every mark, in the order the picker offers them (the design's tile grid).
    pub const ALL: [ChartMark; 6] = [
        ChartMark::Bar,
        ChartMark::Line,
        ChartMark::Area,
        ChartMark::Scatter,
        ChartMark::Histogram,
        ChartMark::Pie,
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
    #[serde(default)]
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
    /// the explicit choice, which are the same thing here, so there is nothing to tell apart.
    #[serde(default)]
    pub series: Option<String>,
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
    /// How the settled rows are ordered on the way to the marks.
    #[serde(default)]
    pub sort: ChartSort,
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
