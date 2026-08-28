//! `SnapshotReads::chart` against a **real snapshot** (Rz2 acceptance): the renderer-first shapes,
//! driven straight through the facade the way a freya-query capability calls it — a Run
//! spools the result to an Arrow IPC file, and every chart is a projected, ordinal-ordered
//! read of *that*, never of the source.
//!
//! The unit tests in `engine::chart` cover the reshaping matrix over in-memory fixtures.
//! What only a real snapshot can show is the round trip: the ordinal ordering a real read
//! applies, types surviving IPC, the chart and the grid agreeing on the same result, and a
//! retired snapshot failing cleanly.

use std::collections::BTreeMap;

use strata_arrow::config::{display_subset, DisplayStamp};
use strata_engine::{Engine, RunRows, RunTag, WsId};
use strata_model::{CapUnit, ChartData, ChartPoint, ChartQuery, PageQuery, SnapshotId};

/// A result the user shaped themselves — ordered DESC by amount, the exact case the
/// renderer-first design exists for: the chart must draw it in *this* order.
const SQL: &str = "SELECT column1 AS region, column2 AS amount, column3 AS qty \
     FROM (VALUES ('eu', 30.0, 3), ('us', 20.0, 2), ('ap', 10.0, 1)) AS t \
     ORDER BY column2 DESC";

async fn snapshot(eng: &Engine) -> SnapshotId {
    let RunRows { output, .. } = eng
        .ws(WsId(1))
        .query(RunTag(1), SQL.into(), 10)
        .await
        .expect("run");
    output
        .snapshot
        .expect("a non-empty result materializes one")
}

fn rows_q(x: Option<&str>, ys: &[&str], series: Option<&str>) -> ChartQuery {
    ChartQuery::Rows {
        x: x.map(String::from),
        ys: ys.iter().map(ToString::to_string).collect(),
        series: series.map(String::from),
        cap: 1_000,
    }
}

/// **The chart draws the user's order.** The result was `ORDER BY amount DESC`; the axis is
/// exactly that, and the same snapshot still pages for the grid.
#[tokio::test]
async fn the_chart_draws_the_result_in_its_own_order() {
    let eng = Engine::builder().build();
    let snap = snapshot(&eng).await;

    let data = eng
        .snapshot(snap)
        .chart(
            rows_q(Some("region"), &["amount"], None),
            DisplayStamp::default(),
        )
        .await
        .expect("chart");
    let ChartData::Table { axis, series } = data else {
        panic!("expected a table, got {data:?}")
    };
    assert_eq!(
        axis.labels,
        vec!["eu", "us", "ap"],
        "the ORDER BY the user wrote is the axis order"
    );
    assert_eq!(series[0].name, "amount");
    assert_eq!(series[0].values, vec![Some(30.0), Some(20.0), Some(10.0)]);

    let page = eng
        .snapshot(snap)
        .page(
            PageQuery {
                page: 1,
                page_size: 10,
                sort: None,
            },
            DisplayStamp::default(),
        )
        .await
        .expect("page");
    let grid: Vec<&str> = page.rows.iter().map(|r| r[0].text.as_str()).collect();
    assert_eq!(grid, vec!["eu", "us", "ap"], "chart and grid agree");
}

/// Multiple Y columns are multiple series over a real snapshot too.
#[tokio::test]
async fn several_ys_split_into_series_over_a_real_snapshot() {
    let eng = Engine::builder().build();
    let snap = snapshot(&eng).await;

    let data = eng
        .snapshot(snap)
        .chart(
            rows_q(Some("region"), &["amount", "qty"], None),
            DisplayStamp::default(),
        )
        .await
        .expect("chart");
    let ChartData::Table { series, .. } = data else {
        panic!("expected a table, got {data:?}")
    };
    let names: Vec<&str> = series.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["amount", "qty"]);
    assert_eq!(series[1].values, vec![Some(3.0), Some(2.0), Some(1.0)]);
}

/// The pivot works end to end: a long result splits into one series per distinct value,
/// categories in result order, absent cells as gaps.
#[tokio::test]
async fn a_series_column_pivots_over_a_real_snapshot() {
    let eng = Engine::builder().build();
    let RunRows { output: out, .. } = eng
        .ws(WsId(1))
        .query(
            RunTag(1),
            "SELECT column1 AS m, column2 AS region, column3 AS v \
             FROM (VALUES ('jan', 'eu', 1.0), ('jan', 'us', 2.0), ('feb', 'eu', 3.0)) AS t"
                .into(),
            10,
        )
        .await
        .expect("run");
    let snap = out.snapshot.expect("snapshot");

    let data = eng
        .snapshot(snap)
        .chart(
            rows_q(Some("m"), &["v"], Some("region")),
            DisplayStamp::default(),
        )
        .await
        .expect("chart");
    let ChartData::Table { axis, series } = data else {
        panic!("expected a table, got {data:?}")
    };
    assert_eq!(axis.labels, vec!["jan", "feb"]);
    assert_eq!(series[0].name, "eu");
    assert_eq!(series[0].values, vec![Some(1.0), Some(3.0)]);
    assert_eq!(series[1].name, "us");
    assert_eq!(series[1].values, vec![Some(2.0), None]);
}

/// The raw shape: finite points, cap honoured with a refusal.
#[tokio::test]
async fn scatter_returns_points_and_refuses_over_cap() {
    let eng = Engine::builder().build();
    let snap = snapshot(&eng).await;

    let data = eng
        .snapshot(snap)
        .chart(
            ChartQuery::Raw {
                x: "qty".into(),
                y: "amount".into(),
                cap: 6_000,
            },
            DisplayStamp::default(),
        )
        .await
        .expect("chart");
    let ChartData::Points(mut points) = data else {
        panic!("expected points, got {data:?}")
    };
    points.sort_by(|a, b| a.x.total_cmp(&b.x));
    assert_eq!(
        points,
        vec![
            ChartPoint { x: 1.0, y: 10.0 },
            ChartPoint { x: 2.0, y: 20.0 },
            ChartPoint { x: 3.0, y: 30.0 },
        ]
    );

    let data = eng
        .snapshot(snap)
        .chart(
            ChartQuery::Raw {
                x: "qty".into(),
                y: "amount".into(),
                cap: 2,
            },
            DisplayStamp::default(),
        )
        .await
        .expect("chart");
    assert_eq!(
        data,
        ChartData::OverCap {
            unit: CapUnit::Points,
            cap: 2
        }
    );
}

/// The binned shape over a real snapshot.
#[tokio::test]
async fn a_histogram_bins_the_snapshot() {
    let eng = Engine::builder().build();
    let snap = snapshot(&eng).await;

    let data = eng
        .snapshot(snap)
        .chart(
            ChartQuery::Histogram {
                col: "amount".into(),
                bins: Some(2),
            },
            DisplayStamp::default(),
        )
        .await
        .expect("chart");
    let ChartData::Bins(bins) = data else {
        panic!("expected bins, got {data:?}")
    };
    assert_eq!(bins.len(), 2);
    assert_eq!(bins[0].lo, 10.0);
    assert_eq!(bins[1].hi, 30.0);
    assert_eq!(bins.iter().map(|b| b.count).sum::<u64>(), 3);
}

/// The trendline is the engine's own fit over the spooled snapshot, and degenerate data is
/// an absent overlay rather than an error the user must dismiss (Chart 11).
#[tokio::test]
async fn a_trendline_fits_a_real_snapshot_and_degenerate_data_is_absent() {
    let eng = Engine::builder().build();
    let snap = snapshot(&eng).await;

    let fit = eng
        .snapshot(snap)
        .trend("qty".into(), "amount".into())
        .await
        .expect("trend")
        .expect("three clean pairs fit a line");
    assert!((fit.slope - 10.).abs() < 1e-9, "{fit:?}");
    assert!(fit.intercept.abs() < 1e-9, "{fit:?}");
    assert!((fit.r2 - 1.).abs() < 1e-9, "{fit:?}");
    assert_eq!(fit.n, 3);

    let RunRows { output: out, .. } = eng
        .ws(WsId(2))
        .query(RunTag(1), "SELECT 1.0 AS x, 2.0 AS y".into(), 10)
        .await
        .expect("run");
    let one = out.snapshot.expect("snapshot");
    assert_eq!(
        eng.snapshot(one)
            .trend("x".into(), "y".into())
            .await
            .expect("trend"),
        None
    );
}

/// A chart of a retired snapshot fails like any other read of one — the caller tells that
/// from a real fault by asking `SnapshotReads::live`, never by matching prose.
#[tokio::test]
async fn charting_a_retired_snapshot_fails_like_any_other_read() {
    let eng = Engine::builder().build();
    let snap = snapshot(&eng).await;
    let _ = eng
        .ws(WsId(1))
        .query(RunTag(2), "SELECT 1 AS n".into(), 10)
        .await
        .expect("re-run");
    assert!(!eng.snapshot(snap).live());

    let err = eng
        .snapshot(snap)
        .chart(
            rows_q(Some("region"), &["amount"], None),
            DisplayStamp::default(),
        )
        .await
        .expect_err("a retired snapshot has nothing to chart");
    assert!(!err.to_string().is_empty());
}

/// **The axis renders through the stamp too**: a chart's labels come out of the same cell
/// formatter, so the read answers under the stamp it was handed.
#[tokio::test]
async fn the_axis_renders_through_the_stamp_it_was_handed() {
    async fn axis_under(eng: &Engine, snap: SnapshotId, display: DisplayStamp) -> Vec<String> {
        let q = rows_q(Some("day"), &["amount"], None);
        match eng.snapshot(snap).chart(q, display).await.expect("chart") {
            ChartData::Table { axis, .. } => axis.labels,
            other => panic!("expected a table, got {other:?}"),
        }
    }

    let eng = Engine::builder().build();
    let RunRows { output, .. } = eng
        .ws(WsId(1))
        .query(
            RunTag(1),
            "SELECT DATE '2026-08-28' AS day, 1.0 AS amount".into(),
            10,
        )
        .await
        .expect("run");
    let snap = output.snapshot.expect("a row is a snapshot");

    assert_eq!(
        axis_under(&eng, snap, DisplayStamp::default()).await,
        vec!["2026-08-28"]
    );

    let overrides: BTreeMap<String, String> = [(
        "datafusion.format.date_format".to_string(),
        "%d/%m/%Y".to_string(),
    )]
    .into_iter()
    .collect();
    assert_eq!(
        axis_under(&eng, snap, display_subset(&overrides)).await,
        vec!["28/08/2026"],
        "the stamp is what the labels render through"
    );
}
