//! `Engine::chart` against a **real snapshot** (Rz2 acceptance): all three query shapes,
//! driven straight through the facade the way a freya-query capability calls it — a Run
//! spools the result to an Arrow IPC file, and every chart is a grouped/raw/binned read of
//! *that*, never of the source.
//!
//! The unit tests in `engine::chart` cover the answer matrix over in-memory fixtures. What
//! only a real snapshot can show is the round trip: a `Timestamp(Nanosecond)` written to
//! IPC, read back, cast and bucketed; a chart and a page read of the same immutable result
//! agreeing; and the read surviving as a registered table between the two.

use std::sync::Arc;

use strata_core::engine::{Engine, RunTag, WsId};
use strata_model::{
    AggFn, Bucket, CapUnit, ChartData, ChartPoint, ChartQuery, Measure, SnapshotId, Stride, Width,
};

/// Three months of two regions, with **March missing** — the gap a line must not draw
/// across — and a numeric pair for the numeric-X, scatter and histogram shapes.
const SQL: &str = "SELECT column1 AS at, column2 AS region, column3 AS amount, column4 AS qty \
     FROM (VALUES \
       (TIMESTAMP '2024-01-05 00:00:00', 'eu', 10.0, 1), \
       (TIMESTAMP '2024-02-11 00:00:00', 'eu', 20.0, 2), \
       (TIMESTAMP '2024-04-02 00:00:00', 'us', 40.0, 4)) AS t";

async fn snapshot(eng: &Engine) -> SnapshotId {
    let (output, _) = eng
        .query(WsId(1), RunTag(1), SQL.into(), 10)
        .await
        .expect("run");
    output
        .snapshot
        .expect("a non-empty result materializes one")
}

fn sum(y: &str) -> Vec<Measure> {
    vec![Measure {
        y: Some(y.into()),
        agg_fn: AggFn::Sum,
    }]
}

fn rows() -> Vec<Measure> {
    vec![Measure {
        y: None,
        agg_fn: AggFn::Count,
    }]
}

/// The aggregate shape end to end: a timestamp that survived the IPC round trip is bucketed
/// by month, the empty month comes back as a gap, and a series splits each bucket.
#[tokio::test]
async fn an_aggregate_chart_reads_the_snapshot_the_grid_is_paging() {
    let eng = Arc::new(Engine::new(Default::default()));
    let snapshot = snapshot(&eng).await;

    let data = eng
        .chart(
            snapshot,
            ChartQuery::Aggregate {
                x: Some("at".into()),
                series: Some("region".into()),
                measures: sum("amount"),
                bucket: Some(Bucket::Time(Stride::Month)),
                group_cap: 1_000,
            },
        )
        .await
        .expect("chart");

    let ChartData::Grouped {
        categories,
        series,
        bucket,
    } = data
    else {
        panic!("expected a grouped chart, got {data:?}")
    };
    assert_eq!(bucket, Some(Bucket::Time(Stride::Month)));
    assert_eq!(categories.len(), 4, "January through April: {categories:?}");
    assert!(categories[0].starts_with("2024-01-01"), "{categories:?}");
    let values = |name: &str| {
        series
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("series {name}: {series:?}"))
            .values
            .clone()
    };
    assert_eq!(values("eu"), vec![Some(10.0), Some(20.0), None, None]);
    assert_eq!(values("us"), vec![None, None, None, Some(40.0)]);

    // The chart is a read, not a consumption: the same snapshot still pages.
    let (rows, _) = eng.fetch_page(snapshot, 1, 10, None).await.expect("page");
    assert_eq!(rows.len(), 3, "the grid still reads the result it charted");
}

/// A bucket the request leaves open is resolved from the column's own span, and reported.
#[tokio::test]
async fn an_open_bucket_comes_back_named() {
    let eng = Arc::new(Engine::new(Default::default()));
    let snapshot = snapshot(&eng).await;

    let data = eng
        .chart(
            snapshot,
            ChartQuery::Aggregate {
                x: Some("at".into()),
                series: None,
                measures: rows(),
                bucket: None,
                group_cap: 1_000,
            },
        )
        .await
        .expect("chart");
    let ChartData::Grouped { bucket, .. } = data else {
        panic!("expected a grouped chart, got {data:?}")
    };
    // Just under three months apart: the ladder's daily rung, and 88 buckets is well
    // inside the cap, so nothing widens it.
    assert_eq!(bucket, Some(Bucket::Time(Stride::Day)));
}

/// A numeric X is first-class on the aggregate shape: grouped by value, or binned to a
/// uniform width with the empty bins filled back in.
#[tokio::test]
async fn a_numeric_x_groups_by_value_and_bins_on_request() {
    let eng = Arc::new(Engine::new(Default::default()));
    let snapshot = snapshot(&eng).await;

    let by_value = eng
        .chart(
            snapshot,
            ChartQuery::Aggregate {
                x: Some("qty".into()),
                series: None,
                measures: sum("amount"),
                bucket: None,
                group_cap: 1_000,
            },
        )
        .await
        .expect("chart");
    let ChartData::Grouped {
        categories,
        series,
        bucket,
    } = by_value
    else {
        panic!("expected a grouped chart, got {by_value:?}")
    };
    assert_eq!(categories, vec!["1", "2", "4"], "ascending by value");
    assert_eq!(series[0].values, vec![Some(10.0), Some(20.0), Some(40.0)]);
    assert_eq!(bucket, None, "grouping by value is not bucketing");

    let width = Width::new(2.0).expect("a width");
    let binned = eng
        .chart(
            snapshot,
            ChartQuery::Aggregate {
                x: Some("qty".into()),
                series: None,
                measures: sum("amount"),
                bucket: Some(Bucket::Width(width)),
                group_cap: 1_000,
            },
        )
        .await
        .expect("chart");
    let ChartData::Grouped {
        categories,
        series,
        bucket,
    } = binned
    else {
        panic!("expected a grouped chart, got {binned:?}")
    };
    assert_eq!(bucket, Some(Bucket::Width(width)));
    // qty 1 → [0,2), qty 2 → [2,4), qty 4 → [4,6): three bins, none of them empty here.
    assert_eq!(categories, vec!["0.0", "2.0", "4.0"]);
    assert_eq!(series[0].values, vec![Some(10.0), Some(20.0), Some(40.0)]);
}

/// The raw shape: two numeric columns, no aggregation, one point per row.
#[tokio::test]
async fn a_raw_chart_returns_one_point_per_row() {
    let eng = Arc::new(Engine::new(Default::default()));
    let snapshot = snapshot(&eng).await;

    let data = eng
        .chart(
            snapshot,
            ChartQuery::Raw {
                x: "qty".into(),
                y: "amount".into(),
                cap: 6_000,
            },
        )
        .await
        .expect("chart");
    assert_eq!(
        data,
        ChartData::Points(vec![
            ChartPoint { x: 1.0, y: 10.0 },
            ChartPoint { x: 2.0, y: 20.0 },
            ChartPoint { x: 4.0, y: 40.0 },
        ])
    );
}

/// Over its cap the raw shape refuses, and the refusal names what it counted.
#[tokio::test]
async fn a_raw_chart_past_its_cap_refuses() {
    let eng = Arc::new(Engine::new(Default::default()));
    let snapshot = snapshot(&eng).await;

    let data = eng
        .chart(
            snapshot,
            ChartQuery::Raw {
                x: "qty".into(),
                y: "amount".into(),
                cap: 2,
            },
        )
        .await
        .expect("chart");
    assert_eq!(
        data,
        ChartData::OverCap {
            unit: CapUnit::Points,
            cap: 2,
            bucket: None
        }
    );
}

/// The binned shape: uniform bins over one numeric column, accounting for every row.
#[tokio::test]
async fn a_histogram_bins_the_snapshot() {
    let eng = Arc::new(Engine::new(Default::default()));
    let snapshot = snapshot(&eng).await;

    let data = eng
        .chart(
            snapshot,
            ChartQuery::Histogram {
                col: "amount".into(),
                bins: Some(3),
            },
        )
        .await
        .expect("chart");
    let ChartData::Bins(bins) = data else {
        panic!("expected bins, got {data:?}")
    };
    assert_eq!(bins.len(), 3);
    assert_eq!(bins[0].lo, 10.0);
    assert_eq!(bins[2].hi, 40.0);
    assert_eq!(bins.iter().map(|b| b.count).sum::<u64>(), 3);
}

/// A chart of a retired snapshot fails like any other read of one — the caller tells that
/// from a real fault by asking [`Engine::snapshot_live`], never by matching prose.
#[tokio::test]
async fn charting_a_retired_snapshot_fails_like_any_other_read() {
    let eng = Arc::new(Engine::new(Default::default()));
    let snapshot = snapshot(&eng).await;
    // A re-run in the same workspace retires the previous snapshot at dispatch.
    let _ = eng
        .query(WsId(1), RunTag(2), "SELECT 1 AS n".into(), 10)
        .await
        .expect("re-run");
    assert!(!eng.snapshot_live(snapshot));

    let err = eng
        .chart(
            snapshot,
            ChartQuery::Histogram {
                col: "amount".into(),
                bins: None,
            },
        )
        .await
        .expect_err("a retired snapshot has nothing to chart");
    assert!(!err.is_empty());
}
