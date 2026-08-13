//! **What the chart is actually drawing**: the result's column roles, what each encoder may
//! offer over them, and the one place a [`ChartConfig`] plus a schema become a [`ChartQuery`].
//!
//! Three things live here and they are deliberately the same three the strip is built from:
//!
//! - [`Roles`] — the result's columns split by [`ChartRole`], in **result order**, which is
//!   what makes "the first temporal column" something a user can predict from their own
//!   SELECT list.
//! - the per-mark **option sets** ([`x_options`], [`y_options`], [`series_options`]) — spec §4's
//!   table, as functions. The strip builds its menus from them and the resolution below
//!   validates against them, so an encoding the mark cannot take is not reported, it is
//!   *unreachable*: no control ever offers the column.
//! - [`resolve`] + [`encode`] — intent + schema → the read. `resolve` merges the schema's
//!   defaults **under** the user's own choices (spec §6) and drops any reference the current
//!   result cannot answer; `encode` is the single `ChartQuery` construction site, so cache
//!   identity is built in exactly one place.
//!
//! Dropping a stale reference is a *read-time* fallback, never a write back into the config:
//! a column that vanishes from one result and returns in the next must bring the user's
//! choice back with it, and a config the surface silently rewrote could not do that.

use strata_core::engine::MAX_BINS;
use strata_model::{ChartConfig, ChartMark, ChartQuery, ChartRole, ChartSort, ChartX, ColumnInfo};

/// How many result rows the renderer-first read will draw before it refuses (spec §7).
pub const ROWS_CAP: usize = 1_000;
/// A pie's own cap — slices, not rows.
pub const PIE_CAP: usize = 24;
/// Raw points a scatter will draw before it refuses.
pub const RAW_CAP: usize = 6_000;
/// How many measures an **unchosen** Y charts at once. Every measure would make a wide
/// result unreadable; the encoder strip is how you ask for more.
const DEFAULT_YS: usize = 4;

/// A result's columns, each with the role its Arrow type resolved to, in result order.
///
/// The role is read off the column, not derived here: it was resolved where the engine still
/// had the `DataType` (`engine::catalog::chart_role`), so the encoder and the read agree on
/// what a measure is by construction. [`ChartRole::Other`] never enters — a nested column has
/// no axis to sit on and no value to plot, so it is offered nowhere.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Roles {
    columns: Vec<(String, ChartRole)>,
}

impl Roles {
    pub fn of(columns: &[ColumnInfo]) -> Self {
        Self {
            columns: columns
                .iter()
                .filter(|c| c.role != ChartRole::Other)
                .map(|c| (c.name.clone(), c.role))
                .collect(),
        }
    }

    /// Every column of one role, in result order.
    fn with_role(&self, role: ChartRole) -> Vec<String> {
        self.columns
            .iter()
            .filter(|(_, r)| *r == role)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The numeric columns — Ys, both scatter axes, a histogram's value.
    pub fn measures(&self) -> Vec<String> {
        self.with_role(ChartRole::Measure)
    }

    /// Everything a category axis or a series split can take: temporal and dimension
    /// columns, interleaved in result order.
    pub fn categories(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|(_, r)| *r != ChartRole::Measure)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Every chartable column, in result order.
    pub fn all(&self) -> Vec<String> {
        self.columns.iter().map(|(name, _)| name.clone()).collect()
    }

    /// The default category axis (spec §6): the first time-like column, else the first
    /// dimension, else none — which charts against the row index.
    fn x(&self) -> Option<String> {
        self.columns
            .iter()
            .find(|(_, role)| is_time(*role))
            .or_else(|| {
                self.columns
                    .iter()
                    .find(|(_, role)| *role == ChartRole::Dimension)
            })
            .map(|(name, _)| name.clone())
    }

    /// The role this result gives `name`, or `None` for a column it does not have.
    fn role(&self, name: &str) -> Option<ChartRole> {
        self.columns
            .iter()
            .find(|(column, _)| column == name)
            .map(|(_, role)| *role)
    }
}

/// Whether this role puts a column on a **time** axis — an instant or a clock time.
///
/// On an axis the two are one thing, which is all this surface reads: the same default X, the
/// same default mark, offered in the same menus. They are separate roles because they differ
/// in **SQL**, where a day-wide `date_bin` stride is meaningful over a calendar instant and
/// refused outright over a time of day — the distinction chart-side bucketing will need, kept
/// where the Arrow `DataType` still is rather than re-derived later from a type's spelling
/// (`ChartRole::Instant`).
fn is_time(role: ChartRole) -> bool {
    matches!(role, ChartRole::Instant | ChartRole::Clock)
}

/// The default mark (spec §6): a line over a **temporal X**, a bar otherwise.
///
/// It reads the X actually being charted, not the result's column list: a user who put a
/// country on the axis of a result that also happens to carry a date is not asking for a line
/// across an unordered category. Charting against the row index is a bar for the same reason.
fn default_mark(x: Option<&str>, roles: &Roles) -> ChartMark {
    match x.and_then(|name| roles.role(name)) {
        Some(role) if is_time(role) => ChartMark::Line,
        _ => ChartMark::Bar,
    }
}

/// Which columns this mark's **X** will take. Empty means the mark has no category axis at
/// all (a histogram bins one column and puts the counts on Y), so the strip shows no X row.
pub fn x_options(mark: ChartMark, roles: &Roles) -> Vec<String> {
    match mark {
        ChartMark::Scatter => roles.measures(),
        ChartMark::Pie | ChartMark::Heatmap | ChartMark::Box => roles.categories(),
        ChartMark::Histogram => Vec::new(),
        ChartMark::Bar | ChartMark::Line | ChartMark::Area | ChartMark::Band => roles.all(),
    }
}

/// Whether this mark can chart against the **row index** — the "X: none" of spec §4. A pie of
/// the row number is a slice per row, and a scatter has no axis without a measure on it. A
/// band is a line with bounds, so it keeps the line's row-index axis; a heatmap and a box
/// need real categories.
pub fn allows_row_index(mark: ChartMark) -> bool {
    matches!(
        mark,
        ChartMark::Bar | ChartMark::Line | ChartMark::Area | ChartMark::Band
    )
}

/// Which columns this mark's **Y** will take: the measures, always — the same predicate the
/// engine's read gates a Y on.
pub fn y_options(roles: &Roles) -> Vec<String> {
    roles.measures()
}

/// Whether this mark draws **several** Ys as several series. Every other mark takes exactly
/// one (spec §4) — a heatmap's measure is its colour, and a band's and box's other columns
/// are *roles*, not extra series — so their Y picker replaces rather than accumulates.
pub fn takes_many_ys(mark: ChartMark) -> bool {
    matches!(mark, ChartMark::Bar | ChartMark::Line | ChartMark::Area)
}

/// Which columns this mark's **series** will take, given the X it already has. Empty means no
/// series row at all: only bar / line / area split — and a heatmap, whose second category *is*
/// the series channel (the pivot is the matrix) — the pivot needs an X to pivot *around*,
/// and one column cannot be both the category and the split (all three are the engine's own
/// refusals — this is what keeps them unreachable).
pub fn series_options(mark: ChartMark, roles: &Roles, x: Option<&str>) -> Vec<String> {
    match (mark, x) {
        (ChartMark::Bar | ChartMark::Line | ChartMark::Area | ChartMark::Heatmap, Some(x)) => roles
            .categories()
            .into_iter()
            .filter(|name| name != x)
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether this mark **requires** its series channel — a heatmap's matrix has no shape
/// without the second category, so its picker offers no "None" row and an unset choice takes
/// the first remaining category as its default.
pub fn series_required(mark: ChartMark) -> bool {
    mark == ChartMark::Heatmap
}

/// How many fixed-order ys a band's read carries — centre, lower, upper. The renderer
/// destructures exactly this many and the notice asks for a complete row of them, so the
/// arity has one author.
pub const BAND_YS: usize = 3;
/// How many fixed-order ys a box plot's read carries — median, low whisker, high whisker,
/// q1, q3.
pub const BOX_YS: usize = 5;

/// Whether this mark reads the **bound** roles (`y_lo` / `y_hi`, Chart 10) — a band's edges,
/// and a box plot's whiskers. Everything else drops them at resolve time the way `bins` is
/// dropped for a mark with nothing to bin.
pub fn reads_bounds(mark: ChartMark) -> bool {
    matches!(mark, ChartMark::Band | ChartMark::Box)
}

/// Whether this mark reads the **quartile** roles (`q1` / `q3`) — the box plot alone.
pub fn reads_quartiles(mark: ChartMark) -> bool {
    mark == ChartMark::Box
}

/// Whether this mark's data has an order to sort — [`ChartData::Table`]'s categories. A
/// scatter draws unordered marks and a histogram's bins are ascending by construction, so
/// neither has anything the sort could mean.
///
/// [`ChartData::Table`]: strata_model::ChartData::Table
pub fn sortable(mark: ChartMark) -> bool {
    matches!(
        mark,
        ChartMark::Bar
            | ChartMark::Line
            | ChartMark::Area
            | ChartMark::Pie
            | ChartMark::Heatmap
            | ChartMark::Band
            | ChartMark::Box
    )
}

/// Whether this mark's value axis can be logarithmic.
///
/// A **bar** and an **area** are read as area from a baseline — the length of the bar and the
/// filled span *are* the magnitude — and a log axis has no baseline to measure from, so the
/// same picture would mean something the reader has no way to recover. A **pie** has no axis
/// at all. What is left is the three marks that plot position rather than extent, which is
/// exactly where a log axis earns its keep: a line, a scatter, and a histogram's counts.
pub fn log_axis(mark: ChartMark) -> bool {
    matches!(
        mark,
        ChartMark::Line | ChartMark::Scatter | ChartMark::Histogram
    )
}

/// Whether this mark can carry the least-squares trendline (Chart 11).
///
/// Only a scatter: the fit is a statement about how one measure moves with another, which is
/// the scatter's whole encoding. A line or bar's X may be categorical — regression over
/// category indices is a number with no meaning — and a histogram's bars are counts of the
/// one measure it has.
pub fn trendable(mark: ChartMark) -> bool {
    mark == ChartMark::Scatter
}

/// Whether this mark draws **several** series, so a legend press has anything to hide.
///
/// A pie is deliberately not one of them. Hiding a slice would silently recompute every
/// remaining percentage against a smaller total, which is the chart telling a different story
/// than the data (spec §1.4) — so its legend rows stay inert.
pub fn hideable(mark: ChartMark) -> bool {
    takes_many_ys(mark)
}

/// What the chart is drawing: the config resolved against the result actually in hand. Every
/// column here exists in that result, which is the whole point — nothing downstream has to
/// re-check.
#[derive(Clone, PartialEq, Debug)]
pub struct Encoding {
    pub mark: ChartMark,
    /// `None` charts against the row index.
    pub x: Option<String>,
    pub ys: Vec<String>,
    pub series: Option<String>,
    /// The band roles (Chart 10), resolved only for the marks that read them
    /// ([`reads_bounds`] / [`reads_quartiles`]) and distinct from the Y and from each other
    /// by construction — a stale or colliding reference falls back to unset, which `encode`
    /// answers with the message naming the fix.
    pub y_lo: Option<String>,
    pub y_hi: Option<String>,
    pub q1: Option<String>,
    pub q3: Option<String>,
    /// A histogram's bin count, `None` for the engine's own choice. Empty for every other
    /// mark, which has nothing to bin.
    pub bins: Option<u16>,
    /// The series the legend has hidden, by name — empty for a mark whose legend cannot
    /// un-hide them. Names this result has no series for are **kept**: one matches nothing,
    /// and dropping it would spend a choice the next result might be able to honour.
    pub hidden: Vec<String>,
    pub log_y: bool,
    /// Whether the least-squares trendline is drawn — resolved against the mark like
    /// [`log_y`](Self::log_y), so only a scatter ever carries it. The fit itself is a
    /// separate read ([`TrendSpec`](crate::apps::project::query::TrendSpec)); this is only
    /// whether to ask for one.
    pub trend: bool,
    pub sort: ChartSort,
}

/// Merge the schema's defaults **under** the user's own choices (spec §6).
///
/// Each channel resolves the same way: take the choice if the result can still answer it,
/// otherwise derive. That single rule covers all of it — an untouched config (everything
/// derived), a tab restored from disk against a query whose SELECT list has since changed, a
/// mark switched to one whose axes the old choice cannot sit on, and a re-run whose result
/// simply dropped a column.
pub fn resolve(config: &ChartConfig, roles: &Roles) -> Encoding {
    let probe = config.mark.unwrap_or(ChartMark::Bar);
    let offered = x_options(probe, roles);
    let x = match &config.x {
        ChartX::Column(name) if offered.contains(name) => Some(name.clone()),
        ChartX::RowIndex if allows_row_index(probe) => None,
        _ => default_x(probe, roles),
    };
    let mark = config
        .mark
        .unwrap_or_else(|| default_mark(x.as_deref(), roles));

    let measures = roles.measures();
    let ys = match &config.ys {
        Some(chosen) if chosen.is_empty() => Vec::new(),
        Some(chosen) => {
            let kept: Vec<String> = measures
                .iter()
                .filter(|name| chosen.contains(name))
                .cloned()
                .collect();
            if kept.is_empty() {
                default_ys(mark, &measures, x.as_deref())
            } else {
                kept
            }
        }
        None => default_ys(mark, &measures, x.as_deref()),
    };
    let ys = if takes_many_ys(mark) {
        ys
    } else {
        ys.into_iter().take(1).collect()
    };

    let offered_series = series_options(mark, roles, x.as_deref());
    let series = config
        .series
        .clone()
        .filter(|name| offered_series.contains(name))
        .or_else(|| {
            series_required(mark)
                .then(|| offered_series.first().cloned())
                .flatten()
        });

    let mut spoken: Vec<String> = ys.clone();
    let mut band_ref = |choice: &Option<String>, wanted: bool| -> Option<String> {
        let name = choice
            .clone()
            .filter(|_| wanted)
            .filter(|name| measures.contains(name) && !spoken.contains(name))?;
        spoken.push(name.clone());
        Some(name)
    };
    let bounds = reads_bounds(mark);
    let quartiles = reads_quartiles(mark);
    let y_lo = band_ref(&config.y_lo, bounds);
    let y_hi = band_ref(&config.y_hi, bounds);
    let q1 = band_ref(&config.q1, quartiles);
    let q3 = band_ref(&config.q3, quartiles);

    Encoding {
        mark,
        x,
        ys,
        series,
        y_lo,
        y_hi,
        q1,
        q3,
        bins: config.bins.filter(|_| mark == ChartMark::Histogram),
        hidden: if hideable(mark) {
            config.hidden.clone()
        } else {
            Vec::new()
        },
        log_y: config.log_y && log_axis(mark),
        trend: config.trend && trendable(mark),
        sort: config.sort,
    }
}

/// The default category axis for a mark (spec §6).
fn default_x(mark: ChartMark, roles: &Roles) -> Option<String> {
    match mark {
        ChartMark::Histogram => None,
        ChartMark::Scatter => roles.measures().first().cloned(),
        ChartMark::Bar
        | ChartMark::Line
        | ChartMark::Area
        | ChartMark::Pie
        | ChartMark::Heatmap
        | ChartMark::Band
        | ChartMark::Box => roles.x(),
    }
}

/// The default value columns: the leading measures, minus whatever X already took — charting
/// a column against itself is never what an untouched config should mean, and for a scatter
/// it is the difference between two axes and one diagonal.
fn default_ys(mark: ChartMark, measures: &[String], x: Option<&str>) -> Vec<String> {
    let take = if takes_many_ys(mark) { DEFAULT_YS } else { 1 };
    measures
        .iter()
        .filter(|name| Some(name.as_str()) != x)
        .take(take)
        .cloned()
        .collect()
}

/// The read this encoding asks for, or why the columns cannot answer it. The message is the
/// whole answer at this stage — Chart 04 adds the scaffold CTA beneath it.
///
/// **The one `ChartQuery` construction site.** It is freya-query cache identity, so a second
/// place building one would fork the entry into a duplicate read.
pub fn encode(encoding: &Encoding, roles: &Roles) -> Result<ChartQuery, (&'static str, String)> {
    match encoding.mark {
        ChartMark::Scatter => match (&encoding.x, encoding.ys.first()) {
            (Some(x), Some(y)) => Ok(ChartQuery::Raw {
                x: x.clone(),
                y: y.clone(),
                cap: RAW_CAP,
            }),
            _ if roles.measures().len() < 2 => Err((
                "Pick two numeric columns",
                "A scatter plots one measure against another, and the result has fewer than two."
                    .into(),
            )),
            _ => Err(no_y(roles)),
        },
        ChartMark::Histogram => match encoding.ys.first() {
            Some(col) => Ok(ChartQuery::Histogram {
                col: col.clone(),
                bins: encoding
                    .bins
                    .map(|bins| usize::from(bins).clamp(1, MAX_BINS)),
            }),
            None => Err(no_y(roles)),
        },
        ChartMark::Pie => match (&encoding.x, encoding.ys.first()) {
            (Some(x), Some(y)) => Ok(ChartQuery::Rows {
                x: Some(x.clone()),
                ys: vec![y.clone()],
                series: None,
                cap: PIE_CAP,
            }),
            (None, _) => Err((
                "Pick a category column",
                "A pie slices one measure by a category, and the result has no column to slice by."
                    .into(),
            )),
            (_, None) => Err(no_y(roles)),
        },
        ChartMark::Heatmap => match (&encoding.x, &encoding.series, encoding.ys.first()) {
            (Some(x), Some(series), Some(y)) => Ok(ChartQuery::Rows {
                x: Some(x.clone()),
                ys: vec![y.clone()],
                series: Some(series.clone()),
                cap: ROWS_CAP,
            }),
            (Some(_), Some(_), None) => Err(no_y(roles)),
            _ => Err((
                "Pick two category columns",
                "A heatmap crosses two category columns, and the result has fewer than two.".into(),
            )),
        },
        ChartMark::Band => match (encoding.ys.first(), &encoding.y_lo, &encoding.y_hi) {
            (Some(y), Some(lo), Some(hi)) => Ok(ChartQuery::Rows {
                x: encoding.x.clone(),
                ys: vec![y.clone(), lo.clone(), hi.clone()],
                series: None,
                cap: ROWS_CAP,
            }),
            _ if roles.measures().len() < BAND_YS => Err((
                "Not enough numeric columns",
                format!(
                    "A band draws its centre between two bound columns and this result has \
                     {} numeric {}. Compute the bounds in SQL, for example avg(y) - \
                     stddev(y) and avg(y) + stddev(y).",
                    roles.measures().len(),
                    plural(roles.measures().len())
                ),
            )),
            (None, _, _) => Err(no_y(roles)),
            _ => Err((
                "Pick the band's bounds",
                "A band draws its centre between two bound columns your SQL computes, for \
                 example avg(y) - stddev(y) and avg(y) + stddev(y). Pick them on LOWER and \
                 UPPER."
                    .into(),
            )),
        },
        ChartMark::Box => match (
            &encoding.x,
            encoding.ys.first(),
            &encoding.y_lo,
            &encoding.y_hi,
            &encoding.q1,
            &encoding.q3,
        ) {
            (Some(x), Some(median), Some(lo), Some(hi), Some(q1), Some(q3)) => {
                Ok(ChartQuery::Rows {
                    x: Some(x.clone()),
                    ys: vec![
                        median.clone(),
                        lo.clone(),
                        hi.clone(),
                        q1.clone(),
                        q3.clone(),
                    ],
                    series: None,
                    cap: ROWS_CAP,
                })
            }
            (None, ..) => Err((
                "Pick a category column",
                "A box plot draws one box per category, and the result has no column to \
                 group by."
                    .into(),
            )),
            _ if roles.measures().len() < BOX_YS => Err((
                "Not enough numeric columns",
                format!(
                    "A box plot draws five measures per category and this result has {} \
                     numeric {}. Compute median, quartile and whisker columns in SQL, for \
                     example percentile_cont(0.25) WITHIN GROUP (ORDER BY y), min(y) and \
                     max(y).",
                    roles.measures().len(),
                    plural(roles.measures().len())
                ),
            )),
            (_, None, ..) => Err(no_y(roles)),
            _ => Err((
                "Pick the box's measures",
                "A box plot draws median, quartile and whisker columns your SQL computes, \
                 for example percentile_cont(0.25) WITHIN GROUP (ORDER BY y), min(y) and \
                 max(y) per category. Pick them on Q1, Q3, LOWER and UPPER."
                    .into(),
            )),
        },
        ChartMark::Bar | ChartMark::Line | ChartMark::Area => {
            if encoding.ys.is_empty() {
                return Err(no_y(roles));
            }
            Ok(ChartQuery::Rows {
                x: encoding.x.clone(),
                ys: encoding.ys.clone(),
                series: encoding.series.clone(),
                cap: ROWS_CAP,
            })
        }
    }
}

/// Why there is no Y — two different problems that would read as one message. The result
/// having no numeric column at all is a fact about the data; an empty pick is a choice the
/// user just made, and telling them their result has nothing numeric in it while its measures
/// sit in the menu they emptied would be a lie.
fn no_y(roles: &Roles) -> (&'static str, String) {
    if roles.measures().is_empty() {
        (
            "Pick a numeric column",
            "This mark plots a measure, and the result has no numeric column to plot.".into(),
        )
    } else {
        (
            "Pick a column to plot",
            "No column is chosen on the Y axis.".into(),
        )
    }
}

/// The one plural a count message needs.
fn plural(n: usize) -> &'static str {
    if n == 1 {
        "column"
    } else {
        "columns"
    }
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
    use strata_core::engine::column_info;

    use super::*;

    /// A result column built the way the engine builds one — from a real Arrow field, through
    /// the same `column_info` a Run's schema goes through. Nothing here restates the role
    /// mapping; the point of these cases is what the *encoder* does with it.
    fn column(name: &str, dtype: DataType) -> ColumnInfo {
        column_info(&Field::new(name, dtype, true))
    }

    /// The shape most of these cases resolve against: a date, a category and two measures.
    fn sales() -> Roles {
        Roles::of(&[
            column("month", DataType::Date32),
            column("country", DataType::Utf8),
            column("revenue", DataType::Int64),
            column("cost", DataType::Float64),
        ])
    }

    fn read(config: &ChartConfig, roles: &Roles) -> Result<ChartQuery, (&'static str, String)> {
        encode(&resolve(config, roles), roles)
    }

    #[test]
    fn an_untouched_config_charts_a_line_over_every_measure() {
        let roles = sales();
        let resolved = resolve(&ChartConfig::default(), &roles);
        assert_eq!(resolved.mark, ChartMark::Line);
        assert_eq!(resolved.x.as_deref(), Some("month"));
        assert_eq!(
            read(&ChartConfig::default(), &roles),
            Ok(ChartQuery::Rows {
                x: Some("month".into()),
                ys: vec!["revenue".into(), "cost".into()],
                series: None,
                cap: ROWS_CAP,
            })
        );
    }

    /// A nested column has no axis to sit on and no value to plot, so it is invisible to the
    /// encoder — which is the one place the chart's taxonomy has to be finer than the display
    /// one (a union renders as a string and cannot be charted as one).
    #[test]
    fn nested_columns_are_offered_nowhere() {
        let roles = Roles::of(&[
            column(
                "payload",
                DataType::Struct(vec![Field::new("a", DataType::Int64, true)].into()),
            ),
            column(
                "tags",
                DataType::List(Field::new("item", DataType::Utf8, true).into()),
            ),
            column("elapsed", DataType::Duration(TimeUnit::Second)),
            column("n", DataType::Int64),
        ]);
        assert_eq!(roles.all(), ["n"]);
        assert_eq!(roles.measures(), ["n"]);
        assert!(roles.categories().is_empty());
    }

    #[test]
    fn without_a_temporal_column_the_first_dimension_is_the_axis_and_the_mark_is_a_bar() {
        let roles = Roles::of(&[
            column("country", DataType::Utf8),
            column("revenue", DataType::Int64),
        ]);
        assert_eq!(
            read(&ChartConfig::default(), &roles),
            Ok(ChartQuery::Rows {
                x: Some("country".into()),
                ys: vec!["revenue".into()],
                series: None,
                cap: ROWS_CAP,
            })
        );
        let pie = ChartConfig {
            mark: Some(ChartMark::Pie),
            ..ChartConfig::default()
        };
        assert_eq!(
            read(&pie, &roles),
            Ok(ChartQuery::Rows {
                x: Some("country".into()),
                ys: vec!["revenue".into()],
                series: None,
                cap: PIE_CAP,
            })
        );
    }

    /// With no column to put on the axis the read still happens — against the row index,
    /// which is what "X: none" means (spec §4).
    #[test]
    fn measures_alone_chart_against_the_row_index() {
        let roles = Roles::of(&[column("n", DataType::Int64)]);
        assert_eq!(
            read(&ChartConfig::default(), &roles),
            Ok(ChartQuery::Rows {
                x: None,
                ys: vec!["n".into()],
                series: None,
                cap: ROWS_CAP,
            })
        );
    }

    #[test]
    fn the_marks_that_need_measures_say_so_rather_than_reading_nothing() {
        let text = Roles::of(&[column("country", DataType::Utf8)]);
        for mark in [ChartMark::Bar, ChartMark::Histogram] {
            let config = ChartConfig {
                mark: Some(mark),
                ..ChartConfig::default()
            };
            assert_eq!(read(&config, &text).unwrap_err().0, "Pick a numeric column");
        }

        let one = Roles::of(&[column("n", DataType::Int64)]);
        let scatter = ChartConfig {
            mark: Some(ChartMark::Scatter),
            ..ChartConfig::default()
        };
        for roles in [&text, &one] {
            assert_eq!(
                read(&scatter, roles).unwrap_err().0,
                "Pick two numeric columns"
            );
        }
        let histogram = ChartConfig {
            mark: Some(ChartMark::Histogram),
            ..ChartConfig::default()
        };
        assert_eq!(
            read(&histogram, &one),
            Ok(ChartQuery::Histogram {
                col: "n".into(),
                bins: None,
            })
        );

        let pie = ChartConfig {
            mark: Some(ChartMark::Pie),
            ..ChartConfig::default()
        };
        assert_eq!(read(&pie, &one).unwrap_err().0, "Pick a category column");

        let two = Roles::of(&[column("x", DataType::Int64), column("y", DataType::Float64)]);
        assert_eq!(
            read(&scatter, &two),
            Ok(ChartQuery::Raw {
                x: "x".into(),
                y: "y".into(),
                cap: RAW_CAP,
            })
        );
    }

    /// **The user's choice outranks the default, and survives a mark that cannot take it.**
    /// Switching to a pie narrows four Ys to one and drops the series — but only in the
    /// *encoding*: the config still holds them, so switching back restores the bar chart the
    /// user built.
    #[test]
    fn a_mark_narrows_the_encoding_without_spending_the_config() {
        let roles = sales();
        let config = ChartConfig {
            mark: Some(ChartMark::Bar),
            x: ChartX::Column("month".into()),
            ys: Some(vec!["revenue".into(), "cost".into()]),
            series: Some("country".into()),
            sort: ChartSort::ResultOrder,
            ..ChartConfig::default()
        };
        assert_eq!(
            read(&config, &roles),
            Ok(ChartQuery::Rows {
                x: Some("month".into()),
                ys: vec!["revenue".into(), "cost".into()],
                series: Some("country".into()),
                cap: ROWS_CAP,
            })
        );

        let pie = ChartConfig {
            mark: Some(ChartMark::Pie),
            ..config.clone()
        };
        assert_eq!(
            read(&pie, &roles),
            Ok(ChartQuery::Rows {
                x: Some("month".into()),
                ys: vec!["revenue".into()],
                series: None,
                cap: PIE_CAP,
            })
        );
        assert_eq!(read(&config, &roles).unwrap(), {
            ChartQuery::Rows {
                x: Some("month".into()),
                ys: vec!["revenue".into(), "cost".into()],
                series: Some("country".into()),
                cap: ROWS_CAP,
            }
        });
    }

    /// **A stale column name never reaches the read.** A tab restored against a result whose
    /// SELECT list has moved on falls back to the defaults, channel by channel — and keeps
    /// the choices the new result *can* still answer.
    #[test]
    fn a_reference_the_result_cannot_answer_falls_back_to_the_default() {
        let roles = sales();
        let config = ChartConfig {
            mark: Some(ChartMark::Bar),
            x: ChartX::Column("quarter".into()),
            ys: Some(vec!["margin".into(), "cost".into()]),
            series: Some("region".into()),
            sort: ChartSort::ResultOrder,
            ..ChartConfig::default()
        };
        let resolved = resolve(&config, &roles);
        assert_eq!(resolved.x.as_deref(), Some("month"), "X falls back");
        assert_eq!(resolved.ys, ["cost"], "the surviving Y is kept, alone");
        assert_eq!(resolved.series, None, "a series that is gone is no series");

        let gone = ChartConfig {
            ys: Some(vec!["margin".into()]),
            ..config
        };
        assert_eq!(resolve(&gone, &roles).ys, ["revenue", "cost"]);
    }

    /// Unpicking every Y is a choice, not a dead reference: it stays empty, and the read says
    /// so in the user's own terms rather than the engine's.
    #[test]
    fn an_emptied_y_stays_empty_and_says_which_kind_of_empty_it_is() {
        let roles = sales();
        let none = ChartConfig {
            ys: Some(Vec::new()),
            ..ChartConfig::default()
        };
        assert!(resolve(&none, &roles).ys.is_empty());
        assert_eq!(read(&none, &roles).unwrap_err().0, "Pick a column to plot");
    }

    /// The row index is a **choice**, so a result that happens to have a date column in it
    /// does not overrule it — the distinction `ChartX` exists to keep.
    #[test]
    fn charting_against_the_row_index_survives_a_temporal_column() {
        let roles = sales();
        let config = ChartConfig {
            x: ChartX::RowIndex,
            ..ChartConfig::default()
        };
        assert_eq!(resolve(&config, &roles).x, None);

        let pie = ChartConfig {
            mark: Some(ChartMark::Pie),
            ..config
        };
        assert_eq!(resolve(&pie, &roles).x.as_deref(), Some("month"));
    }

    /// **The menus are the constraint.** Each mark offers exactly what spec §4 says it takes,
    /// which is what makes an invalid encoding unreachable rather than reported.
    #[test]
    fn each_mark_offers_only_the_columns_it_can_take() {
        let roles = sales();

        assert_eq!(
            x_options(ChartMark::Bar, &roles),
            ["month", "country", "revenue", "cost"]
        );
        assert!(allows_row_index(ChartMark::Bar));

        assert_eq!(x_options(ChartMark::Scatter, &roles), ["revenue", "cost"]);
        assert!(!allows_row_index(ChartMark::Scatter));
        assert!(series_options(ChartMark::Scatter, &roles, Some("revenue")).is_empty());

        assert_eq!(x_options(ChartMark::Pie, &roles), ["month", "country"]);
        assert!(!takes_many_ys(ChartMark::Pie));
        assert!(series_options(ChartMark::Pie, &roles, Some("month")).is_empty());

        assert!(x_options(ChartMark::Histogram, &roles).is_empty());

        assert_eq!(
            series_options(ChartMark::Bar, &roles, Some("month")),
            ["country"]
        );
        assert!(series_options(ChartMark::Bar, &roles, None).is_empty());

        assert_eq!(y_options(&roles), ["revenue", "cost"]);
    }

    /// **The default mark follows the charted axis, not the result's column list** (spec §6).
    /// The distinction only shows once the user can set X — which is what this task added:
    /// putting a country on the axis of a result that also carries a date must not leave a
    /// line running across an unordered category.
    #[test]
    fn the_default_mark_reads_the_x_it_is_drawing_not_the_schema() {
        let roles = sales();
        assert_eq!(
            resolve(&ChartConfig::default(), &roles).mark,
            ChartMark::Line,
            "the derived X is the date"
        );

        for (x, expected) in [
            (ChartX::Column("country".into()), ChartMark::Bar),
            (ChartX::Column("revenue".into()), ChartMark::Bar),
            (ChartX::RowIndex, ChartMark::Bar),
            (ChartX::Column("month".into()), ChartMark::Line),
        ] {
            let config = ChartConfig {
                x: x.clone(),
                ..ChartConfig::default()
            };
            assert_eq!(resolve(&config, &roles).mark, expected, "{x:?}");
        }

        let chosen = ChartConfig {
            mark: Some(ChartMark::Area),
            x: ChartX::Column("country".into()),
            ..ChartConfig::default()
        };
        assert_eq!(resolve(&chosen, &roles).mark, ChartMark::Area);
    }

    /// **A bin count is part of the read, and only a histogram has one.** It reaches
    /// `ChartQuery`, so a new value is a new cache entry — and it is clamped where it is
    /// encoded as well as in the engine, because a control that accepts 5 000 over a read that
    /// answers 200 shows one thing and means another.
    #[test]
    fn a_bin_count_rides_in_the_read_clamped_and_only_for_a_histogram() {
        let roles = Roles::of(&[column("n", DataType::Int64)]);
        let histogram = |bins| ChartConfig {
            mark: Some(ChartMark::Histogram),
            bins,
            ..ChartConfig::default()
        };
        assert_eq!(
            read(&histogram(Some(24)), &roles),
            Ok(ChartQuery::Histogram {
                col: "n".into(),
                bins: Some(24),
            })
        );
        assert_eq!(
            read(&histogram(None), &roles),
            Ok(ChartQuery::Histogram {
                col: "n".into(),
                bins: None,
            })
        );
        assert_eq!(
            read(&histogram(Some(5_000)), &roles),
            Ok(ChartQuery::Histogram {
                col: "n".into(),
                bins: Some(MAX_BINS),
            })
        );

        let bar = ChartConfig {
            mark: Some(ChartMark::Bar),
            bins: Some(24),
            ..ChartConfig::default()
        };
        assert_eq!(resolve(&bar, &roles).bins, None);
        assert_eq!(bar.bins, Some(24));
    }

    /// **The two display transforms never reach the read.** Hiding a series and flipping the
    /// value axis are repaints, so the same columns under any of them encode to the same
    /// `ChartQuery` — which is what keeps them off cache identity.
    #[test]
    fn hiding_a_series_and_a_log_axis_leave_the_read_alone() {
        let roles = sales();
        let plain = ChartConfig {
            mark: Some(ChartMark::Line),
            ..ChartConfig::default()
        };
        let dressed = ChartConfig {
            hidden: vec!["cost".into()],
            log_y: true,
            ..plain.clone()
        };
        assert_eq!(read(&plain, &roles), read(&dressed, &roles));

        let stale = ChartConfig {
            hidden: vec!["margin".into()],
            ..plain
        };
        assert_eq!(resolve(&stale, &roles).hidden, ["margin"]);
    }

    /// **A log axis is offered where a mark plots position, not extent.** A bar and an area are
    /// read as area from a baseline, which a log axis has none of, so the preference is dropped
    /// from the encoding rather than drawn — and the config keeps it for the marks that can.
    #[test]
    fn only_the_marks_that_plot_position_resolve_a_log_axis() {
        let roles = sales();
        for (mark, expected) in [
            (ChartMark::Line, true),
            (ChartMark::Scatter, true),
            (ChartMark::Histogram, true),
            (ChartMark::Bar, false),
            (ChartMark::Area, false),
            (ChartMark::Pie, false),
        ] {
            assert_eq!(log_axis(mark), expected, "{mark:?}");
            let config = ChartConfig {
                mark: Some(mark),
                log_y: true,
                ..ChartConfig::default()
            };
            assert_eq!(resolve(&config, &roles).log_y, expected, "{mark:?}");
        }
    }

    /// A result with the columns a band or box plot maps: one category, one time column,
    /// and five measures the user's SQL computed.
    fn stats() -> Roles {
        Roles::of(&[
            column("day", DataType::Date32),
            column("region", DataType::Utf8),
            column("med", DataType::Float64),
            column("lo", DataType::Float64),
            column("hi", DataType::Float64),
            column("p25", DataType::Float64),
            column("p75", DataType::Float64),
        ])
    }

    /// **A heatmap's matrix is the pivot**: two categories and one measure, the series
    /// channel required — an unset second category takes the first remaining one as its
    /// default, because the matrix has no shape without it.
    #[test]
    fn a_heatmap_requires_two_categories_and_derives_the_second() {
        let roles = sales();
        let config = ChartConfig {
            mark: Some(ChartMark::Heatmap),
            ..ChartConfig::default()
        };
        let resolved = resolve(&config, &roles);
        assert_eq!(resolved.x.as_deref(), Some("month"), "the default X");
        assert_eq!(
            resolved.series.as_deref(),
            Some("country"),
            "the required series derives"
        );
        assert_eq!(
            read(&config, &roles),
            Ok(ChartQuery::Rows {
                x: Some("month".into()),
                ys: vec!["revenue".into()],
                series: Some("country".into()),
                cap: ROWS_CAP,
            })
        );
        assert!(series_required(ChartMark::Heatmap));
        assert!(!allows_row_index(ChartMark::Heatmap));
        assert_eq!(x_options(ChartMark::Heatmap, &roles), ["month", "country"]);

        let narrow = Roles::of(&[
            column("country", DataType::Utf8),
            column("revenue", DataType::Int64),
        ]);
        assert_eq!(
            read(&config, &narrow).unwrap_err().0,
            "Pick two category columns"
        );
    }

    /// **The band roles resolve like every reference** — kept where the result answers
    /// them, distinct from the Y and each other by construction, and dropped entirely for a
    /// mark that does not read them.
    #[test]
    fn band_roles_resolve_distinct_and_only_for_the_marks_that_read_them() {
        let roles = stats();
        let config = ChartConfig {
            mark: Some(ChartMark::Band),
            ys: Some(vec!["med".into()]),
            y_lo: Some("lo".into()),
            y_hi: Some("hi".into()),
            q1: Some("p25".into()),
            q3: Some("p75".into()),
            ..ChartConfig::default()
        };
        assert_eq!(
            read(&config, &roles),
            Ok(ChartQuery::Rows {
                x: Some("day".into()),
                ys: vec!["med".into(), "lo".into(), "hi".into()],
                series: None,
                cap: ROWS_CAP,
            }),
            "centre, lower, upper — the renderer reads by position"
        );
        let resolved = resolve(&config, &roles);
        assert_eq!(resolved.q1, None, "a band has no quartiles");
        assert_eq!(resolved.q3, None);

        let collided = ChartConfig {
            y_lo: Some("med".into()),
            ..config.clone()
        };
        assert_eq!(resolve(&collided, &roles).y_lo, None);
        let (title, body) = read(&collided, &roles).unwrap_err();
        assert_eq!(title, "Pick the band's bounds");
        assert!(body.contains("LOWER"), "{body}");

        let bar = ChartConfig {
            mark: Some(ChartMark::Bar),
            ..config
        };
        let resolved = resolve(&bar, &roles);
        assert_eq!(
            (resolved.y_lo, resolved.y_hi, resolved.q1, resolved.q3),
            (None, None, None, None)
        );
        assert_eq!(bar.y_lo.as_deref(), Some("lo"), "the config keeps them");
    }

    /// **A box plot encodes five fixed-order measures, the median first** — the sort's
    /// `ByYDesc` reads the first series, so a box plot sorts by its median.
    #[test]
    fn a_box_plot_encodes_median_first_and_names_what_is_missing() {
        let roles = stats();
        let config = ChartConfig {
            mark: Some(ChartMark::Box),
            x: ChartX::Column("region".into()),
            ys: Some(vec!["med".into()]),
            y_lo: Some("lo".into()),
            y_hi: Some("hi".into()),
            q1: Some("p25".into()),
            q3: Some("p75".into()),
            ..ChartConfig::default()
        };
        assert_eq!(
            read(&config, &roles),
            Ok(ChartQuery::Rows {
                x: Some("region".into()),
                ys: vec![
                    "med".into(),
                    "lo".into(),
                    "hi".into(),
                    "p25".into(),
                    "p75".into(),
                ],
                series: None,
                cap: ROWS_CAP,
            })
        );

        let partial = ChartConfig {
            q1: None,
            ..config.clone()
        };
        let (title, body) = read(&partial, &roles).unwrap_err();
        assert_eq!(title, "Pick the box's measures");
        assert!(body.contains("percentile_cont"), "{body}");

        let no_cats = Roles::of(&[
            column("med", DataType::Float64),
            column("lo", DataType::Float64),
        ]);
        assert_eq!(
            read(&config, &no_cats).unwrap_err().0,
            "Pick a category column"
        );
    }

    /// **The trendline resolves only for a scatter, and it never reaches the read.** The fit
    /// is its own entry keyed by the two columns, so the same encoding with the toggle on and
    /// off is one `ChartQuery` — toggling can never re-read the points.
    #[test]
    fn only_a_scatter_resolves_a_trendline_and_the_read_never_carries_it() {
        let roles = sales();
        for mark in ChartMark::ALL {
            let config = ChartConfig {
                mark: Some(mark),
                trend: true,
                ..ChartConfig::default()
            };
            assert_eq!(resolve(&config, &roles).trend, trendable(mark), "{mark:?}");
            assert_eq!(trendable(mark), mark == ChartMark::Scatter, "{mark:?}");
        }

        let plain = ChartConfig {
            mark: Some(ChartMark::Scatter),
            ..ChartConfig::default()
        };
        let fitted = ChartConfig {
            trend: true,
            ..plain.clone()
        };
        assert_eq!(read(&plain, &roles), read(&fitted, &roles));
    }

    /// A measure on X is not also a default Y: an untouched config must never plot a column
    /// against itself, and for a scatter that is the whole difference between two axes and a
    /// diagonal line.
    #[test]
    fn the_column_on_x_is_not_also_a_default_y() {
        let roles = sales();
        let config = ChartConfig {
            mark: Some(ChartMark::Bar),
            x: ChartX::Column("revenue".into()),
            ..ChartConfig::default()
        };
        assert_eq!(resolve(&config, &roles).ys, ["cost"]);
    }
}
