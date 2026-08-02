//! Chart reads over a snapshot (Rz2, `docs/CHART_SPEC.md` §5) — the grouped, raw and
//! binned queries behind the results Chart surface, answered as a small
//! [`ChartData`] the renderer can draw without touching a row of the result.
//!
//! **The aggregation is DataFusion's.** Every snapshot is already registered as
//! `__snap_{id}`, so a chart is a `GROUP BY` over a local table — an aggregated chart over
//! a multi-million-row result is a normal hash aggregation, and there is no client-side
//! reducer and no materialize cap. Built with the DataFrame API, like [`super::profile`]:
//! internal logic doesn't write SQL, only the user does.
//!
//! **It composes an algebra; it does not know what a candlestick is**
//! (`docs/CHART_FUNCTIONS.md` §2). One group slot, a *list* of measures, one pivot — so a
//! preset that wants four values per bucket sends four measures rather than a fifth code
//! path through here.
//!
//! Three things this module refuses to fake:
//!
//! - **A cap is a refusal, never a truncation.** Each read runs with `LIMIT cap + 1`; one
//!   row over means the answer is [`ChartData::OverCap`], carrying no data at all. There is
//!   no `TABLESAMPLE` in DataFusion and no silent `head()` here.
//! - **An empty bucket is a gap.** Neither `date_bin` nor a numeric bin emits a row for a
//!   bucket nothing fell in, so the bucket sequence is filled back in with `None` values
//!   (spec §5) — a renderer that joined bucket to bucket would otherwise draw a straight
//!   line across missing months.
//! - **A NULL X (or series) is its own group**, labelled `(null)`, and it is keyed by the
//!   value's identity rather than by that label — so a column that genuinely contains the
//!   string `(null)` yields two categories, not one silently merged.
//!
//! ## Category order is the measure, not the snapshot
//!
//! A temporal or numeric X orders by **value**, which needs no scan order at all. For a
//! categorical one `docs/CHART_SPEC.md` §5 proposed `min(row_number() OVER ())`, so a result
//! the user `ORDER BY`ed kept that order on the axis, and asked for the assumption to be
//! tested. **It does not hold**, and the failure is invisible at test sizes: an Arrow *File*
//! scan is range-split across `target_partitions` once the file passes
//! `datafusion.optimizer.repartition_file_min_size` (10 MB), and a window with no
//! `PARTITION BY` then sits above a `CoalescePartitionsExec`, whose own contract is "no
//! guarantees are made about the order of the resulting partition". Measured on stock
//! config: a 200k-row snapshot returned every row in file order, a 3M-row one put
//! 2 975 424 of 3 000 000 rows out of it. So the property would have held for small results
//! and silently reversed itself for exactly the large ones the chart exists for.
//!
//! Categorical categories are therefore ordered by **measure descending**, ties broken by
//! label ascending — deterministic, independent of scan parallelism, and the fallback the
//! spec named.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Months};
use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, Float64Array, RecordBatch, TimestampMillisecondArray,
};
use datafusion::arrow::compute::{cast as cast_array, concat_batches};
use datafusion::arrow::datatypes::{
    DataType, Float64Type, Int64Type, IntervalMonthDayNano, TimeUnit, TimestampMillisecondType,
};
use datafusion::arrow::util::display::ArrayFormatter;
use datafusion::common::ScalarValue;
use datafusion::functions::datetime::date_bin;
use datafusion::functions_aggregate::expr_fn::{avg, count, count_distinct, max, median, min, sum};
use datafusion::logical_expr::expr::ScalarFunction;
use datafusion::prelude::{cast, floor, ident, lit, DataFrame, Expr, SessionContext};

use strata_model::{
    AggFn, Bucket, CapUnit, ChartBin, ChartData, ChartPoint, ChartQuery, ChartSeries, Measure,
    SnapshotId, Stride, Width,
};

use super::query::{snapshot_name, CellFormat};
use crate::util::{clip, DISPLAY_CHARS};

/// What a NULL group reads as on an axis or in a legend (spec §5). Only ever a *label*:
/// groups are keyed by the value itself, so this never merges with a real `(null)` string.
const NULL_LABEL: &str = "(null)";

/// The single category of a chart with no X — the whole result, undivided. Whatever splits
/// it (a series column, a measure list, both) names itself in the legend, so the axis tick
/// says only what it covers.
const ALL_LABEL: &str = "all";

/// Read `snapshot` as a chart (`docs/CHART_SPEC.md` §5).
///
/// Snapshot-scoped and side-effect free, exactly like [`super::query::fetch_page`], which is
/// what lets the UI cache the answer by `(SnapshotId, ChartQuery)` with no confirm in front
/// of it: this is `fetch_page`-tier work, not the profile scan's tier.
pub async fn run_chart(
    ctx: &SessionContext,
    snapshot: SnapshotId,
    q: &ChartQuery,
    fmt: &CellFormat,
) -> Result<ChartData, String> {
    let df = ctx
        .table(snapshot_name(snapshot).as_str())
        .await
        .map_err(|e| e.to_string())?;
    match q {
        ChartQuery::Aggregate {
            x,
            series,
            measures,
            bucket,
            group_cap,
        } => {
            aggregate(
                df,
                x.as_deref(),
                series.as_deref(),
                measures,
                *bucket,
                *group_cap,
                fmt,
            )
            .await
        }
        ChartQuery::Raw { x, y, cap } => raw(df, x, y, *cap).await,
        ChartQuery::Histogram { col, bins } => histogram(df, col, *bins).await,
    }
}

// ---- the aggregate read (bar / line / area / pie, and every one-GROUP-BY preset) ----

/// How X is grouped, which is also what decides how its axis is ordered.
enum Grouping {
    /// Bucketed instants, read back as milliseconds. `auto` records that the engine chose the
    /// stride, which is what makes it free to choose again — see [`Grouping::wider`].
    Time {
        source: Expr,
        stride: Stride,
        auto: bool,
        /// What a bucket boundary is *rendered* as, which is not what it is grouped as.
        /// `date_bin` bins on the epoch value against a UTC origin and merely re-attaches the
        /// column's timezone, so labelling a bucket in that zone reads a January bucket as
        /// 7pm on 31 December. Boundaries are UTC, so they are labelled UTC. A column that
        /// was a date is rendered back as one, so its axis uses `date_format` like the grid
        /// rather than `timestamp_format`.
        label_as: DataType,
    },
    /// A column with an order of its own — a number, or a clock time. Grouped by raw value,
    /// or by **bin index** when a width is set.
    Ordered { expr: Expr, width: Option<Width> },
    /// Anything else, grouped on its raw value.
    Category { expr: Expr },
}

impl Grouping {
    /// The group expression, built fresh: a bucketed axis carries its *source* and its
    /// stride rather than the binned expression, so widening the stride is one field.
    fn expr(&self) -> Expr {
        match self {
            Grouping::Time { source, stride, .. } => binned(*stride, source.clone()),
            Grouping::Ordered { expr, .. } | Grouping::Category { expr } => expr.clone(),
        }
    }

    /// Whether this axis is ordered by the value it groups on. A bucketed or ordered axis is;
    /// a categorical one has no order of its own and is ranked by what it measures.
    fn orders_by_value(&self) -> bool {
        !matches!(self, Grouping::Category { .. })
    }

    /// The same grouping one rung wider, or `None` when there is no rung left **or the stride
    /// was the caller's**.
    ///
    /// This exists because the auto stride is chosen before the series cardinality is known
    /// and the cap counts categories × series: a ten-day span resolves to hourly, which fits
    /// the cap on its own and does not fit it split five ways. Guessing the split ahead of the
    /// query would need a second pass over the data; asking again with a wider bucket costs a
    /// pass only when the answer would otherwise have been a refusal.
    fn wider(&self) -> Option<Grouping> {
        match self {
            Grouping::Time {
                source,
                stride,
                auto: true,
                label_as,
            } => stride.wider().map(|stride| Grouping::Time {
                source: source.clone(),
                stride,
                auto: true,
                label_as: label_as.clone(),
            }),
            _ => None,
        }
    }

    /// What the answer reports as the bucket it used.
    fn bucket(&self) -> Option<Bucket> {
        match self {
            Grouping::Time { stride, .. } => Some(Bucket::Time(*stride)),
            Grouping::Ordered { width, .. } => width.map(Bucket::Width),
            Grouping::Category { .. } => None,
        }
    }
}

async fn aggregate(
    df: DataFrame,
    x: Option<&str>,
    series: Option<&str>,
    measures: &[Measure],
    bucket: Option<Bucket>,
    cap: usize,
    fmt: &CellFormat,
) -> Result<ChartData, String> {
    if measures.is_empty() {
        return Err("a chart needs at least one measure".into());
    }
    let mut group = match x {
        Some(name) => Some(grouping(&df, name, bucket, cap).await?),
        None if bucket.is_some() => {
            return Err("a chart with no X column has nothing to bucket".into())
        }
        None => None,
    };
    if let (Some(x), Some(series)) = (x, series) {
        if x == series {
            // DataFusion answers this one itself, with "Schema contains duplicate qualified
            // field name" — an internal message for an encoding mistake this module names in
            // its own words everywhere else.
            return Err(format!("'{x}' cannot be both the category and the series"));
        }
    }
    let series_expr = series.map(ident);

    // Run, and if what came back is more than the chart will draw, widen an engine-chosen
    // bucket and ask again rather than refuse a chart a wider rung would have drawn. Bounded
    // by the ladder, and it cannot loop for a bucket the caller named.
    //
    // The budget is spent **three** times, because three different things can overrun it and
    // only the first is the returned row count: a sparse decade at an hourly stride is two
    // rows and eighty-seven thousand filled buckets, and a dense axis crossed with a dense
    // series is a cell count neither of the other two sees. `group_cap` is documented as
    // bounding categories x series, so all three have to hold for that to be true.
    let (batch, axis, legend) = loop {
        let batch = grouped_rows(&df, group.as_ref(), &series_expr, measures, cap).await?;
        let rows = batch.num_rows();
        let drawn = (rows <= cap)
            .then(|| {
                // `aggregate` lays its schema out as group exprs then aggregate exprs, so the
                // columns are read positionally.
                let mut next = 0;
                let x_col = group.is_some().then(|| take(&batch, &mut next));
                let series_col = series_expr.is_some().then(|| take(&batch, &mut next));
                let axis = match (group.as_ref(), &x_col) {
                    (
                        Some(Grouping::Time {
                            stride, label_as, ..
                        }),
                        Some(col),
                    ) => temporal_axis(col, *stride, cap, label_as, fmt)?,
                    (Some(Grouping::Ordered { width: Some(w), .. }), Some(col)) => {
                        binned_axis(col, w.get(), cap, fmt)?
                    }
                    (Some(Grouping::Ordered { width: None, .. }), Some(col)) => {
                        Some(ordered_axis(col, fmt)?)
                    }
                    (Some(Grouping::Category { .. }), Some(col)) => Some(value_axis(col, fmt)?),
                    // No X: one category, covering everything.
                    _ => Some(Axis {
                        labels: vec![ALL_LABEL.into()],
                        of_row: vec![0; rows],
                    }),
                };
                let legend = match &series_col {
                    Some(col) => value_axis(col, fmt)?,
                    None => Axis {
                        labels: vec![String::new()],
                        of_row: vec![0; rows],
                    },
                };
                Ok::<_, String>(
                    axis.filter(|a| a.labels.len() * legend.labels.len() <= cap)
                        .map(|a| (a, legend)),
                )
            })
            .transpose()?
            .flatten();
        match drawn {
            Some((axis, legend)) => break (batch, axis, legend),
            None => match group.as_ref().and_then(Grouping::wider) {
                Some(wider) => group = Some(wider),
                None => return Ok(over_cap(cap, group.as_ref().and_then(Grouping::bucket))),
            },
        }
    };
    let rows = batch.num_rows();

    let mut next = usize::from(group.is_some()) + usize::from(series_expr.is_some());
    let mut values = Vec::with_capacity(measures.len());
    for _ in measures {
        values.push(numbers(&take(&batch, &mut next))?);
    }

    // One slot per (measure, series value) pair — measure-major, which is the order the
    // answer promises. A cell nothing was returned for stays `None`: an empty bucket, or a
    // pair the data never contained.
    //
    // The assignment is only sound while the axis is **injective** — every returned group row
    // in a cell of its own. That held silently until a binned axis folded four distinct keys
    // onto one category and the last write won, so the invariant is checked rather than
    // assumed: a collision is a defect in an axis builder, and it fails here instead of
    // reaching the renderer as a plausible number.
    let split = legend.labels.len();
    let mut cells = vec![vec![None; axis.labels.len()]; measures.len() * split];
    let mut filled = vec![false; axis.labels.len() * split];
    for row in 0..rows {
        let at = legend.of_row[row] * axis.labels.len() + axis.of_row[row];
        if std::mem::replace(&mut filled[at], true) {
            return Err(format!(
                "two aggregate rows landed in category {} of series {} — the axis is not \
                 one category per group",
                axis.of_row[row], legend.of_row[row]
            ));
        }
    }
    for (m, column) in values.iter().enumerate() {
        for row in 0..rows {
            cells[m * split + legend.of_row[row]][axis.of_row[row]] = column[row];
        }
    }

    // A value axis is already in its final order; a categorical one is ranked by what it
    // measures — see the module header.
    let categories = if group.as_ref().is_some_and(Grouping::orders_by_value) {
        axis.labels
    } else {
        let order = by_measure(&axis.labels, &category_totals(&cells, axis.labels.len()));
        for row in cells.iter_mut() {
            *row = permute(row, &order);
        }
        permute(&axis.labels, &order)
    };

    // Series values are ranked **once**, across every measure, so one value sits in the
    // same legend position throughout — and the measure order is the caller's, untouched,
    // because a multi-measure preset reads its parts back by it.
    let weights: Vec<f64> = (0..split)
        .map(|s| {
            (0..measures.len())
                .map(|m| cells[m * split + s].iter().flatten().sum::<f64>())
                .sum()
        })
        .collect();
    let order = by_measure(&legend.labels, &weights);
    let mut drawn = Vec::with_capacity(cells.len());
    for (m, measure) in measures.iter().enumerate() {
        for &s in &order {
            drawn.push(ChartSeries {
                name: series_name(measure, &legend.labels[s], series.is_some(), measures.len()),
                values: cells[m * split + s].clone(),
            });
        }
    }

    Ok(ChartData::Grouped {
        categories,
        series: drawn,
        bucket: group.as_ref().and_then(Grouping::bucket),
    })
}

/// The refusal every over-budget aggregate path answers with — one shape, so a second cap
/// check cannot invent a different one. It carries the bucket that was in effect, which is
/// the one thing the guardrail can suggest changing.
fn over_cap(cap: usize, bucket: Option<Bucket>) -> ChartData {
    ChartData::OverCap {
        unit: CapUnit::Groups,
        cap,
        bucket,
    }
}

/// One aggregate read, limited to `cap + 1` rows. `LIMIT cap + 1` is the whole cap
/// mechanism: one row over the budget and nothing is drawn. Unordered on purpose — *which*
/// rows come back only matters when we keep them, and we only keep them when every row fit.
async fn grouped_rows(
    df: &DataFrame,
    group: Option<&Grouping>,
    series: &Option<Expr>,
    measures: &[Measure],
    cap: usize,
) -> Result<RecordBatch, String> {
    let mut group_exprs = Vec::new();
    if let Some(g) = group {
        group_exprs.push(g.expr());
    }
    if let Some(e) = series {
        group_exprs.push(e.clone());
    }
    // Every measure is aliased to a name of our own, positionally. Two things collide
    // otherwise, and both are ordinary: a row-count measure's natural field name is the
    // literal `count(*)`, which is exactly what `SELECT region, count(*) FROM t GROUP BY 1`
    // names its column — and an unqualified aggregate field matching a qualified group column
    // is what `DFSchema` calls ambiguous. Two Y-less measures collide with each other for the
    // same reason. The decode is positional, so the names are never read back.
    let alias = measure_alias(&df);
    let aggregates = measures
        .iter()
        .enumerate()
        .map(|(i, m)| measure(m).alias(format!("{alias}{i}")))
        .collect();
    let plan = df
        .clone()
        .aggregate(group_exprs, aggregates)
        .map_err(|e| e.to_string())?
        .limit(0, Some(cap.saturating_add(1)))
        .map_err(|e| e.to_string())?;
    one_batch(plan).await
}

/// Drain a plan into a single batch. Five reads in this module want exactly this, and the
/// schema a batch is concatenated against has to be the plan's own.
async fn one_batch(plan: DataFrame) -> Result<RecordBatch, String> {
    let schema = plan.schema().inner().clone();
    let batches = plan.collect().await.map_err(|e| e.to_string())?;
    concat_batches(&schema, &batches).map_err(|e| e.to_string())
}

/// What splits one series from the others: whichever of the two axes is actually doing the
/// splitting, and both when both are.
fn series_name(measure: &Measure, value: &str, split_by_series: bool, measures: usize) -> String {
    match (split_by_series, measures) {
        (false, _) => measure.label(),
        (true, 1) => value.to_string(),
        (true, _) => format!("{value}: {}", measure.label()),
    }
}

/// How to group X, resolved from the column's own type and whatever bucket the request
/// named. A bucket of the wrong kind is **refused, not ignored**: a stale one left behind by
/// an encoding change would otherwise silently chart something the strip isn't showing.
async fn grouping(
    df: &DataFrame,
    name: &str,
    bucket: Option<Bucket>,
    cap: usize,
) -> Result<Grouping, String> {
    let dtype = field_type(df, name)?;
    if let Some(target) = bucket_type(&dtype) {
        let source = cast(ident(name), target);
        // The span pass runs for **every** temporal X, not only an open bucket, because it is
        // also the range check: `date_bin` converts its input to nanoseconds before binning
        // it, so an instant outside the nanosecond window overflows `i64` inside DataFusion —
        // an opaque "Invalid timestamp value" in a release build and an "attempt to multiply
        // with overflow" panic in a debug one. A far-future sentinel date is ordinary data
        // (`9999-12-31` is the usual "still current" end-date), so this has to be a refusal
        // the user can read rather than a fault they report.
        let range = range_ms(df, &source).await?;
        if let Some((lo, hi)) = range {
            if lo < MIN_BINNABLE_MS || hi > MAX_BINNABLE_MS {
                return Err(format!(
                    "'{name}' holds a date outside the range a time bucket covers (1678 to 2261)"
                ));
            }
        }
        let (stride, auto) = match bucket {
            Some(Bucket::Time(stride)) => (stride, false),
            Some(Bucket::Width(_)) => {
                return Err(format!(
                    "'{name}' is a time column, so it buckets by a stride"
                ))
            }
            None => (auto_stride(range, cap), true),
        };
        // A date renders as a date; every other instant renders in UTC, where its bucket
        // boundary actually sits.
        let label_as = match dtype {
            DataType::Date32 | DataType::Date64 => DataType::Date32,
            _ => DataType::Timestamp(TimeUnit::Millisecond, None),
        };
        return Ok(Grouping::Time {
            source,
            stride,
            auto,
            label_as,
        });
    }
    if dtype.is_numeric() || is_time_of_day(&dtype) {
        let width = match bucket {
            Some(Bucket::Width(width)) if dtype.is_numeric() => Some(width),
            // Both arms below are a time-of-day column, which buckets neither way: it has no
            // calendar for a stride and no meaningful width. Naming it a number and pointing
            // at a width the arm above refuses would send the user in a circle.
            Some(Bucket::Width(_)) => {
                return Err(format!(
                    "'{name}' is a time of day, so it has no bucket to set"
                ))
            }
            Some(Bucket::Time(_)) if is_time_of_day(&dtype) => {
                return Err(format!(
                    "'{name}' is a time of day, so it has no bucket to set"
                ))
            }
            Some(Bucket::Time(_)) => {
                return Err(format!("'{name}' is a number, so it buckets by a width"))
            }
            // Deliberately not auto-resolved: a numeric X grouped by its own values is the
            // honest default (spec §5), and a width is something the user turns on.
            None => None,
        };
        let expr = match width {
            // Group on the bin **index**, not on `floor(x / w) * w`. The same buckets, but the
            // key is a whole number, so filling the empty bins is exact rather than a float
            // comparison. Left as `Float64` rather than cast to `Int64` in the plan: that cast
            // is strict, so an index outside `i64` — a tiny width over a wide column — would
            // fail the read with an Arrow message instead of reaching the cap refusal that
            // exists for exactly that case.
            Some(w) => floor(cast(ident(name), DataType::Float64) / lit(w.get())),
            None => ident(name),
        };
        return Ok(Grouping::Ordered { expr, width });
    }
    if bucket.is_some() {
        return Err(format!("'{name}' is not a column a chart can bucket"));
    }
    Ok(Grouping::Category { expr: ident(name) })
}

/// The instants `date_bin` can bin, as milliseconds. It multiplies up to nanoseconds in an
/// `i64`, so the window is that type's range divided by a million — 1678-01-01 to 2261-12-31,
/// give or take.
const MIN_BINNABLE_MS: i64 = i64::MIN / 1_000_000;
const MAX_BINNABLE_MS: i64 = i64::MAX / 1_000_000;

/// A clock time with no date. Orders by value like a number, but has no calendar to bin
/// against — `date_bin` takes it only with a sub-day stride and wraps it around midnight.
fn is_time_of_day(dtype: &DataType) -> bool {
    matches!(dtype, DataType::Time32(_) | DataType::Time64(_))
}

/// A measure-alias prefix no column of this result starts with, so `{prefix}{i}` can collide
/// with neither a group column nor another measure. Terminates: each round lengthens the
/// prefix and the column names are finite.
fn measure_alias(df: &DataFrame) -> String {
    let mut prefix = String::from("m_");
    while df
        .schema()
        .fields()
        .iter()
        .any(|f| f.name().starts_with(&prefix))
    {
        prefix.push('_');
    }
    prefix
}

/// Counting rows, spelled so the output field cannot collide with a snapshot column.
///
/// **Not `count_all()`**, which is `count(1)` aliased to the literal string `count(*)`.
/// That alias is unqualified while a group column is qualified, and `DFSchema` calls an
/// unqualified field that matches a qualified one ambiguous — so charting a result of
/// `SELECT region, count(*) FROM t GROUP BY 1` (whose second column really is named
/// `count(*)`) with that column on X failed the read with a schema error. Two Y-less measures
/// in one request collided with each other the same way. Without the alias the field is
/// `count(Int64(1))`, which no query produces by accident, and the decode is positional so
/// the name is never read.
fn rows_measure() -> Expr {
    count(lit(COUNT_ROWS))
}

/// The literal `count` counts, kept out of line so the two callers spell it identically.
const COUNT_ROWS: i64 = 1;

/// The aggregate expression for one measure. A measure with no Y counts rows, whatever
/// function it names — the same rule [`Measure::label`] renders.
fn measure(measure: &Measure) -> Expr {
    // `ident`, not `col`: `col` parses its argument (so a column named `a.b` becomes
    // relation `a` column `b`) and lower-cases it, and a result column's name is whatever
    // the user's query produced.
    let Some(y) = measure.y.as_deref().map(ident) else {
        return rows_measure();
    };
    match measure.agg_fn {
        AggFn::Sum => sum(y),
        AggFn::Avg => avg(y),
        AggFn::Min => min(y),
        AggFn::Max => max(y),
        AggFn::Count => count(y),
        AggFn::Median => median(y),
        AggFn::CountDistinct => count_distinct(y),
    }
}

/// `date_bin(interval, x)` — the two-argument form, whose origin is the Unix epoch. That is
/// midnight on the first of a month, which is exactly what `date_bin` requires of a
/// calendar-stepped stride, and it needs no timezone-matching origin literal of our own.
fn binned(stride: Stride, x: Expr) -> Expr {
    let (months, days, nanos) = stride.parts();
    let interval = lit(ScalarValue::IntervalMonthDayNano(Some(
        IntervalMonthDayNano::new(months, days, nanos),
    )));
    Expr::ScalarFunction(ScalarFunction::new_udf(date_bin(), vec![interval, x]))
}

/// The timestamp a temporal X is binned as, or `None` for a column `date_bin` shouldn't
/// touch.
///
/// Everything bucketable lands on **milliseconds**, so one decode path reads every bucket
/// back. The unit costs nothing: the narrowest stride is a minute.
///
/// `Date32`/`Date64` are cast rather than left to argument coercion — `date_bin`'s
/// signature takes timestamps and times only, and while `Date32` happens to coerce into
/// `Timestamp(Nanosecond, None)`, `Date64` does not, so a date column would resolve or fail
/// depending on its width.
///
/// `Time32`/`Time64` deliberately aren't bucketed. `date_bin` accepts them, but only with a
/// stride under a day and with wrap-around semantics of its own; a time-of-day column
/// groups on its raw value instead, like any other dimension.
fn bucket_type(dtype: &DataType) -> Option<DataType> {
    match dtype {
        DataType::Timestamp(_, tz) => Some(DataType::Timestamp(TimeUnit::Millisecond, tz.clone())),
        DataType::Date32 | DataType::Date64 => {
            Some(DataType::Timestamp(TimeUnit::Millisecond, None))
        }
        _ => None,
    }
}

/// Pick a bucket width for a temporal X from the span it covers (spec §5), then widen it
/// until the axis plausibly fits under `cap`.
///
/// The widening is the part the spec doesn't state, and it exists because the ladder alone
/// produces axes that cannot be drawn: 60 days at the ladder's hourly rung is 1 440 buckets
/// against a default cap of 1 000, so "chart my last two months" would refuse by
/// construction. A **default** that guarantees a refusal is not a default. The count here is
/// an estimate — calendar rungs have no fixed width — and it only chooses; the exact axis
/// length is enforced later, against the buckets that actually came back.
///
/// A stride the *request* named is never widened: the user asked for it, and a refusal is
/// the honest answer to a bucket that doesn't fit.
fn auto_stride(range: Option<(i64, i64)>, cap: usize) -> Stride {
    let span = range.map_or(0, |(lo, hi)| hi.saturating_sub(lo));
    const DAY: i64 = 86_400_000;
    let mut stride = if span > 730 * DAY {
        Stride::Month
    } else if span > 60 * DAY {
        Stride::Day
    } else if span > 2 * DAY {
        Stride::Hour
    } else {
        Stride::FiveMinutes
    };
    while buckets_over(span, stride, cap) {
        match stride.wider() {
            Some(wider) => stride = wider,
            // Already the widest rung; the exact check refuses if it still doesn't fit.
            None => break,
        }
    }
    stride
}

/// Whether `span` at `stride` would need more than `cap` buckets. Approximate by design —
/// a month is not a fixed width — because this only picks a default (see [`auto_stride`]).
fn buckets_over(span: i64, stride: Stride, cap: usize) -> bool {
    let (months, days, nanos) = stride.parts();
    // Mean Gregorian month, so a calendar rung is measured against the calendar it steps
    // through rather than against 30 days flat.
    let width = if months != 0 {
        months as i64 * 2_629_746_000
    } else {
        days as i64 * 86_400_000 + nanos / 1_000_000
    };
    span / width.max(1) >= i64::try_from(cap).unwrap_or(i64::MAX)
}

/// `(min(x), max(x))` in milliseconds, or `None` when every value is NULL.
async fn range_ms(df: &DataFrame, x: &Expr) -> Result<Option<(i64, i64)>, String> {
    let plan = df
        .clone()
        .aggregate(vec![], vec![min(x.clone()), max(x.clone())])
        .map_err(|e| e.to_string())?;
    let batch = one_batch(plan).await?;
    let mut next = 0;
    let lo = millis(&take(&batch, &mut next))?;
    let hi = millis(&take(&batch, &mut next))?;
    Ok(
        match (lo.first().copied().flatten(), hi.first().copied().flatten()) {
            (Some(lo), Some(hi)) => Some((lo, hi)),
            _ => None,
        },
    )
}

/// The category axis of one aggregate read: the labels in draw order, and which category
/// each returned row belongs to.
struct Axis {
    labels: Vec<String>,
    of_row: Vec<usize>,
}

impl Axis {
    /// No groups, so no categories. What an aggregate over an empty result produces.
    fn empty() -> Axis {
        Axis {
            labels: Vec::new(),
            of_row: Vec::new(),
        }
    }
}

/// The axis for a bucketed temporal X: every bucket from the first to the last, ascending,
/// **including the ones no row fell in** — the gaps a renderer must not draw across.
///
/// `None` when the filled sequence would run past `cap`. That check has to happen here
/// rather than on the returned row count: two rows a decade apart at an hourly stride are
/// two aggregate rows and eighty-seven thousand buckets.
///
/// The sequence is stepped from a bucket `date_bin` itself produced, so its alignment is
/// inherited rather than re-derived — no second implementation of `date_bin`'s calendar
/// rules to drift from the first. A returned bucket the walk misses is a genuine
/// disagreement, and it fails loudly instead of dropping that bucket's data.
fn temporal_axis(
    col: &ArrayRef,
    stride: Stride,
    cap: usize,
    label_as: &DataType,
    fmt: &CellFormat,
) -> Result<Option<Axis>, String> {
    // Any timestamp unit, not the millisecond one we cast the source to: `date_bin`'s
    // signature lists its exact forms nanoseconds-first, and a timezone-stamped input coerces
    // up to that variant, so the bucket column comes back as `Timestamp(ns, tz)`. Insisting on
    // milliseconds here failed every zoned temporal X outright. `millis` casts whatever
    // arrives, so the unit is not something this function needs to care about.
    if !matches!(col.data_type(), DataType::Timestamp(_, _)) {
        return Err(format!(
            "bucketed X came back as {}, not a timestamp",
            col.data_type()
        ));
    }
    let buckets = millis(col)?;
    let has_null = buckets.iter().any(|b| b.is_none());
    let Some(lo) = buckets.iter().flatten().min().copied() else {
        return Ok(Some(all_null_axis(buckets.len())));
    };
    let hi = buckets.iter().flatten().max().copied().unwrap_or(lo);

    // The NULL bucket is a category too, so it comes out of the same budget — otherwise a
    // full axis plus a NULL group draws one category past the cap it was just checked
    // against.
    let budget = cap.saturating_sub(usize::from(has_null));
    let mut seq = Vec::new();
    let mut step = 0;
    loop {
        let at = advance(lo, stride, step).ok_or("temporal axis ran out of range")?;
        if at > hi {
            break;
        }
        if seq.len() >= budget {
            return Ok(None);
        }
        seq.push(at);
        step += 1;
    }
    let index: HashMap<i64, usize> = seq.iter().enumerate().map(|(i, at)| (*at, i)).collect();

    let sequence: ArrayRef = Arc::new(TimestampMillisecondArray::from(seq.clone()));
    let sequence = cast_array(&sequence, label_as).map_err(|e| e.to_string())?;
    let (labels, null_at) = with_null_label(strings(&sequence, fmt)?, has_null);

    let mut of_row = Vec::with_capacity(buckets.len());
    for bucket in &buckets {
        of_row.push(match bucket {
            Some(at) => *index.get(at).ok_or_else(|| {
                format!(
                    "bucket {at} is not on a {stride} sequence from {lo}: date_bin and the \
                     axis walk disagree"
                )
            })?,
            None => null_at.ok_or("a NULL bucket appeared after the axis was built")?,
        });
    }
    Ok(Some(Axis { labels, of_row }))
}

/// The axis for a numeric X binned to a uniform width: every bin from the first to the last,
/// ascending, empty ones included — the numeric analog of [`temporal_axis`], and `None` past
/// `cap` for the same reason.
///
/// The group key is the **bin index**, so this is integer arithmetic throughout, and a bin
/// start is `index × width` — the same expression the SQL scaffold writes, computed the same
/// way for every bin whether a row landed in it or not. Matching returned bucket *starts*
/// against a generated sequence would have compared floats that agree mathematically and
/// differ in their last bit.
/// A bin key that is not a position on the axis. The group column is a `Float64`, so
/// DataFusion hands back one group row per distinct key — and a NULL, a NaN and each infinity
/// are **four different groups**, not one. Folding them together is what let two of them share
/// a category and silently overwrite each other in the pivot, so each keeps its own identity
/// here and gets its own tick.
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
enum Unplaced {
    Null,
    /// The key's bit pattern, so two distinct non-finite keys stay two categories.
    NotFinite(u64),
}

fn binned_axis(
    col: &ArrayRef,
    width: f64,
    cap: usize,
    fmt: &CellFormat,
) -> Result<Option<Axis>, String> {
    let index = numbers(col)?;
    if index.is_empty() {
        return Ok(Some(Axis::empty()));
    }
    // A finite index is a bin, whatever its magnitude: an index past 2^53 has `f64` granularity
    // rather than integer granularity, but that granularity is the *query's* — the group key is
    // the float DataFusion grouped on — so nothing is lost by placing it, and the span check
    // below refuses the range if it is genuinely too wide. Excluding those keys instead charted
    // real rows as `(null)`.
    let place = |k: Option<f64>| match k {
        None => Err(Unplaced::Null),
        Some(k) if k.is_finite() => Ok(k),
        Some(k) => Err(Unplaced::NotFinite(k.to_bits())),
    };
    let placed: Vec<Result<f64, Unplaced>> = index.iter().map(|k| place(*k)).collect();

    // One category per distinct unplaceable key, in first-seen order, after the sequence.
    let mut off_axis: Vec<Unplaced> = Vec::new();
    for key in placed.iter().filter_map(|p| p.err()) {
        if !off_axis.contains(&key) {
            off_axis.push(key);
        }
    }

    let Some(lo) = placed.iter().filter_map(|p| p.ok()).reduce(f64::min) else {
        // Nothing sits on the axis, but the unplaceable groups are still groups.
        return Ok(Some(off_axis_only(
            &placed,
            &off_axis,
            index.len(),
            col,
            fmt,
        )?));
    };
    let hi = placed
        .iter()
        .filter_map(|p| p.ok())
        .reduce(f64::max)
        .unwrap_or(lo);

    let budget = cap.saturating_sub(off_axis.len()) as f64;
    // Compared as `f64` because that is what the index is: a tiny width over a wide column
    // spans more bins than an `i64` subtraction could hold, and it has to reach this check as
    // a big number rather than as a wrapped small one or an Arrow cast failure.
    if hi - lo + 1.0 > budget {
        return Ok(None);
    }
    let span = (hi - lo) as usize;
    // Labels come from the *decimal* of the width, not from repeated `f64` multiplication:
    // `k * 0.1` prints `0.30000000000000004` at k = 3, and this axis is read by a human. The
    // scaled-integer form is exact for every width a control can produce.
    let starts: Vec<f64> = (0..=span)
        .map(|k| bin_start(lo + k as f64, width))
        .collect();
    let sequence: ArrayRef = Arc::new(Float64Array::from(starts));
    let mut labels = strings(&sequence, fmt)?;
    let first_off = labels.len();
    labels.extend(off_axis_labels(&off_axis, col, fmt)?);

    let mut of_row = Vec::with_capacity(index.len());
    for bin in &placed {
        of_row.push(match bin {
            Ok(k) => (k - lo) as usize,
            Err(key) => first_off + position(&off_axis, key)?,
        });
    }
    Ok(Some(Axis { labels, of_row }))
}

/// The axis when every bin key was unplaceable — all NULL, all NaN, or a mix. Still one
/// category per distinct key.
fn off_axis_only(
    placed: &[Result<f64, Unplaced>],
    off_axis: &[Unplaced],
    rows: usize,
    col: &ArrayRef,
    fmt: &CellFormat,
) -> Result<Axis, String> {
    let labels = off_axis_labels(off_axis, col, fmt)?;
    let mut of_row = Vec::with_capacity(rows);
    for bin in placed {
        of_row.push(match bin {
            Ok(_) => return Err("a placeable bin reached the off-axis path".into()),
            Err(key) => position(off_axis, key)?,
        });
    }
    Ok(Axis { labels, of_row })
}

/// How each unplaceable key reads: `(null)` for a NULL, and arrow's own rendering (`NaN`,
/// `inf`) for a value that is a number's bit pattern but not a position.
fn off_axis_labels(
    off_axis: &[Unplaced],
    col: &ArrayRef,
    fmt: &CellFormat,
) -> Result<Vec<String>, String> {
    let values: Vec<Option<f64>> = off_axis
        .iter()
        .map(|key| match key {
            Unplaced::Null => None,
            Unplaced::NotFinite(bits) => Some(f64::from_bits(*bits)),
        })
        .collect();
    let _ = col;
    let array: ArrayRef = Arc::new(Float64Array::from(values));
    strings(&array, fmt)
}

fn position(off_axis: &[Unplaced], key: &Unplaced) -> Result<usize, String> {
    off_axis
        .iter()
        .position(|k| k == key)
        .ok_or_else(|| "a bin key appeared after the axis was built".into())
}

/// The value a bin starts at: `index × width`, rounded back onto the grid the width defines
/// so the axis reads in the units the user asked for. `0.1 × 3` is `0.30000000000000004` in
/// binary floating point; the nearest representable tenth is what belongs on a tick.
fn bin_start(index: f64, width: f64) -> f64 {
    let start = index * width;
    // How many decimals the width itself carries, bounded — beyond that the product is
    // already the shortest representation of itself.
    let places = (0..=12).find(|p| {
        let scale = 10f64.powi(*p);
        (width * scale).fract() == 0.0
    });
    match places {
        Some(p) => {
            let scale = 10f64.powi(p);
            (start * scale).round() / scale
        }
        None => start,
    }
}

/// The axis for a column with an order of its own, grouped by raw value: every distinct
/// value, ascending, with a NULL last.
///
/// Covers a clock time as well as a number. `Time32`/`Time64` are not `is_numeric()` and
/// cannot be `date_bin`ned, which is what used to route them to the categorical branch and
/// rank a time-of-day axis by measure — 17:00 before 09:00 because the afternoon sold more.
fn ordered_axis(col: &ArrayRef, fmt: &CellFormat) -> Result<Axis, String> {
    let axis = value_axis(col, fmt)?;
    let values = ordering_key(col)?;
    // One value per category, from the first row that landed in it — every row in a
    // category holds the same group key, so the first is the category's value.
    let mut of_category: Vec<Option<f64>> = vec![None; axis.labels.len()];
    for (row, at) in axis.of_row.iter().enumerate() {
        if of_category[*at].is_none() {
            of_category[*at] = values[row];
        }
    }
    // Ascending, with anything that has no position on a number line — a NULL group, a NaN —
    // after everything that does, and the label breaking a tie.
    //
    // The tiebreak is not decoration. `numbers` reads the key as `f64`, so two distinct
    // integer keys past 2^53 collapse to one value here; without it they would fall back to
    // the order the hash aggregate emitted them in, which is the one thing this axis promises
    // not to depend on. `total_cmp` rather than `partial_cmp(..).unwrap_or(Equal)` for the
    // reason spelled out on [`by_measure`]: the latter is intransitive and `sort_by` panics on
    // it.
    let mut order: Vec<usize> = (0..axis.labels.len()).collect();
    order.sort_by(|&a, &b| {
        let place = |v: Option<f64>| match v {
            Some(v) if !v.is_nan() => (0, v),
            _ => (1, 0.0),
        };
        let (side_a, value_a) = place(of_category[a]);
        let (side_b, value_b) = place(of_category[b]);
        side_a
            .cmp(&side_b)
            .then_with(|| value_a.total_cmp(&value_b))
            .then_with(|| axis.labels[a].cmp(&axis.labels[b]))
    });
    Ok(reorder(axis, &order))
}

/// The axis for a group key taken at face value — one category per distinct value, in the
/// order the rows arrived (the caller reorders).
///
/// Keyed by the **value**, not by its label: a NULL renders as `(null)`, and so does a
/// column that genuinely holds that string. Keying on the rendering would merge the two and
/// lose one group's data without saying so.
fn value_axis(col: &ArrayRef, fmt: &CellFormat) -> Result<Axis, String> {
    let labels = strings(col, fmt)?;
    let mut seen: HashMap<ScalarValue, usize> = HashMap::new();
    let mut ordered = Vec::new();
    let mut of_row = Vec::with_capacity(col.len());
    for row in 0..col.len() {
        let key = ScalarValue::try_from_array(col, row).map_err(|e| e.to_string())?;
        let at = match seen.get(&key) {
            Some(at) => *at,
            None => {
                let at = ordered.len();
                seen.insert(key, at);
                ordered.push(labels[row].clone());
                at
            }
        };
        of_row.push(at);
    }
    Ok(Axis {
        labels: ordered,
        of_row,
    })
}

/// The axis of a bucketed X whose every value was NULL: one group, and it is the null one.
///
/// **Zero rows is not one null group.** An aggregate over an empty result returns no groups at
/// all, and answering with a `(null)` tick asserts a group nothing created — an empty result
/// draws an empty axis.
fn all_null_axis(rows: usize) -> Axis {
    if rows == 0 {
        return Axis::empty();
    }
    Axis {
        labels: vec![NULL_LABEL.into()],
        of_row: vec![0; rows],
    }
}

/// Append the NULL category to a filled sequence, if there is one — **after** it, where it
/// cannot imply a position on a time or number axis. Returns where it landed.
fn with_null_label(mut labels: Vec<String>, has_null: bool) -> (Vec<String>, Option<usize>) {
    let at = has_null.then(|| {
        labels.push(NULL_LABEL.into());
        labels.len() - 1
    });
    (labels, at)
}

/// Reorder an axis's categories, keeping every row pointing at the category it was in.
fn reorder(axis: Axis, order: &[usize]) -> Axis {
    let mut at = vec![0; order.len()];
    for (to, &from) in order.iter().enumerate() {
        at[from] = to;
    }
    Axis {
        labels: permute(&axis.labels, order),
        of_row: axis.of_row.into_iter().map(|c| at[c]).collect(),
    }
}

/// `step` strides on from `from`. Calendar strides step through chrono so they land where
/// `date_bin`'s own month arithmetic does; fixed ones are plain milliseconds.
fn advance(from: i64, stride: Stride, step: i64) -> Option<i64> {
    let (months, days, nanos) = stride.parts();
    if months != 0 {
        let months = u32::try_from(months as i64 * step).ok()?;
        return DateTime::from_timestamp_millis(from)?
            .checked_add_months(Months::new(months))
            .map(|at| at.timestamp_millis());
    }
    let width = days as i64 * 86_400_000 + nanos / 1_000_000;
    from.checked_add(step.checked_mul(width)?)
}

/// The permutation ordering `labels` by `weight` descending, ties by label ascending —
/// **total**, so the same data always draws in the same order.
///
/// Totality is the whole of it, and it is not free: `partial_cmp(..).unwrap_or(Equal)` looks
/// like a safe reading of a NaN weight and is the one thing that breaks the order. NaN
/// compares `Equal` to every real weight while those weights still order among themselves,
/// which is intransitive, and the label tiebreak does not repair it — it fires *because* of
/// the `Equal`, so it makes the cycle rather than closing it. `slice::sort_by` detects the
/// violation and **panics** above its insertion-sort threshold: measured on rustc 1.94, 21
/// categories with one NaN weight abort the whole chart read, and 20 or fewer come back in an
/// arbitrary order.
fn by_measure(labels: &[String], weight: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..labels.len()).collect();
    order.sort_by(|&a, &b| heaviest(weight[a], weight[b]).then_with(|| labels[a].cmp(&labels[b])));
    order
}

/// Descending, with a NaN last. Total: it is `(is_nan, -value)` compared lexicographically,
/// and `total_cmp` orders every other pair of `f64`s including the infinities.
///
/// A NaN measure is not a quantity, so it ranks below every quantity rather than above the
/// largest — which is where `total_cmp` alone would put a positive NaN.
fn heaviest(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => b.total_cmp(&a),
    }
}

/// What each category is worth down the whole chart — one value per category, summed across
/// every series. Absent cells contribute nothing, which is what ranks a sparse category
/// below a dense one.
fn category_totals(cells: &[Vec<Option<f64>>], len: usize) -> Vec<f64> {
    (0..len)
        .map(|at| cells.iter().filter_map(|series| series[at]).sum())
        .collect()
}

fn permute<T: Clone>(items: &[T], order: &[usize]) -> Vec<T> {
    order.iter().map(|&i| items[i].clone()).collect()
}

// ---- the raw read (scatter) ----

async fn raw(df: DataFrame, x: &str, y: &str, cap: usize) -> Result<ChartData, String> {
    // Checked before the cast rather than after. The cast DataFusion plans is the **strict**
    // one, so a text column fails the read with an Arrow message about a string it could not
    // parse; the aggregate path answers the same mistake by naming the column's type, and one
    // module should not give two answers to one encoding error.
    plottable(&df, x)?;
    plottable(&df, y)?;
    // Filtered before the limit, so the cap counts points that can actually be drawn. A row
    // missing either coordinate has no position on the plane at all — that is not the
    // sampling §1.4 rules out, it is the absence of a point.
    // Finite, not merely non-NULL. Arrow's null bitmap is unset for a NaN, so filtering NULLs
    // alone let `ChartPoint { x: NaN }` out of here — a mark with no position, counted against
    // a cap this function's own comment says counts points that can be drawn. Every other path
    // in this module already excludes non-finite values.
    let plan = df
        .filter(finite(ident(x)).and(finite(ident(y))))
        .map_err(|e| e.to_string())?
        .select(vec![
            cast(ident(x), DataType::Float64).alias("x"),
            cast(ident(y), DataType::Float64).alias("y"),
        ])
        .map_err(|e| e.to_string())?
        .limit(0, Some(cap.saturating_add(1)))
        .map_err(|e| e.to_string())?;
    let batch = one_batch(plan).await?;
    if batch.num_rows() > cap {
        return Ok(ChartData::OverCap {
            unit: CapUnit::Points,
            cap,
            bucket: None,
        });
    }
    let mut next = 0;
    let xs = numbers(&take(&batch, &mut next))?;
    let ys = numbers(&take(&batch, &mut next))?;
    Ok(ChartData::Points(
        xs.into_iter()
            .zip(ys)
            .filter_map(|(x, y)| Some(ChartPoint { x: x?, y: y? }))
            .collect(),
    ))
}

// ---- the binned read (histogram) ----

async fn histogram(df: DataFrame, col: &str, bins: Option<usize>) -> Result<ChartData, String> {
    plottable(&df, col)?;
    let value = cast(ident(col), DataType::Float64);
    // Non-finite values are filtered out of **both** passes, not guarded after the first.
    // Arrow's `max` reports a NaN as greater than every real value, so one NaN row makes the
    // maximum NaN, the width NaN, and the strict cast of `floor(x / NaN)` fails the whole
    // read — for a column pandas and Spark write NaN into routinely. `x > -inf AND x < inf`
    // is exactly the finite predicate: NaN fails both comparisons and each infinity fails one.
    let df = df
        .filter(finite(value.clone()))
        .map_err(|e| e.to_string())?;
    let plan = df
        .clone()
        .aggregate(
            vec![],
            vec![min(value.clone()), max(value.clone()), count(value.clone())],
        )
        .map_err(|e| e.to_string())?;
    let batch = one_batch(plan).await?;
    let mut next = 0;
    let lo = numbers(&take(&batch, &mut next))?;
    let hi = numbers(&take(&batch, &mut next))?;
    let rows = integers(&take(&batch, &mut next))?;
    let (Some(Some(lo)), Some(Some(hi)), Some(Some(rows))) = (lo.first(), hi.first(), rows.first())
    else {
        // Nothing to bin: every value is NULL or non-finite, which is a histogram of nothing
        // rather than a histogram of zeroes.
        return Ok(ChartData::Bins(Vec::new()));
    };
    let (lo, hi, rows) = (*lo, *hi, *rows);
    let bins = bins.unwrap_or_else(|| auto_bins(rows)).clamp(1, MAX_BINS);
    if hi <= lo {
        // One value, however many rows carry it — a width of zero has no bins to divide.
        return Ok(ChartData::Bins(vec![ChartBin {
            lo,
            hi,
            count: rows as u64,
        }]));
    }

    let width = (hi - lo) / bins as f64;
    let plan = df
        .aggregate(
            vec![floor((value - lit(lo)) / lit(width)).alias("bin")],
            vec![rows_measure()],
        )
        .map_err(|e| e.to_string())?;
    let batch = one_batch(plan).await?;
    let mut next = 0;
    let index = numbers(&take(&batch, &mut next))?;
    let counts = integers(&take(&batch, &mut next))?;

    let mut binned = vec![0u64; bins];
    for (at, n) in index.into_iter().zip(counts) {
        // A NULL index is a NULL value — not a number, so not in any bin. The maximum
        // value divides out to exactly `bins`, which belongs in the last one: bins are
        // half-open but the range's top edge has to land somewhere.
        let (Some(at), Some(n)) = (at, n) else {
            continue;
        };
        let at = (at.max(0.0) as usize).min(bins - 1);
        binned[at] += n.max(0) as u64;
    }
    Ok(ChartData::Bins(
        binned
            .into_iter()
            .enumerate()
            .map(|(i, count)| ChartBin {
                lo: lo + i as f64 * width,
                // The last edge is the measured maximum, not `lo + bins * width`, which
                // floating-point accumulation would leave a hair off it.
                hi: if i + 1 == bins {
                    hi
                } else {
                    lo + (i + 1) as f64 * width
                },
                count,
            })
            .collect(),
    ))
}

/// The most bins a request may ask for. Not a guardrail like the group cap — a histogram is
/// a *picture* of a distribution, and past a couple of hundred bars there are more bins than
/// the canvas has columns of pixels. It is also what keeps a bin count that arrived as a
/// number from allocating against it.
const MAX_BINS: usize = 200;

/// Bin count when the request leaves it open: `√n`, floored at 6 and capped at 24 — enough
/// shape to read, few enough bars to label. The rule is the task file's
/// (`.claude/tasks/workstream-chart-view/01-engine-chart-data.md`), not the spec's, which
/// says only that the engine picks from the row count.
fn auto_bins(rows: i64) -> usize {
    let root = (rows as f64).sqrt().ceil() as usize;
    root.clamp(6, 24)
}

// ---- decoding ----

/// The next result column, advancing the cursor. Results are read **by position** — an
/// aggregate's schema is its group expressions then its aggregates, so there is nothing to
/// match names on and nothing for an alias to collide with.
fn take(batch: &RecordBatch, next: &mut usize) -> ArrayRef {
    let col = batch.column(*next).clone();
    *next += 1;
    col
}

/// One column as `f64`s. Cast rather than matched per type, so an `Int64` count, a
/// `Float64` average and a `Decimal128` sum all decode the same way.
///
/// A non-numeric column is **refused, not cast**. Arrow's default cast is the lenient one:
/// `Utf8` → `Float64` turns every unparseable string into a NULL, so `min` over a text
/// column — the one measure DataFusion plans happily for a non-numeric Y — would come back
/// as a chart of empty cells rather than as the encoding error it is.
fn numbers(col: &ArrayRef) -> Result<Vec<Option<f64>>, String> {
    if !col.data_type().is_numeric() {
        return Err(format!(
            "{} is not a measure a chart can plot",
            col.data_type()
        ));
    }
    let cast = cast_array(col, &DataType::Float64).map_err(|e| e.to_string())?;
    let cast = cast.as_primitive::<Float64Type>();
    Ok((0..cast.len())
        .map(|i| (!cast.is_null(i)).then(|| cast.value(i)))
        .collect())
}

/// Where each group sits on an axis that orders by value. A number is its own key; a clock
/// time keys on its integer representation, which is exact in an `f64` (a day is 8.64e13
/// nanoseconds, well inside the integer range).
fn ordering_key(col: &ArrayRef) -> Result<Vec<Option<f64>>, String> {
    if is_time_of_day(col.data_type()) {
        let ticks = integers(col)?;
        return Ok(ticks.into_iter().map(|t| t.map(|t| t as f64)).collect());
    }
    numbers(col)
}

/// One column as `i64`s — counts and clock ticks, where a float round-trip would be a lie
/// about precision.
fn integers(col: &ArrayRef) -> Result<Vec<Option<i64>>, String> {
    let cast = cast_array(col, &DataType::Int64).map_err(|e| e.to_string())?;
    let cast = cast.as_primitive::<Int64Type>();
    Ok((0..cast.len())
        .map(|i| (!cast.is_null(i)).then(|| cast.value(i)))
        .collect())
}

/// One millisecond-timestamp column as raw instants.
fn millis(col: &ArrayRef) -> Result<Vec<Option<i64>>, String> {
    let cast = cast_array(col, &DataType::Timestamp(TimeUnit::Millisecond, None))
        .map_err(|e| e.to_string())?;
    let cast = cast.as_primitive::<TimestampMillisecondType>();
    Ok((0..cast.len())
        .map(|i| (!cast.is_null(i)).then(|| cast.value(i)))
        .collect())
}

/// One column as display labels, rendered through the **engine's** display config so a
/// category reads the way the same value reads in the grid. `datafusion.format.date_format`
/// and its siblings have non-empty defaults in `ENGINE_KEYS`, so leaving this on arrow's
/// defaults was not a smaller difference than a user override — it was a difference on every
/// installation.
///
/// Two deliberate departures from the grid. A NULL is [`NULL_LABEL`], not the configured NULL
/// text, because spec §5 names that label and the axis is not a cell. And a label is clipped
/// to [`DISPLAY_CHARS`] like every other display text this crate produces: a categorical X
/// over a column of JSON documents would otherwise put up to `group_cap` whole documents into
/// the answer and into the cache entry holding it.
fn strings(col: &ArrayRef, fmt: &CellFormat) -> Result<Vec<String>, String> {
    let opts = fmt.opts();
    let render = ArrayFormatter::try_new(col.as_ref(), &opts).map_err(|e| e.to_string())?;
    Ok((0..col.len())
        .map(|i| {
            if col.is_null(i) {
                NULL_LABEL.to_string()
            } else {
                clip(&render.value(i).to_string(), DISPLAY_CHARS).into_owned()
            }
        })
        .collect())
}

/// Keeps only values with a position on a number line. `x > -inf AND x < inf` is exactly the
/// finite predicate: a NaN fails both comparisons and each infinity fails one, and a NULL
/// propagates to NULL, which `WHERE` drops. There is no `isfinite` in DataFusion 54.
fn finite(value: Expr) -> Expr {
    let value = cast(value, DataType::Float64);
    value
        .clone()
        .gt(lit(f64::NEG_INFINITY))
        .and(value.lt(lit(f64::INFINITY)))
}

/// Refuse a column a chart cannot put on a numeric axis, naming its type — the same answer
/// [`numbers`] gives for a measure, given before the plan is built so the two paths agree.
fn plottable(df: &DataFrame, name: &str) -> Result<(), String> {
    let dtype = field_type(df, name)?;
    if dtype.is_numeric() {
        return Ok(());
    }
    Err(format!("{dtype} is not a measure a chart can plot"))
}

/// The Arrow type of one result column, by exact name — result column names come out of the
/// user's own query and are never normalized.
fn field_type(df: &DataFrame, name: &str) -> Result<DataType, String> {
    df.schema()
        .fields()
        .iter()
        .find(|f| f.name() == name)
        .map(|f| f.data_type().clone())
        .ok_or_else(|| format!("no column '{name}' in this result"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;

    use datafusion::arrow::array::{Date32Array, Int64Array, StringArray};

    use super::*;

    /// Drive one read on a runtime of its own. DataFusion's operators spawn onto a Tokio
    /// executor, which [`super::super::Engine`] normally owns and a unit test calling
    /// [`run_chart`] directly has to supply — and a plain function (rather than
    /// `#[tokio::test]` on each case) is what lets the cases below build their fixtures
    /// inside ordinary closures.
    fn on_runtime<T>(fut: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(fut)
    }

    /// A fixture result, registered under the name a snapshot would have had.
    ///
    /// An in-memory table rather than a spooled file because everything under test here is
    /// the *aggregation*: the one property a real snapshot adds is scan parallelism, and
    /// nothing below depends on scan order (see the module header). `tests/engine_chart.rs`
    /// drives the same three shapes through a real spooled snapshot.
    fn fixture(columns: Vec<(&str, ArrayRef)>) -> SessionContext {
        let batch = RecordBatch::try_from_iter(columns).expect("fixture batch");
        let ctx = SessionContext::new();
        ctx.register_batch(snapshot_name(SnapshotId(1)).as_str(), batch)
            .expect("register fixture");
        ctx
    }

    fn read(columns: Vec<(&str, ArrayRef)>, q: ChartQuery) -> Result<ChartData, String> {
        let ctx = fixture(columns);
        let fmt = CellFormat::new(&BTreeMap::new());
        on_runtime(run_chart(&ctx, SnapshotId(1), &q, &fmt))
    }

    fn chart_of(columns: Vec<(&str, ArrayRef)>, q: ChartQuery) -> ChartData {
        read(columns, q).expect("chart")
    }

    fn m(y: Option<&str>, agg_fn: AggFn) -> Measure {
        Measure {
            y: y.map(String::from),
            agg_fn,
        }
    }

    fn agg(x: Option<&str>, series: Option<&str>, measures: Vec<Measure>) -> ChartQuery {
        ChartQuery::Aggregate {
            x: x.map(String::from),
            series: series.map(String::from),
            measures,
            bucket: None,
            group_cap: 1_000,
        }
    }

    /// The same, with an explicit bucket — a case that must not depend on what
    /// auto-resolution would have chosen.
    fn bucketed(x: &str, measures: Vec<Measure>, bucket: Bucket, cap: usize) -> ChartQuery {
        ChartQuery::Aggregate {
            x: Some(x.into()),
            series: None,
            measures,
            bucket: Some(bucket),
            group_cap: cap,
        }
    }

    fn months(x: &str, measures: Vec<Measure>, cap: usize) -> ChartQuery {
        bucketed(x, measures, Bucket::Time(Stride::Month), cap)
    }

    fn grouped(data: ChartData) -> (Vec<String>, Vec<ChartSeries>, Option<Bucket>) {
        match data {
            ChartData::Grouped {
                categories,
                series,
                bucket,
            } => (categories, series, bucket),
            other => panic!("expected a grouped chart, got {other:?}"),
        }
    }

    fn strs(values: Vec<Option<&str>>) -> ArrayRef {
        Arc::new(StringArray::from(values))
    }

    fn ints(values: Vec<Option<i64>>) -> ArrayRef {
        Arc::new(Int64Array::from(values))
    }

    fn floats(values: Vec<Option<f64>>) -> ArrayRef {
        Arc::new(Float64Array::from(values))
    }

    /// Milliseconds since the epoch for an RFC 3339 instant — so a fixture reads as dates.
    fn at(rfc3339: &str) -> i64 {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("fixture instant")
            .timestamp_millis()
    }

    fn stamps(values: Vec<Option<&str>>) -> ArrayRef {
        Arc::new(TimestampMillisecondArray::from(
            values
                .into_iter()
                .map(|v| v.map(at))
                .collect::<Vec<Option<i64>>>(),
        ))
    }

    /// **Every aggregate measures what it names.** They are DataFusion built-ins, so this is
    /// about the mapping: a chart that quietly summed where it said it averaged would look
    /// entirely plausible.
    #[test]
    fn each_aggregate_function_measures_what_it_names() {
        let columns = || {
            vec![
                ("g", strs(vec![Some("a"), Some("a"), Some("a"), Some("b")])),
                (
                    "v",
                    floats(vec![Some(1.0), Some(3.0), Some(8.0), Some(5.0)]),
                ),
            ]
        };
        // Group 'a' holds 1, 3, 8; group 'b' holds 5.
        let cases = [
            (AggFn::Sum, 12.0, 5.0),
            (AggFn::Avg, 4.0, 5.0),
            (AggFn::Min, 1.0, 5.0),
            (AggFn::Max, 8.0, 5.0),
            (AggFn::Count, 3.0, 1.0),
            (AggFn::Median, 3.0, 5.0),
            (AggFn::CountDistinct, 3.0, 1.0),
        ];
        for (agg_fn, a, b) in cases {
            let measure = m(Some("v"), agg_fn);
            let (categories, series, _) = grouped(chart_of(
                columns(),
                agg(Some("g"), None, vec![measure.clone()]),
            ));
            let cell = |label: &str| {
                let at = categories
                    .iter()
                    .position(|c| c == label)
                    .expect("category");
                series[0].values[at].expect("a measured cell")
            };
            assert_eq!(cell("a"), a, "{agg_fn:?} over group a");
            assert_eq!(cell("b"), b, "{agg_fn:?} over group b");
            assert_eq!(
                series[0].name,
                measure.label(),
                "a series with nothing else splitting it is named for its measure"
            );
        }
    }

    /// **No Y is a row count**, and it says so.
    #[test]
    fn a_measure_with_no_y_counts_rows() {
        let (categories, series, _) = grouped(chart_of(
            vec![("g", strs(vec![Some("a"), Some("a"), Some("b")]))],
            agg(Some("g"), None, vec![m(None, AggFn::Sum)]),
        ));
        assert_eq!(categories, vec!["a", "b"]);
        assert_eq!(series[0].name, "count(*)");
        assert_eq!(series[0].values, vec![Some(2.0), Some(1.0)]);
    }

    /// **The measure slot is plural, and its order is the caller's.** A preset that asks for
    /// four values per bucket reads them back by position, so nothing here may re-rank them.
    #[test]
    fn every_measure_is_its_own_series_in_the_order_it_was_asked_for() {
        let (categories, series, _) = grouped(chart_of(
            vec![
                ("g", strs(vec![Some("a"), Some("a"), Some("b")])),
                ("v", ints(vec![Some(1), Some(5), Some(3)])),
            ],
            agg(
                Some("g"),
                None,
                vec![m(Some("v"), AggFn::Min), m(Some("v"), AggFn::Max)],
            ),
        ));
        assert_eq!(categories, vec!["a", "b"]);
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].name, "min(v)");
        assert_eq!(series[0].values, vec![Some(1.0), Some(3.0)]);
        assert_eq!(
            series[1].name, "max(v)",
            "the heavier measure must not overtake the one asked for first"
        );
        assert_eq!(series[1].values, vec![Some(5.0), Some(3.0)]);
    }

    /// With both a series column and several measures, a legend entry has to name both.
    #[test]
    fn a_series_split_and_a_measure_list_both_name_the_series() {
        let (_, series, _) = grouped(chart_of(
            vec![
                ("g", strs(vec![Some("a"), Some("a")])),
                ("s", strs(vec![Some("eu"), Some("us")])),
                ("v", ints(vec![Some(1), Some(2)])),
            ],
            agg(
                Some("g"),
                Some("s"),
                vec![m(Some("v"), AggFn::Min), m(Some("v"), AggFn::Max)],
            ),
        ));
        let names: Vec<&str> = series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["us: min(v)", "eu: min(v)", "us: max(v)", "eu: max(v)"],
            "measure-major, series values ranked by what they measure"
        );
    }

    /// An empty measure list is refused rather than defaulted into a row count — nobody
    /// asked for that chart.
    #[test]
    fn a_chart_with_no_measure_at_all_is_refused() {
        let err = read(
            vec![("g", strs(vec![Some("a")]))],
            agg(Some("g"), None, Vec::new()),
        )
        .expect_err("nothing to draw");
        assert!(err.contains("measure"), "{err}");
    }

    /// **An empty temporal bucket is a gap, not a zero and not an absence.** `date_bin`
    /// emits no row for a month nothing fell in; the axis puts it back with no value, so a
    /// renderer draws a break rather than joining February straight to April.
    #[test]
    fn an_empty_temporal_bucket_comes_back_as_a_gap() {
        let (categories, series, bucket) = grouped(chart_of(
            vec![
                (
                    "t",
                    stamps(vec![
                        Some("2024-01-05T00:00:00Z"),
                        Some("2024-02-11T00:00:00Z"),
                        Some("2024-04-02T00:00:00Z"),
                    ]),
                ),
                ("v", ints(vec![Some(1), Some(2), Some(4)])),
            ],
            months("t", vec![m(Some("v"), AggFn::Sum)], 1_000),
        ));
        assert_eq!(bucket, Some(Bucket::Time(Stride::Month)));
        assert_eq!(categories.len(), 4, "January through April: {categories:?}");
        assert!(categories[0].starts_with("2024-01-01"), "{categories:?}");
        assert!(categories[3].starts_with("2024-04-01"), "{categories:?}");
        assert_eq!(
            series[0].values,
            vec![Some(1.0), Some(2.0), None, Some(4.0)],
            "March fell in no bucket and must read as a gap"
        );
    }

    /// A bucketed axis is ascending — never re-sorted by what it measures.
    #[test]
    fn a_bucketed_axis_stays_in_time_order() {
        let (categories, series, _) = grouped(chart_of(
            vec![
                (
                    "t",
                    stamps(vec![
                        Some("2024-03-01T00:00:00Z"),
                        Some("2024-01-01T00:00:00Z"),
                    ]),
                ),
                ("v", ints(vec![Some(9), Some(1)])),
            ],
            months("t", vec![m(Some("v"), AggFn::Sum)], 1_000),
        ));
        assert!(categories[0].starts_with("2024-01"), "{categories:?}");
        assert_eq!(
            series[0].values,
            vec![Some(1.0), None, Some(9.0)],
            "the larger measure must not jump to the front of a time axis"
        );
    }

    /// A date column buckets like a timestamp — `date_bin` takes neither `Date32` nor
    /// `Date64` on its own.
    #[test]
    fn a_date_column_buckets() {
        // Days since the epoch: 2024-01-10 and 2024-02-20.
        let (categories, _, bucket) = grouped(chart_of(
            vec![(
                "d",
                Arc::new(Date32Array::from(vec![19_732, 19_773])) as ArrayRef,
            )],
            months("d", vec![m(None, AggFn::Count)], 1_000),
        ));
        assert_eq!(bucket, Some(Bucket::Time(Stride::Month)));
        assert_eq!(categories.len(), 2, "{categories:?}");
        assert!(categories[0].starts_with("2024-01-01"), "{categories:?}");
    }

    /// **The stride is resolved from the span, then widened until the axis can be drawn.**
    /// The ladder alone hands back an hourly axis for a two-month span — 1 440 buckets
    /// against a cap of 1 000 — i.e. a default that refuses by construction.
    #[test]
    fn an_open_bucket_resolves_from_the_span_and_widens_to_fit() {
        let span = |from: &str, to: &str, cap: usize| {
            let (_, _, bucket) = grouped(chart_of(
                vec![("t", stamps(vec![Some(from), Some(to)]))],
                ChartQuery::Aggregate {
                    x: Some("t".into()),
                    series: None,
                    measures: vec![m(None, AggFn::Count)],
                    bucket: None,
                    group_cap: cap,
                },
            ));
            match bucket.expect("a temporal X reports the width it binned at") {
                Bucket::Time(stride) => stride,
                Bucket::Width(w) => panic!("a time column bucketed by a width of {w}"),
            }
        };
        assert_eq!(
            span("2024-01-01T00:00:00Z", "2024-01-01T06:00:00Z", 1_000),
            Stride::FiveMinutes,
            "hours apart"
        );
        assert_eq!(
            span("2024-01-01T00:00:00Z", "2024-01-20T00:00:00Z", 1_000),
            Stride::Hour,
            "weeks apart"
        );
        assert_eq!(
            span("2024-01-01T00:00:00Z", "2024-06-01T00:00:00Z", 1_000),
            Stride::Day,
            "months apart"
        );
        assert_eq!(
            span("2010-01-01T00:00:00Z", "2024-01-01T00:00:00Z", 1_000),
            Stride::Month,
            "years apart"
        );
        assert_eq!(
            span("2024-01-01T00:00:00Z", "2024-03-01T00:00:00Z", 10),
            Stride::Month,
            "a tight cap widens the bucket rather than refusing the chart"
        );
    }

    /// **A numeric X is first-class, grouped by its own values and ordered by them.**
    #[test]
    fn a_numeric_x_groups_by_value_and_orders_by_it() {
        let (categories, series, bucket) = grouped(chart_of(
            vec![("n", ints(vec![Some(10), Some(2), Some(2), None]))],
            agg(Some("n"), None, vec![m(None, AggFn::Count)]),
        ));
        assert_eq!(
            categories,
            vec!["2", "10", NULL_LABEL],
            "ascending, with the NULL group off the end of the number line"
        );
        assert_eq!(series[0].values, vec![Some(2.0), Some(1.0), Some(1.0)]);
        assert_eq!(bucket, None, "grouping by value is not bucketing");
    }

    /// **A binned numeric X fills its empty bins**, exactly as a time axis fills empty
    /// buckets — the same honesty rule, the same `None`.
    #[test]
    fn a_binned_numeric_x_fills_the_bins_nothing_fell_in() {
        let width = Width::new(2.0).expect("a width");
        let (categories, series, bucket) = grouped(chart_of(
            vec![("n", floats(vec![Some(1.0), Some(2.0), Some(9.0)]))],
            bucketed(
                "n",
                vec![m(None, AggFn::Count)],
                Bucket::Width(width),
                1_000,
            ),
        ));
        assert_eq!(bucket, Some(Bucket::Width(width)));
        assert_eq!(categories.len(), 5, "0, 2, 4, 6, 8: {categories:?}");
        assert_eq!(categories[0], "0.0", "{categories:?}");
        assert_eq!(categories[4], "8.0", "{categories:?}");
        assert_eq!(
            series[0].values,
            vec![Some(1.0), Some(1.0), None, None, Some(1.0)],
            "the bins between 4 and 8 held nothing and must read as gaps"
        );
    }

    /// **A bucket of the wrong kind is refused, not ignored** — a stale one left by an
    /// encoding change would otherwise chart something the strip isn't showing.
    #[test]
    fn a_bucket_that_does_not_fit_the_column_is_refused() {
        let width = Bucket::Width(Width::new(2.0).unwrap());
        let err = read(
            vec![("t", stamps(vec![Some("2024-01-01T00:00:00Z")]))],
            bucketed("t", vec![m(None, AggFn::Count)], width, 1_000),
        )
        .expect_err("a time column does not take a width");
        assert!(err.contains("stride"), "{err}");

        let err = read(
            vec![("n", ints(vec![Some(1)]))],
            bucketed(
                "n",
                vec![m(None, AggFn::Count)],
                Bucket::Time(Stride::Month),
                1_000,
            ),
        )
        .expect_err("a number does not take a stride");
        assert!(err.contains("width"), "{err}");

        let err = read(
            vec![("g", strs(vec![Some("a")]))],
            bucketed(
                "g",
                vec![m(None, AggFn::Count)],
                Bucket::Time(Stride::Month),
                1_000,
            ),
        )
        .expect_err("a category does not bucket at all");
        assert!(err.contains("bucket"), "{err}");
    }

    /// **A NULL X is its own group, and the label is not the key.** A column that genuinely
    /// holds the string `(null)` therefore keeps its own category: merging the two would
    /// lose one group's rows and say nothing about it.
    #[test]
    fn a_null_group_is_labelled_but_never_merged_with_that_label() {
        let (categories, series, _) = grouped(chart_of(
            vec![
                ("g", strs(vec![Some("a"), None, Some("(null)")])),
                ("v", ints(vec![Some(1), Some(2), Some(3)])),
            ],
            agg(Some("g"), None, vec![m(Some("v"), AggFn::Sum)]),
        ));
        assert_eq!(categories.len(), 3, "{categories:?}");
        assert_eq!(
            categories.iter().filter(|c| *c == NULL_LABEL).count(),
            2,
            "the NULL group and the literal string both read as (null): {categories:?}"
        );
        let total: f64 = series[0].values.iter().flatten().sum();
        assert_eq!(total, 6.0, "no group's rows went missing");
    }

    /// A NULL bucket on a time axis lands after the sequence, where it cannot imply a
    /// position in time.
    #[test]
    fn a_null_bucket_sits_off_the_end_of_a_time_axis() {
        let (categories, series, _) = grouped(chart_of(
            vec![(
                "t",
                stamps(vec![
                    Some("2024-01-05T00:00:00Z"),
                    Some("2024-02-05T00:00:00Z"),
                    None,
                ]),
            )],
            months("t", vec![m(None, AggFn::Count)], 1_000),
        ));
        assert_eq!(categories.len(), 3, "{categories:?}");
        assert_eq!(categories[2], NULL_LABEL, "{categories:?}");
        assert_eq!(series[0].values, vec![Some(1.0), Some(1.0), Some(1.0)]);
    }

    /// **Over the cap, nothing is drawn.** Not a truncated chart, not the top N — the
    /// answer carries no data at all (spec §1.4).
    #[test]
    fn a_read_past_its_cap_refuses_instead_of_truncating() {
        let capped = |cap: usize| {
            chart_of(
                vec![("g", strs(vec![Some("a"), Some("b"), Some("c")]))],
                ChartQuery::Aggregate {
                    x: Some("g".into()),
                    series: None,
                    measures: vec![m(None, AggFn::Count)],
                    bucket: None,
                    group_cap: cap,
                },
            )
        };
        assert_eq!(
            capped(2),
            ChartData::OverCap {
                unit: CapUnit::Groups,
                cap: 2,
                bucket: None
            }
        );
        let (categories, _, _) = grouped(capped(3));
        assert_eq!(categories.len(), 3, "exactly at the cap still draws");
    }

    /// A bucketed axis is capped on the buckets it would *span*, not on the rows that came
    /// back: two rows a decade apart are two aggregate rows and eighty-seven thousand
    /// hourly buckets. The same holds for a numeric bin width fine enough to shatter a range.
    #[test]
    fn a_sparse_span_is_capped_on_the_buckets_it_would_fill() {
        let sparse = chart_of(
            vec![(
                "t",
                stamps(vec![
                    Some("2014-01-01T00:00:00Z"),
                    Some("2024-01-01T00:00:00Z"),
                ]),
            )],
            // The user's own stride, so it is honoured and then refused — never silently
            // widened out from under them.
            bucketed(
                "t",
                vec![m(None, AggFn::Count)],
                Bucket::Time(Stride::Hour),
                1_000,
            ),
        );
        assert_eq!(
            sparse,
            ChartData::OverCap {
                unit: CapUnit::Groups,
                cap: 1_000,
                bucket: Some(Bucket::Time(Stride::Hour))
            }
        );

        let fine = chart_of(
            vec![("n", floats(vec![Some(0.0), Some(1_000_000.0)]))],
            bucketed(
                "n",
                vec![m(None, AggFn::Count)],
                Bucket::Width(Width::new(0.5).unwrap()),
                1_000,
            ),
        );
        assert_eq!(
            fine,
            ChartData::OverCap {
                unit: CapUnit::Groups,
                cap: 1_000,
                // The refusal names the width in effect, which is the one thing the guardrail
                // can suggest changing.
                bucket: Some(Bucket::Width(Width::new(0.5).unwrap()))
            }
        );
    }

    /// **A series splits each category, and a pair the data never held is `None`.**
    #[test]
    fn a_series_splits_each_category_and_a_missing_cell_is_absent() {
        let (categories, series, _) = grouped(chart_of(
            vec![
                ("g", strs(vec![Some("a"), Some("a"), Some("b")])),
                ("s", strs(vec![Some("x"), Some("y"), Some("x")])),
                ("v", ints(vec![Some(1), Some(2), Some(3)])),
            ],
            agg(Some("g"), Some("s"), vec![m(Some("v"), AggFn::Sum)]),
        ));
        assert_eq!(categories, vec!["a", "b"]);
        let by_name = |name: &str| {
            series
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("series {name}: {series:?}"))
                .values
                .clone()
        };
        assert_eq!(by_name("x"), vec![Some(1.0), Some(3.0)]);
        assert_eq!(
            by_name("y"),
            vec![Some(2.0), None],
            "(b, y) is a pair the data never contained"
        );
        assert_eq!(
            series[0].name, "x",
            "the heavier series leads the legend: {series:?}"
        );
    }

    /// Categorical categories rank by what they measure, ties by label — total, so one
    /// result always draws in one order however the scan was parallelised.
    #[test]
    fn categories_rank_by_measure_then_by_label() {
        let (categories, _, _) = grouped(chart_of(
            vec![
                ("g", strs(vec![Some("a"), Some("c"), Some("b")])),
                ("v", ints(vec![Some(1), Some(5), Some(5)])),
            ],
            agg(Some("g"), None, vec![m(Some("v"), AggFn::Sum)]),
        ));
        assert_eq!(categories, vec!["b", "c", "a"]);
    }

    /// No X at all is one category covering the whole result; what splits it names itself
    /// in the legend.
    #[test]
    fn a_chart_with_no_x_is_a_single_category() {
        let (categories, series, bucket) = grouped(chart_of(
            vec![("v", ints(vec![Some(1), Some(2)]))],
            agg(None, None, vec![m(Some("v"), AggFn::Sum)]),
        ));
        assert_eq!(categories, vec![ALL_LABEL]);
        assert_eq!(bucket, None);
        assert_eq!(series[0].name, "sum(v)");
        assert_eq!(series[0].values, vec![Some(3.0)]);
    }

    /// **Bins are uniform, contiguous, and account for every row** — the top of the range
    /// included, which is why the last bin is closed.
    #[test]
    fn a_histogram_bins_uniformly_and_loses_no_row() {
        let values: Vec<Option<i64>> = (1..=10).map(Some).collect();
        let data = chart_of(
            vec![("v", ints(values))],
            ChartQuery::Histogram {
                col: "v".into(),
                bins: Some(5),
            },
        );
        let ChartData::Bins(bins) = data else {
            panic!("expected bins, got {data:?}")
        };
        assert_eq!(bins.len(), 5);
        assert_eq!(bins.iter().map(|b| b.count).sum::<u64>(), 10);
        assert_eq!(bins[0].lo, 1.0);
        assert_eq!(bins[4].hi, 10.0, "the last edge is the measured maximum");
        for pair in bins.windows(2) {
            assert_eq!(pair[0].hi, pair[1].lo, "bins are contiguous: {bins:?}");
        }
    }

    /// A column with one distinct value has a width of zero — one bin, not a division by it.
    #[test]
    fn a_single_valued_histogram_is_one_bin() {
        let data = chart_of(
            vec![("v", ints(vec![Some(7), Some(7), Some(7)]))],
            ChartQuery::Histogram {
                col: "v".into(),
                bins: None,
            },
        );
        assert_eq!(
            data,
            ChartData::Bins(vec![ChartBin {
                lo: 7.0,
                hi: 7.0,
                count: 3
            }])
        );
    }

    /// Nothing to bin is a histogram of nothing, not a histogram of zeroes.
    #[test]
    fn an_all_null_histogram_has_no_bins() {
        let data = chart_of(
            vec![("v", ints(vec![None, None]))],
            ChartQuery::Histogram {
                col: "v".into(),
                bins: None,
            },
        );
        assert_eq!(data, ChartData::Bins(Vec::new()));
    }

    /// The bin count comes from the row count when the request leaves it open.
    #[test]
    fn an_open_bin_count_is_bounded_both_ways() {
        assert_eq!(auto_bins(1), 6, "a handful of rows still reads as a shape");
        assert_eq!(auto_bins(100), 10);
        assert_eq!(auto_bins(10_000_000), 24, "and never more than 24 bars");
    }

    /// **Scatter returns raw points**, and a row missing either coordinate has no position
    /// on the plane to return.
    #[test]
    fn scatter_returns_the_points_that_can_be_drawn() {
        let data = chart_of(
            vec![
                ("x", ints(vec![Some(1), Some(2), None, Some(4)])),
                ("y", floats(vec![Some(10.0), None, Some(30.0), Some(40.0)])),
            ],
            ChartQuery::Raw {
                x: "x".into(),
                y: "y".into(),
                cap: 10,
            },
        );
        assert_eq!(
            data,
            ChartData::Points(vec![
                ChartPoint { x: 1.0, y: 10.0 },
                ChartPoint { x: 4.0, y: 40.0 },
            ])
        );
    }

    /// The point cap counts points that can be drawn, and refuses rather than thinning.
    #[test]
    fn too_many_points_refuse() {
        let data = chart_of(
            vec![
                ("x", ints(vec![Some(1), Some(2), Some(3)])),
                ("y", ints(vec![Some(1), Some(2), Some(3)])),
            ],
            ChartQuery::Raw {
                x: "x".into(),
                y: "y".into(),
                cap: 2,
            },
        );
        assert_eq!(
            data,
            ChartData::OverCap {
                unit: CapUnit::Points,
                cap: 2,
                bucket: None
            }
        );
    }

    /// A measure that isn't a number is refused, not lossily cast — arrow's default cast
    /// would turn `min` over a text column into a chart of empty cells.
    #[test]
    fn a_measure_that_is_not_a_number_is_refused() {
        let err = read(
            vec![
                ("g", strs(vec![Some("a")])),
                ("t", strs(vec![Some("text")])),
            ],
            agg(Some("g"), None, vec![m(Some("t"), AggFn::Min)]),
        )
        .expect_err("text is not a measure");
        assert!(err.contains("measure"), "{err}");
    }

    /// **A NaN measure must not break the ordering.** `partial_cmp(..).unwrap_or(Equal)` makes
    /// NaN compare equal to every real weight while those weights still order among
    /// themselves, which is intransitive: `sort_by` returns an arbitrary order below its
    /// insertion-sort threshold and **panics** above it, aborting the whole chart read.
    #[test]
    fn a_nan_measure_orders_last_instead_of_breaking_the_sort() {
        // Comfortably past the 20-element threshold where `sort_by` starts checking.
        let n = 30;
        let groups: Vec<Option<String>> = (0..n).map(|i| Some(format!("g{i:02}"))).collect();
        let labels: Vec<Option<&str>> = groups.iter().map(|g| g.as_deref()).collect();
        let mut values: Vec<Option<f64>> = (0..n).map(|i| Some(i as f64)).collect();
        values[3] = Some(f64::NAN);
        let (categories, series, _) = grouped(chart_of(
            vec![("g", strs(labels)), ("v", floats(values))],
            agg(Some("g"), None, vec![m(Some("v"), AggFn::Sum)]),
        ));
        assert_eq!(categories.len(), n);
        assert_eq!(
            categories[0], "g29",
            "the heaviest group still leads: {categories:?}"
        );
        assert_eq!(
            categories[n - 1],
            "g03",
            "and the NaN group ranks below every quantity: {categories:?}"
        );
        assert!(series[0].values[n - 1].is_some_and(f64::is_nan));
    }

    /// The same hazard on the value axis, which had no tiebreak at all.
    #[test]
    fn a_nan_numeric_x_sorts_last_instead_of_breaking_the_sort() {
        let n = 30;
        let mut keys: Vec<Option<f64>> = (0..n).map(|i| Some(i as f64)).collect();
        keys[3] = Some(f64::NAN);
        let (categories, _, _) = grouped(chart_of(
            vec![("n", floats(keys))],
            agg(Some("n"), None, vec![m(None, AggFn::Count)]),
        ));
        assert_eq!(categories.len(), n);
        assert_eq!(categories[0], "0.0", "ascending: {categories:?}");
        assert_eq!(
            categories[n - 1],
            "NaN",
            "a NaN has no position on a number line: {categories:?}"
        );
    }

    /// **An auto-resolved bucket widens when the *split* overruns the cap**, not only when the
    /// buckets do. The stride is chosen before the series cardinality is known and the cap
    /// counts categories x series, so a rung that fits on its own can still be refused.
    #[test]
    fn an_auto_bucket_widens_when_a_series_split_overruns_the_cap() {
        // Three days of hourly readings from five hosts: 72 buckets, inside a cap of 200 on
        // their own, and 360 aggregate rows once the series splits them.
        let mut stamps_at = Vec::new();
        let mut hosts = Vec::new();
        for hour in 0..72 {
            for host in 0..5 {
                stamps_at.push(Some(at("2024-01-01T00:00:00Z") + hour * 3_600_000));
                hosts.push(Some(["a", "b", "c", "d", "e"][host]));
            }
        }
        let (_, _, bucket) = grouped(chart_of(
            vec![
                (
                    "t",
                    Arc::new(TimestampMillisecondArray::from(stamps_at)) as ArrayRef,
                ),
                ("h", strs(hosts)),
            ],
            ChartQuery::Aggregate {
                x: Some("t".into()),
                series: Some("h".into()),
                measures: vec![m(None, AggFn::Count)],
                bucket: None,
                group_cap: 200,
            },
        ));
        assert_eq!(
            bucket,
            Some(Bucket::Time(Stride::Day)),
            "the hourly rung fits the buckets and not the split, so it widens"
        );
    }

    /// A date outside `date_bin`'s nanosecond window is refused by name, not left to overflow
    /// inside DataFusion — `9999-12-31` is the ordinary "still current" sentinel.
    #[test]
    fn a_date_beyond_the_bucketable_range_is_refused_by_name() {
        // Days since the epoch for 9999-12-31.
        let err = read(
            vec![(
                "d",
                Arc::new(Date32Array::from(vec![19_732, 2_932_896])) as ArrayRef,
            )],
            months("d", vec![m(None, AggFn::Count)], 1_000),
        )
        .expect_err("that date cannot be bucketed");
        assert!(err.contains("'d'"), "{err}");
        assert!(err.contains("range"), "{err}");
    }

    /// A NaN in the column must not take the histogram down with it: arrow's `max` reports
    /// NaN as the largest value, which made the bin width NaN and failed the read.
    #[test]
    fn a_histogram_bins_around_a_nan_instead_of_failing() {
        let data = chart_of(
            vec![(
                "v",
                floats(vec![
                    Some(1.0),
                    Some(2.0),
                    Some(f64::NAN),
                    Some(f64::INFINITY),
                    Some(3.0),
                ]),
            )],
            ChartQuery::Histogram {
                col: "v".into(),
                bins: Some(2),
            },
        );
        let ChartData::Bins(bins) = data else {
            panic!("expected bins, got {data:?}")
        };
        assert_eq!(bins.len(), 2);
        assert_eq!(bins[0].lo, 1.0);
        assert_eq!(bins[1].hi, 3.0, "the range is of the finite values");
        assert_eq!(
            bins.iter().map(|b| b.count).sum::<u64>(),
            3,
            "the NaN and the infinity are not numbers with a position"
        );
    }

    /// A time-of-day X orders by the clock, not by what it measures.
    #[test]
    fn a_time_of_day_axis_runs_in_clock_order() {
        use datafusion::arrow::array::Time64NanosecondArray;
        let hour = 3_600_000_000_000i64;
        let (categories, series, _) = grouped(chart_of(
            vec![
                (
                    "t",
                    Arc::new(Time64NanosecondArray::from(vec![
                        14 * hour,
                        22 * hour,
                        9 * hour,
                    ])) as ArrayRef,
                ),
                ("v", ints(vec![Some(9), Some(5), Some(1)])),
            ],
            agg(Some("t"), None, vec![m(Some("v"), AggFn::Sum)]),
        ));
        assert_eq!(
            categories,
            vec!["09:00:00", "14:00:00", "22:00:00"],
            "the afternoon selling more must not move it to the front"
        );
        assert_eq!(series[0].values, vec![Some(1.0), Some(9.0), Some(5.0)]);
    }

    /// A snapshot column literally named `count(*)` is ordinary — `SELECT region, count(*)
    /// FROM t GROUP BY 1` produces one — and charting it must not collide with the row-count
    /// measure's own output field.
    #[test]
    fn a_column_named_count_star_can_be_charted() {
        let (categories, series, _) = grouped(chart_of(
            vec![
                ("region", strs(vec![Some("eu"), Some("us")])),
                ("count(*)", ints(vec![Some(3), Some(7)])),
            ],
            agg(Some("count(*)"), None, vec![m(None, AggFn::Count)]),
        ));
        assert_eq!(categories, vec!["3", "7"]);
        assert_eq!(series[0].values, vec![Some(1.0), Some(1.0)]);
    }

    /// Two Y-less measures are two row counts, and they must not collide as one field name.
    #[test]
    fn two_row_count_measures_do_not_collide() {
        let (_, series, _) = grouped(chart_of(
            vec![("g", strs(vec![Some("a"), Some("a")]))],
            agg(
                Some("g"),
                None,
                vec![m(None, AggFn::Count), m(None, AggFn::Sum)],
            ),
        ));
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].values, vec![Some(2.0)]);
        assert_eq!(series[1].values, vec![Some(2.0)]);
    }

    /// A fractional bin width labels its axis in the units the user asked for, not in the
    /// float noise repeated multiplication produces.
    #[test]
    fn a_fractional_bin_width_labels_in_its_own_units() {
        let (categories, _, _) = grouped(chart_of(
            vec![(
                "n",
                floats(vec![Some(0.05), Some(0.25), Some(0.35), Some(0.75)]),
            )],
            bucketed(
                "n",
                vec![m(None, AggFn::Count)],
                Bucket::Width(Width::new(0.1).unwrap()),
                1_000,
            ),
        ));
        assert_eq!(
            categories,
            vec!["0.0", "0.1", "0.2", "0.3", "0.4", "0.5", "0.6", "0.7"],
            "0.1 x 3 is 0.30000000000000004 in binary floating point, and that is not a tick"
        );
    }

    /// A scatter over a text column is refused by naming the type, the same answer the
    /// aggregate path gives — not an Arrow message about a string it could not parse.
    #[test]
    fn a_scatter_over_a_text_column_is_refused_the_same_way() {
        let err = read(
            vec![
                ("name", strs(vec![Some("a")])),
                ("amount", ints(vec![Some(1)])),
            ],
            ChartQuery::Raw {
                x: "name".into(),
                y: "amount".into(),
                cap: 10,
            },
        )
        .expect_err("text has no position on a plane");
        assert!(err.contains("measure"), "{err}");
    }

    /// **A NULL bin and a NaN bin are two groups, not one.** DataFusion emits a distinct group
    /// row for each, and folding them onto one category made the pivot's plain assignment lose
    /// whichever arrived first — nondeterministically, since it depends on emission order.
    #[test]
    fn a_binned_numeric_x_keeps_null_and_nan_apart() {
        let (categories, series, _) = grouped(chart_of(
            vec![(
                "n",
                floats(vec![
                    Some(1.0),
                    None,
                    None,
                    Some(f64::NAN),
                    Some(f64::INFINITY),
                ]),
            )],
            bucketed(
                "n",
                vec![m(None, AggFn::Count)],
                Bucket::Width(Width::new(2.0).unwrap()),
                1_000,
            ),
        ));
        // One real bin, then one tick each for NULL, NaN and inf.
        assert_eq!(categories.len(), 4, "{categories:?}");
        assert_eq!(categories[0], "0.0", "{categories:?}");
        assert_eq!(
            categories.iter().filter(|c| *c == NULL_LABEL).count(),
            1,
            "{categories:?}"
        );
        let total: f64 = series[0].values.iter().flatten().sum();
        assert_eq!(total, 5.0, "every row is still counted once: {series:?}");
    }

    /// A finite bin index past 2^53 is a real bin, not a null one. Excluding it charted real
    /// rows as `(null)` rather than refusing.
    #[test]
    fn a_huge_bin_index_is_still_a_bin() {
        let (categories, series, _) = grouped(chart_of(
            vec![("n", floats(vec![Some(1e18), Some(1e18)]))],
            bucketed(
                "n",
                vec![m(None, AggFn::Count)],
                Bucket::Width(Width::new(100.0).unwrap()),
                1_000,
            ),
        ));
        assert_eq!(categories.len(), 1, "{categories:?}");
        assert_ne!(categories[0], NULL_LABEL, "a real bin is not the null one");
        assert_eq!(series[0].values, vec![Some(2.0)]);
    }

    /// A scatter point needs a position on the plane, and a NaN is not one — the null bitmap
    /// is unset for a NaN, so filtering NULLs alone let it through.
    #[test]
    fn scatter_drops_a_non_finite_coordinate() {
        let data = chart_of(
            vec![
                (
                    "x",
                    floats(vec![Some(1.0), Some(f64::NAN), Some(3.0), Some(4.0)]),
                ),
                (
                    "y",
                    floats(vec![
                        Some(10.0),
                        Some(20.0),
                        Some(f64::INFINITY),
                        Some(40.0),
                    ]),
                ),
            ],
            ChartQuery::Raw {
                x: "x".into(),
                y: "y".into(),
                cap: 10,
            },
        );
        assert_eq!(
            data,
            ChartData::Points(vec![
                ChartPoint { x: 1.0, y: 10.0 },
                ChartPoint { x: 4.0, y: 40.0 },
            ])
        );
    }

    /// An empty result has no groups, so it draws no categories — not one `(null)` tick
    /// asserting a group nothing created.
    #[test]
    fn an_empty_result_draws_an_empty_axis() {
        let (categories, series, _) = grouped(chart_of(
            vec![("t", stamps(Vec::new())), ("v", ints(Vec::new()))],
            agg(Some("t"), None, vec![m(Some("v"), AggFn::Sum)]),
        ));
        assert!(categories.is_empty(), "{categories:?}");
        assert!(series.iter().all(|s| s.values.is_empty()), "{series:?}");
    }

    /// A bucket boundary is UTC, because `date_bin` bins against the UTC epoch and only
    /// re-attaches the column's timezone. Labelling it in that zone read a January bucket as
    /// 7pm on 31 December.
    #[test]
    fn a_zoned_timestamp_labels_its_buckets_where_they_actually_fall() {
        let zoned: ArrayRef = Arc::new(
            TimestampMillisecondArray::from(vec![
                at("2024-01-05T00:00:00Z"),
                at("2024-02-05T00:00:00Z"),
            ])
            .with_timezone("America/New_York"),
        );
        let (categories, _, _) = grouped(chart_of(
            vec![("t", zoned)],
            months("t", vec![m(None, AggFn::Count)], 1_000),
        ));
        assert!(
            categories[0].starts_with("2024-01-01"),
            "a month bucket starts on the first: {categories:?}"
        );
    }

    /// A date column's axis reads as dates, through the same format key the grid uses for it.
    #[test]
    fn a_date_axis_reads_as_dates() {
        let (categories, _, _) = grouped(chart_of(
            vec![(
                "d",
                Arc::new(Date32Array::from(vec![19_732, 19_773])) as ArrayRef,
            )],
            months("d", vec![m(None, AggFn::Count)], 1_000),
        ));
        assert_eq!(
            categories,
            vec!["2024-01-01", "2024-02-01"],
            "{categories:?}"
        );
    }

    /// A time-of-day column buckets neither way, and the refusal must not send the user at a
    /// setting the other arm refuses.
    #[test]
    fn a_time_of_day_column_is_refused_a_bucket_in_its_own_terms() {
        use datafusion::arrow::array::Time64NanosecondArray;
        let column = || {
            vec![(
                "t",
                Arc::new(Time64NanosecondArray::from(vec![3_600_000_000_000i64])) as ArrayRef,
            )]
        };
        for bucket in [
            Bucket::Time(Stride::Month),
            Bucket::Width(Width::new(2.0).unwrap()),
        ] {
            let err = read(
                column(),
                bucketed("t", vec![m(None, AggFn::Count)], bucket, 1_000),
            )
            .expect_err("a clock has no bucket");
            assert!(err.contains("time of day"), "{err}");
            assert!(!err.contains("is a number"), "{err}");
        }
    }

    /// One column cannot be both the category and the series; DataFusion's own answer is an
    /// internal schema message.
    #[test]
    fn the_same_column_on_x_and_series_is_refused_in_our_words() {
        let err = read(
            vec![("region", strs(vec![Some("eu")]))],
            agg(Some("region"), Some("region"), vec![m(None, AggFn::Count)]),
        )
        .expect_err("one column cannot split itself");
        assert!(err.contains("'region'"), "{err}");
        assert!(!err.contains("Schema error"), "{err}");
    }

    /// A column the result doesn't have is named, not planned around.
    #[test]
    fn a_missing_column_says_which_one() {
        let err = read(
            vec![("g", strs(vec![Some("a")]))],
            agg(Some("nope"), None, vec![m(None, AggFn::Count)]),
        )
        .expect_err("a missing column cannot be charted");
        assert!(err.contains("nope"), "{err}");
    }
}
