//! Chart reads over a snapshot (Rz2, `docs/CHART_SPEC.md` §5) — the renderer-first data
//! path behind the results Chart surface, answered as a small [`ChartData`] the renderer
//! draws without touching a row of the result again.
//!
//! **The chart computes nothing SQL can say** (spec §1.2). [`ChartQuery::Rows`] is a
//! *projection*: the referenced columns, `ORDER BY` the snapshot ordinal
//! (`docs/SNAPSHOT_SPEC.md` §9), `LIMIT cap + 1` — then a long→wide pivot in Rust when a
//! series column splits the rows. No aggregation, no bucketing, no imposed order: the rows
//! draw in the order the user's query produced them, and everything analytical is the
//! user's own SQL, written by the user. The engine-side aggregation pipeline that
//! used to live here was built, adversarially reviewed twice, and withdrawn —
//! `docs/reference/INVARIANTS.md` (the chart entry) records the evidence; do not
//! resurrect it.
//!
//! Two exceptions, both deliberate and bounded:
//!
//! - **The histogram computes** (spec §1.2's one exception): binning a raw column needs a
//!   min/max pass, and DataFusion 54 has no `width_bucket`, so hand-writing it is genuinely
//!   tedious. It is an aggregate over the whole column with a bins-count answer — the cap
//!   is the bin count, not a row count.
//! - **Refusals are answers, not errors.** Over a cap ([`ChartData::OverCap`]) or two rows
//!   in one pivot cell ([`ChartData::Duplicates`]) the read succeeds and carries nothing to
//!   draw — the surface renders the refusal, which names the user's own `GROUP BY` as the
//!   fix and puts no control behind it (spec §7, §8). An *encoding*
//!   mistake (a text column as Y, a column that doesn't exist) is an `Err`, in this
//!   module's words rather than DataFusion's.

use std::collections::{HashMap, HashSet};

use datafusion::arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use datafusion::arrow::compute::{cast as cast_array, concat_batches};
use datafusion::arrow::datatypes::{DataType, Float64Type, Int64Type, TimeUnit};
use datafusion::arrow::util::display::ArrayFormatter;
use datafusion::common::{Column, ScalarValue};
use datafusion::functions_aggregate::expr_fn::{count, max, min};
use datafusion::prelude::{cast, col, floor, ident, lit, DataFrame, Expr, SessionContext};

use strata_model::{
    Axis, CapUnit, ChartBin, ChartData, ChartPoint, ChartQuery, ChartSeries, SnapshotId,
};

use super::query::{snapshot_name, CellFormat};
use crate::util::{clip, DISPLAY_CHARS};

/// What a NULL reads as on an axis or in a legend (spec §5). Only ever a *label*: categories
/// and series are keyed by the value itself, so this never merges with a real `(null)`
/// string.
const NULL_LABEL: &str = "(null)";

/// Read `snapshot` as a chart (`docs/CHART_SPEC.md` §5).
///
/// Snapshot-scoped and side-effect free, exactly like [`super::query::fetch_page`], which is
/// what lets the UI cache the answer by `(SnapshotId, ChartQuery)` with no confirm in front
/// of it: a projected, capped read of a local snapshot is `fetch_page`-tier work.
pub async fn run_chart(
    ctx: &SessionContext,
    snapshot: SnapshotId,
    q: &ChartQuery,
    fmt: &CellFormat,
    ord: Option<&str>,
) -> Result<ChartData, String> {
    let df = ctx
        .table(snapshot_name(snapshot).as_str())
        .await
        .map_err(|e| e.to_string())?;
    match q {
        ChartQuery::Rows { x, ys, series, cap } => {
            rows(df, x.as_deref(), ys, series.as_deref(), *cap, ord, fmt).await
        }
        ChartQuery::Raw { x, y, cap } => raw(df, x, y, *cap).await,
        ChartQuery::Histogram { col, bins } => histogram(df, col, *bins).await,
    }
}

// ---- the renderer-first read (bar / line / area / pie) ----

async fn rows(
    df: DataFrame,
    x: Option<&str>,
    ys: &[String],
    series: Option<&str>,
    cap: usize,
    ord: Option<&str>,
    fmt: &CellFormat,
) -> Result<ChartData, String> {
    if ys.is_empty() {
        return Err("a chart needs at least one Y column".into());
    }
    for y in ys {
        plottable(&df, y)?;
    }
    if let Some(x) = x {
        field_type(&df, x)?;
    }
    if let Some(series) = series {
        field_type(&df, series)?;
        // The pivot needs a row identity to pivot *around*; without an X every row is its
        // own category and the split would produce one lonely cell per series value.
        if x.is_none() {
            return Err("a series split needs an X column".into());
        }
        if x == Some(series) {
            // DataFusion answers this itself with "Schema contains duplicate qualified
            // field name" — an internal message for an encoding mistake this module names
            // in its own words everywhere else.
            return Err(format!(
                "'{series}' cannot be both the category and the series"
            ));
        }
    }

    // One projection, each referenced column once — a duplicate name in a `select` is a
    // schema error, and `x` may legitimately also be a Y. The projection makes every name
    // unique and exact, so columns are read back by name below.
    let mut names: Vec<&str> = Vec::new();
    for name in x
        .iter()
        .copied()
        .chain(series.iter().copied())
        .chain(ys.iter().map(String::as_str))
    {
        if !names.contains(&name) {
            names.push(name);
        }
    }

    // Result order is the ordinal's (`SNAPSHOT_SPEC.md` §9) — sort + fetch plans as a TopK,
    // so memory is O(cap) however large the snapshot. The ordinal is `None` only for a
    // snapshot that is gone, whose read fails on its own terms below.
    let mut plan = df;
    if let Some(ord) = ord {
        plan = plan
            .sort(vec![col(Column::from_name(ord)).sort(true, false)])
            .map_err(|e| e.to_string())?;
    }
    let plan = plan
        .limit(0, Some(cap.saturating_add(1)))
        .map_err(|e| e.to_string())?
        .select(names.iter().map(|n| ident(*n)).collect::<Vec<Expr>>())
        .map_err(|e| e.to_string())?;
    let batch = one_batch(plan).await?;
    if batch.num_rows() > cap {
        return Ok(ChartData::OverCap {
            unit: CapUnit::Rows,
            cap,
        });
    }

    let x_col = x.map(|n| projected(&batch, n)).transpose()?;
    let series_col = series.map(|n| projected(&batch, n)).transpose()?;
    let mut y_cols = Vec::with_capacity(ys.len());
    for y in ys {
        y_cols.push(numbers(&projected(&batch, y)?)?);
    }

    match (&series_col, &x_col) {
        (Some(split), Some(keys)) => pivot(
            keys,
            split,
            &y_cols,
            ys,
            x.unwrap_or_default(),
            series.unwrap_or_default(),
            fmt,
        ),
        _ => {
            // No pivot: each row is its own mark, in result order. Duplicate X labels draw
            // as duplicate marks — the chart shows what the result holds (spec §4).
            let axis = match &x_col {
                Some(col) => Axis {
                    labels: strings(col, fmt)?,
                    positions: positions(col)?,
                },
                None => row_index_axis(batch.num_rows()),
            };
            let series = ys
                .iter()
                .zip(y_cols)
                .map(|(name, values)| ChartSeries {
                    name: name.clone(),
                    values,
                })
                .collect();
            Ok(ChartData::Table { axis, series })
        }
    }
}

/// The long→wide pivot: rows `(x, series, y…)` become one series per (series value, Y
/// column), each as long as the category axis.
///
/// Cell identity is the (X value, series value) **pair of values** — `ScalarValue`s, never
/// their renderings, so a NULL and a literal `"(null)"` stay two categories. The pivot is
/// the only operation here that can conflate rows, so it is the only thing that refuses on
/// duplicates: aggregating them is SQL's job (spec §1.2).
fn pivot(
    keys: &ArrayRef,
    split: &ArrayRef,
    y_cols: &[Vec<Option<f64>>],
    ys: &[String],
    x_name: &str,
    series_name: &str,
    fmt: &CellFormat,
) -> Result<ChartData, String> {
    let key_labels = strings(keys, fmt)?;
    let key_positions = positions(keys)?;
    let split_labels = strings(split, fmt)?;

    let mut categories: HashMap<ScalarValue, usize> = HashMap::new();
    let mut labels = Vec::new();
    let mut label_positions = Vec::new();
    let mut slots: HashMap<ScalarValue, usize> = HashMap::new();
    let mut slot_labels = Vec::new();
    // The fill below assigns, and an assignment is only sound while every row has a cell
    // of its own — so a (category, slot) pair seen twice refuses right here.
    let mut cells: HashSet<(usize, usize)> = HashSet::new();
    let mut of_row = Vec::with_capacity(keys.len());
    for row in 0..keys.len() {
        let key = ScalarValue::try_from_array(keys, row).map_err(|e| e.to_string())?;
        let cat = *categories.entry(key).or_insert_with(|| {
            labels.push(key_labels[row].clone());
            if let Some(p) = &key_positions {
                label_positions.push(p[row]);
            }
            labels.len() - 1
        });
        let value = ScalarValue::try_from_array(split, row).map_err(|e| e.to_string())?;
        let slot = *slots.entry(value).or_insert_with(|| {
            slot_labels.push(split_labels[row].clone());
            slot_labels.len() - 1
        });
        if !cells.insert((cat, slot)) {
            return Ok(ChartData::Duplicates {
                x: x_name.to_string(),
                series: series_name.to_string(),
            });
        }
        of_row.push((cat, slot));
    }

    let mut series = Vec::with_capacity(ys.len() * slot_labels.len());
    for (y_at, y_name) in ys.iter().enumerate() {
        for (slot, slot_label) in slot_labels.iter().enumerate() {
            let mut values = vec![None; labels.len()];
            for (row, (cat, s)) in of_row.iter().enumerate() {
                if *s == slot {
                    values[*cat] = y_cols[y_at][row];
                }
            }
            series.push(ChartSeries {
                name: if ys.len() == 1 {
                    slot_label.clone()
                } else {
                    format!("{slot_label}: {y_name}")
                },
                values,
            });
        }
    }
    Ok(ChartData::Table {
        axis: Axis {
            labels,
            positions: key_positions.is_some().then_some(label_positions),
        },
        series,
    })
}

/// The axis of a chart with no X column: the row index, 1-based like the grid's gutter.
fn row_index_axis(rows: usize) -> Axis {
    Axis {
        labels: (1..=rows).map(|i| i.to_string()).collect(),
        positions: Some((1..=rows).map(|i| Some(i as f64)).collect()),
    }
}

// ---- the raw read (scatter) ----

async fn raw(df: DataFrame, x: &str, y: &str, cap: usize) -> Result<ChartData, String> {
    // Checked before the cast rather than after: the cast DataFusion plans is the strict
    // one, so a text column would fail the read with an Arrow message about a string it
    // could not parse — where `Rows` names the column's type. One module, one answer.
    plottable(&df, x)?;
    plottable(&df, y)?;
    // Finite, not merely non-NULL: Arrow's null bitmap is unset for a NaN, so filtering
    // NULLs alone let a mark with no position through — counted against a cap that is
    // documented as counting drawable points.
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
        });
    }
    let xs = numbers(batch.column(0))?;
    let ys = numbers(batch.column(1))?;
    Ok(ChartData::Points(
        xs.into_iter()
            .zip(ys)
            .filter_map(|(x, y)| Some(ChartPoint { x: x?, y: y? }))
            .collect(),
    ))
}

// ---- the binned read (histogram — the one mark that computes) ----

async fn histogram(df: DataFrame, column: &str, bins: Option<usize>) -> Result<ChartData, String> {
    plottable(&df, column)?;
    let value = cast(ident(column), DataType::Float64);
    // Non-finite values are filtered out of **both** passes: arrow's `max` reports a NaN as
    // greater than every real value, so one NaN row would make the width NaN and the strict
    // cast fail the whole read — for a column pandas and Spark write NaN into routinely.
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
    let lo = numbers(batch.column(0))?;
    let hi = numbers(batch.column(1))?;
    let rows = integers(batch.column(2))?;
    let (Some(Some(lo)), Some(Some(hi)), Some(Some(rows))) = (lo.first(), hi.first(), rows.first())
    else {
        // Nothing to bin: every value is NULL or non-finite, which is a histogram of
        // nothing rather than a histogram of zeroes.
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
    // `hi - lo` overflows to infinity when the column spans more than f64 can hold — the
    // values are each finite, so the filter above cannot catch it, and an infinite width
    // would put every row in bin 0 under edges that are not numbers.
    if !width.is_finite() {
        return Err(format!(
            "'{column}' spans a wider range than a chart can bin"
        ));
    }
    let plan = df
        .aggregate(
            vec![floor((value - lit(lo)) / lit(width)).alias("bin")],
            vec![count(lit(1i64))],
        )
        .map_err(|e| e.to_string())?;
    let batch = one_batch(plan).await?;
    let index = numbers(batch.column(0))?;
    let counts = integers(batch.column(1))?;

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

/// The most bins a request may ask for. A histogram is a *picture* of a distribution, and
/// past a couple of hundred bars there are more bins than the canvas has columns of pixels.
/// It also keeps a bin count that arrived as a number from allocating against it.
const MAX_BINS: usize = 200;

/// Bin count when the request leaves it open: `√n`, floored at 6 and capped at 24 — enough
/// shape to read, few enough bars to label.
fn auto_bins(rows: i64) -> usize {
    let root = (rows as f64).sqrt().ceil() as usize;
    root.clamp(6, 24)
}

// ---- shared plan pieces ----

/// Keeps only values with a position on a number line. `x > -inf AND x < inf` is exactly
/// the finite predicate: a NaN fails both comparisons and each infinity fails one, and a
/// NULL propagates to NULL, which `WHERE` drops. There is no `isfinite` in DataFusion 54.
fn finite(value: Expr) -> Expr {
    let value = cast(value, DataType::Float64);
    value
        .clone()
        .gt(lit(f64::NEG_INFINITY))
        .and(value.lt(lit(f64::INFINITY)))
}

/// Refuse a column a chart cannot put on a numeric axis, naming its type — the same answer
/// [`numbers`] gives for a decoded column, given before the plan is built so every path
/// agrees.
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

/// One projected column back out of the read, by the exact name the projection put in.
/// Missing is an invariant break, not a user mistake — the select above named it.
fn projected(batch: &RecordBatch, name: &str) -> Result<ArrayRef, String> {
    batch
        .column_by_name(name)
        .cloned()
        .ok_or_else(|| format!("projected column '{name}' went missing"))
}

/// Drain a plan into a single batch, against the plan's own schema.
async fn one_batch(plan: DataFrame) -> Result<RecordBatch, String> {
    let schema = plan.schema().inner().clone();
    let batches = plan.collect().await.map_err(|e| e.to_string())?;
    concat_batches(&schema, &batches).map_err(|e| e.to_string())
}

// ---- decoding ----

/// One column as `f64`s. Cast rather than matched per type, so every numeric width decodes
/// the same way. A non-numeric column is **refused, not cast**: Arrow's array-level cast is
/// the lenient one, and `Utf8 → Float64` would turn every unparseable string into a NULL —
/// a chart of empty cells instead of the encoding error it is.
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

/// One column as `i64`s — counts, where a float round-trip would be a lie about precision.
fn integers(col: &ArrayRef) -> Result<Vec<Option<i64>>, String> {
    let cast = cast_array(col, &DataType::Int64).map_err(|e| e.to_string())?;
    let cast = cast.as_primitive::<Int64Type>();
    Ok((0..cast.len())
        .map(|i| (!cast.is_null(i)).then(|| cast.value(i)))
        .collect())
}

/// Where each X value truly sits, for a renderer that places marks rather than spacing them
/// equally: numbers as themselves, instants as epoch milliseconds, clock times as their own
/// ticks. `None` for a categorical X, which has no positions; per-entry `None` for a NULL.
fn positions(col: &ArrayRef) -> Result<Option<Vec<Option<f64>>>, String> {
    let dtype = col.data_type();
    if dtype.is_numeric() {
        return numbers(col).map(Some);
    }
    if matches!(
        dtype,
        DataType::Timestamp(_, _) | DataType::Date32 | DataType::Date64
    ) {
        let cast = cast_array(col, &DataType::Timestamp(TimeUnit::Millisecond, None))
            .map_err(|e| e.to_string())?;
        let ms = integers(&cast)?;
        return Ok(Some(ms.into_iter().map(|v| v.map(|v| v as f64)).collect()));
    }
    if matches!(dtype, DataType::Time32(_) | DataType::Time64(_)) {
        let ticks = integers(col)?;
        return Ok(Some(
            ticks.into_iter().map(|v| v.map(|v| v as f64)).collect(),
        ));
    }
    Ok(None)
}

/// One column as display labels, rendered through the **engine's** display config so a
/// value reads the way it reads in the grid, clipped to [`DISPLAY_CHARS`] like every other
/// display text this crate produces. A NULL is [`NULL_LABEL`] — spec §5 names that label,
/// and the axis is not a cell.
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::Arc;

    use datafusion::arrow::array::{
        Float64Array, Int64Array, StringArray, Time64NanosecondArray, TimestampMillisecondArray,
    };

    use super::*;

    /// Drive one read on a runtime of its own — DataFusion's operators need a Tokio context,
    /// which [`super::super::Engine`] normally owns.
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
    /// the reshaping: a single registered batch reads back in insertion order, which stands
    /// in for the ordinal order a real snapshot's read applies (`tests/engine_chart.rs`
    /// drives the same shapes through a real spooled snapshot, ordinal and all).
    fn read(columns: Vec<(&str, ArrayRef)>, q: ChartQuery) -> Result<ChartData, String> {
        let batch = RecordBatch::try_from_iter(columns).expect("fixture batch");
        let ctx = SessionContext::new();
        ctx.register_batch(snapshot_name(SnapshotId(1)).as_str(), batch)
            .expect("register fixture");
        let fmt = CellFormat::new(&BTreeMap::new());
        on_runtime(run_chart(&ctx, SnapshotId(1), &q, &fmt, None))
    }

    fn chart_of(columns: Vec<(&str, ArrayRef)>, q: ChartQuery) -> ChartData {
        read(columns, q).expect("chart")
    }

    fn rows_q(x: Option<&str>, ys: &[&str], series: Option<&str>) -> ChartQuery {
        ChartQuery::Rows {
            x: x.map(String::from),
            ys: ys.iter().map(|y| y.to_string()).collect(),
            series: series.map(String::from),
            cap: 1_000,
        }
    }

    fn table(data: ChartData) -> (Axis, Vec<ChartSeries>) {
        match data {
            ChartData::Table { axis, series } => (axis, series),
            other => panic!("expected a table, got {other:?}"),
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

    /// **The chart is the result, reshaped — nothing more.** Rows come back in result
    /// order, one mark per row, values verbatim.
    #[test]
    fn rows_draw_in_result_order_with_values_verbatim() {
        let (axis, series) = table(chart_of(
            vec![
                ("g", strs(vec![Some("b"), Some("a"), Some("c")])),
                ("v", floats(vec![Some(2.0), Some(9.0), Some(1.0)])),
            ],
            rows_q(Some("g"), &["v"], None),
        ));
        assert_eq!(
            axis.labels,
            vec!["b", "a", "c"],
            "result order, never re-ranked"
        );
        assert_eq!(axis.positions, None, "a categorical X has no positions");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].name, "v");
        assert_eq!(series[0].values, vec![Some(2.0), Some(9.0), Some(1.0)]);
    }

    /// **Multiple Y columns are multiple series**, named by column — `SELECT month, revenue,
    /// cost` is two lines with no configuration.
    #[test]
    fn each_y_column_is_its_own_series() {
        let (_, series) = table(chart_of(
            vec![
                ("m", strs(vec![Some("jan"), Some("feb")])),
                ("revenue", ints(vec![Some(10), Some(20)])),
                ("cost", ints(vec![Some(4), Some(6)])),
            ],
            rows_q(Some("m"), &["revenue", "cost"], None),
        ));
        let names: Vec<&str> = series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["revenue", "cost"],
            "request order, named by column"
        );
        assert_eq!(series[0].values, vec![Some(10.0), Some(20.0)]);
        assert_eq!(series[1].values, vec![Some(4.0), Some(6.0)]);
    }

    /// A NULL Y is a gap; duplicate X labels are duplicate marks — the chart shows what the
    /// result holds.
    #[test]
    fn null_y_gaps_and_duplicate_labels_draw_twice() {
        let (axis, series) = table(chart_of(
            vec![
                ("g", strs(vec![Some("a"), Some("a"), Some("b")])),
                ("v", ints(vec![Some(1), None, Some(3)])),
            ],
            rows_q(Some("g"), &["v"], None),
        ));
        assert_eq!(axis.labels, vec!["a", "a", "b"]);
        assert_eq!(series[0].values, vec![Some(1.0), None, Some(3.0)]);
    }

    /// **The pivot reshapes long to wide**: one series per distinct series value, named by
    /// value, categories in first-appearance (= result) order, absent cells `None`.
    #[test]
    fn a_series_column_pivots_long_to_wide() {
        let (axis, series) = table(chart_of(
            vec![
                ("m", strs(vec![Some("jan"), Some("jan"), Some("feb")])),
                ("region", strs(vec![Some("eu"), Some("us"), Some("eu")])),
                ("v", ints(vec![Some(1), Some(2), Some(3)])),
            ],
            rows_q(Some("m"), &["v"], Some("region")),
        ));
        assert_eq!(axis.labels, vec!["jan", "feb"]);
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].name, "eu");
        assert_eq!(series[0].values, vec![Some(1.0), Some(3.0)]);
        assert_eq!(series[1].name, "us");
        assert_eq!(
            series[1].values,
            vec![Some(2.0), None],
            "(feb, us) is a pair the data never contained"
        );
    }

    /// With several Ys and a series column, both split and the legend names both.
    #[test]
    fn a_series_split_and_several_ys_both_name_the_series() {
        let (_, series) = table(chart_of(
            vec![
                ("m", strs(vec![Some("jan"), Some("jan")])),
                ("r", strs(vec![Some("eu"), Some("us")])),
                ("a", ints(vec![Some(1), Some(2)])),
                ("b", ints(vec![Some(3), Some(4)])),
            ],
            rows_q(Some("m"), &["a", "b"], Some("r")),
        ));
        let names: Vec<&str> = series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["eu: a", "us: a", "eu: b", "us: b"]);
    }

    /// **Two rows in one pivot cell refuse** — aggregating them is SQL's job, and the
    /// refusal names the encoding so the message can say which columns to group by.
    #[test]
    fn a_duplicate_pivot_cell_refuses_rather_than_keeping_either_row() {
        let data = chart_of(
            vec![
                ("m", strs(vec![Some("jan"), Some("jan")])),
                ("r", strs(vec![Some("eu"), Some("eu")])),
                ("v", ints(vec![Some(1), Some(2)])),
            ],
            rows_q(Some("m"), &["v"], Some("r")),
        );
        assert_eq!(
            data,
            ChartData::Duplicates {
                x: "m".into(),
                series: "r".into()
            }
        );
    }

    /// **A NULL and a literal `"(null)"` are two categories and two series** — the label is
    /// never the key.
    #[test]
    fn null_is_a_label_never_a_key() {
        let (axis, series) = table(chart_of(
            vec![
                ("g", strs(vec![Some("(null)"), None])),
                ("r", strs(vec![Some("eu"), Some("eu")])),
                ("v", ints(vec![Some(1), Some(2)])),
            ],
            rows_q(Some("g"), &["v"], Some("r")),
        ));
        assert_eq!(
            axis.labels,
            vec![NULL_LABEL, NULL_LABEL],
            "two categories that render alike"
        );
        let total: f64 = series.iter().flat_map(|s| s.values.iter().flatten()).sum();
        assert_eq!(
            total, 3.0,
            "no row went missing in the merge that must not happen"
        );

        let (_, series) = table(chart_of(
            vec![
                ("g", strs(vec![Some("a"), Some("b")])),
                ("r", strs(vec![Some("(null)"), None])),
                ("v", ints(vec![Some(1), Some(2)])),
            ],
            rows_q(Some("g"), &["v"], Some("r")),
        ));
        assert_eq!(series.len(), 2, "two series that render alike: {series:?}");
        assert_eq!(series[0].name, NULL_LABEL);
        assert_eq!(series[1].name, NULL_LABEL);
    }

    /// **Exactly `cap` rows draw; one more refuses** — never a truncated chart.
    #[test]
    fn the_cap_refuses_instead_of_truncating() {
        let fixture = || {
            vec![
                ("g", strs(vec![Some("a"), Some("b"), Some("c")])),
                ("v", ints(vec![Some(1), Some(2), Some(3)])),
            ]
        };
        let capped = |cap: usize| {
            chart_of(
                fixture(),
                ChartQuery::Rows {
                    x: Some("g".into()),
                    ys: vec!["v".into()],
                    series: None,
                    cap,
                },
            )
        };
        assert_eq!(
            capped(2),
            ChartData::OverCap {
                unit: CapUnit::Rows,
                cap: 2
            }
        );
        let (axis, _) = table(capped(3));
        assert_eq!(axis.labels.len(), 3, "exactly at the cap still draws");
    }

    /// A numeric X carries true positions; a NULL X has a label but no position.
    #[test]
    fn a_numeric_x_carries_positions() {
        let (axis, _) = table(chart_of(
            vec![
                ("n", floats(vec![Some(1.0), Some(100.0), None])),
                ("v", ints(vec![Some(1), Some(2), Some(3)])),
            ],
            rows_q(Some("n"), &["v"], None),
        ));
        assert_eq!(axis.labels.len(), 3);
        assert_eq!(axis.labels[2], NULL_LABEL);
        assert_eq!(
            axis.positions,
            Some(vec![Some(1.0), Some(100.0), None]),
            "a renderer can place 1 and 100 truly rather than equally spaced"
        );
    }

    /// A temporal X positions at epoch milliseconds and labels through the engine's display
    /// config; a clock time positions at its own ticks.
    #[test]
    fn temporal_and_clock_xs_carry_positions() {
        let jan = 1_704_412_800_000i64;
        let (axis, _) = table(chart_of(
            vec![
                (
                    "t",
                    Arc::new(TimestampMillisecondArray::from(vec![jan, jan + 86_400_000]))
                        as ArrayRef,
                ),
                ("v", ints(vec![Some(1), Some(2)])),
            ],
            rows_q(Some("t"), &["v"], None),
        ));
        assert_eq!(
            axis.positions,
            Some(vec![Some(jan as f64), Some(jan as f64 + 86_400_000.0)])
        );

        let hour = 3_600_000_000_000i64;
        let (axis, _) = table(chart_of(
            vec![
                (
                    "t",
                    Arc::new(Time64NanosecondArray::from(vec![9 * hour, 14 * hour])) as ArrayRef,
                ),
                ("v", ints(vec![Some(1), Some(2)])),
            ],
            rows_q(Some("t"), &["v"], None),
        ));
        assert_eq!(axis.labels, vec!["09:00:00", "14:00:00"]);
        assert_eq!(
            axis.positions,
            Some(vec![Some(9.0 * hour as f64), Some(14.0 * hour as f64)])
        );
    }

    /// No X at all charts against the 1-based row index, like the grid's gutter.
    #[test]
    fn no_x_charts_against_the_row_index() {
        let (axis, series) = table(chart_of(
            vec![("v", ints(vec![Some(5), Some(7)]))],
            rows_q(None, &["v"], None),
        ));
        assert_eq!(axis.labels, vec!["1", "2"]);
        assert_eq!(axis.positions, Some(vec![Some(1.0), Some(2.0)]));
        assert_eq!(series[0].values, vec![Some(5.0), Some(7.0)]);
    }

    /// An empty result draws an empty axis — not one invented category.
    #[test]
    fn an_empty_result_draws_an_empty_axis() {
        let (axis, series) = table(chart_of(
            vec![("g", strs(Vec::new())), ("v", ints(Vec::new()))],
            rows_q(Some("g"), &["v"], None),
        ));
        assert!(axis.labels.is_empty());
        assert!(series.iter().all(|s| s.values.is_empty()));
    }

    /// A column can be X and Y at once — the projection names it once and reads it twice.
    #[test]
    fn a_column_can_be_both_x_and_y() {
        let (axis, series) = table(chart_of(
            vec![("v", ints(vec![Some(3), Some(1)]))],
            rows_q(Some("v"), &["v"], None),
        ));
        assert_eq!(axis.labels, vec!["3", "1"]);
        assert_eq!(series[0].values, vec![Some(3.0), Some(1.0)]);
    }

    /// Every encoding mistake is refused in this module's words, before DataFusion can
    /// answer it in its own.
    #[test]
    fn encoding_mistakes_are_named_here() {
        let fixture = || vec![("g", strs(vec![Some("a")])), ("v", ints(vec![Some(1)]))];
        let err = read(fixture(), rows_q(Some("g"), &[], None)).expect_err("no Y");
        assert!(err.contains("Y column"), "{err}");

        let err = read(fixture(), rows_q(Some("g"), &["g"], None)).expect_err("text Y");
        assert!(err.contains("not a measure"), "{err}");

        let err = read(fixture(), rows_q(None, &["v"], Some("g"))).expect_err("series, no X");
        assert!(err.contains("X column"), "{err}");

        let err = read(fixture(), rows_q(Some("g"), &["v"], Some("g"))).expect_err("X == series");
        assert!(err.contains("'g'"), "{err}");
        assert!(!err.contains("Schema error"), "{err}");

        let err = read(fixture(), rows_q(Some("nope"), &["v"], None)).expect_err("missing");
        assert!(err.contains("'nope'"), "{err}");
    }

    /// **Scatter returns raw points**, finite ones only — a NaN has no position on a plane,
    /// and the null bitmap is unset for it.
    #[test]
    fn scatter_returns_the_finite_points() {
        let data = chart_of(
            vec![
                (
                    "x",
                    floats(vec![Some(1.0), Some(f64::NAN), None, Some(4.0)]),
                ),
                (
                    "y",
                    floats(vec![
                        Some(10.0),
                        Some(20.0),
                        Some(30.0),
                        Some(f64::INFINITY),
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
            ChartData::Points(vec![ChartPoint { x: 1.0, y: 10.0 }])
        );
    }

    /// The point cap counts drawable points, and refuses rather than thinning.
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
                cap: 2
            }
        );
    }

    /// A scatter over a text column is refused by naming the type — not with an Arrow parse
    /// message.
    #[test]
    fn a_scatter_over_text_is_refused_by_type() {
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

    /// **Bins are uniform, contiguous, and account for every finite row** — the top of the
    /// range included, which is why the last bin is closed.
    #[test]
    fn a_histogram_bins_uniformly_and_loses_no_finite_row() {
        let mut values: Vec<Option<f64>> = (1..=10).map(|i| Some(i as f64)).collect();
        values.push(Some(f64::NAN));
        values.push(Some(f64::INFINITY));
        let data = chart_of(
            vec![("v", floats(values))],
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

    /// A column spanning more than f64 can hold makes `hi - lo` infinite — refused by name,
    /// not binned under edges that are not numbers.
    #[test]
    fn a_range_too_wide_to_bin_is_refused() {
        let err = read(
            vec![("v", floats(vec![Some(-1.7e308), Some(1.7e308)]))],
            ChartQuery::Histogram {
                col: "v".into(),
                bins: Some(4),
            },
        )
        .expect_err("the width is not a number");
        assert!(err.contains("'v'"), "{err}");
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

    /// The bin count comes from the row count when the request leaves it open, bounded both
    /// ways.
    #[test]
    fn an_open_bin_count_is_bounded_both_ways() {
        assert_eq!(auto_bins(1), 6);
        assert_eq!(auto_bins(100), 10);
        assert_eq!(auto_bins(10_000_000), 24);
    }
}
