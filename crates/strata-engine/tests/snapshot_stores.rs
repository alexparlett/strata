//! `SnapshotStore` at the facade: the reads a frontend makes work whatever the results are
//! kept in, and a [`MemSnapshotStore`] engine makes them with nothing on disk.
//!
//! The two halves are one test on purpose. "Needs no spool" is only worth asserting beside a
//! store that demonstrably does: the control engine is pointed at a scratch root, fills it while
//! serving exactly the same three reads, and the mem engine then serves them again while that
//! root — and the machine-shared one the default would have claimed under — stay exactly as they
//! were.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use strata_engine::{Engine, LocalIpcSnapshotStore, MemSnapshotStore, RunTag, WsId};
use strata_model::{ChartData, ChartQuery, SnapshotId};

const SQL: &str = "SELECT column1 AS region, column2 AS amount, column3 AS qty \
     FROM (VALUES ('eu', 30.0, 3), ('us', 20.0, 2), ('ap', 10.0, 1)) AS t \
     ORDER BY column2 DESC";

/// Everything under `root`, deepest first — absent reads as empty, which is the state the
/// shared root is usually in when this runs.
fn entries(root: &Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        found.push(path.to_string_lossy().into_owned());
        if path.is_dir() {
            found.extend(entries(&path));
        }
    }
    found.sort();
    found
}

async fn run(eng: &Engine) -> SnapshotId {
    let (output, _) = eng
        .ws(WsId(1))
        .query(RunTag(1), SQL.into(), 10)
        .await
        .expect("run");
    output.snapshot.expect("a non-empty result settles one")
}

/// The three reads a frontend makes of a settled result, each asserted on its answer rather
/// than on its succeeding — a store that lost the ordinal or the types would still return `Ok`.
async fn reads(eng: &Engine, snap: SnapshotId) {
    let (rows, _) = eng.snapshot(snap).page(1, 10, None).await.expect("page 1");
    let regions: Vec<&str> = rows.iter().map(|r| r[0].text.as_str()).collect();
    assert_eq!(
        regions,
        vec!["eu", "us", "ap"],
        "a page reads the result in the order it was spooled"
    );

    let (sorted, _) = eng
        .snapshot(snap)
        .page(2, 2, Some(("region".into(), true)))
        .await
        .expect("a sorted page");
    assert_eq!(sorted.len(), 1, "the sort is over the whole snapshot");
    assert_eq!(sorted[0][0].text, "us");

    let data = eng
        .snapshot(snap)
        .chart(ChartQuery::Rows {
            x: Some("region".into()),
            ys: vec!["amount".into()],
            series: None,
            cap: 1_000,
        })
        .await
        .expect("chart");
    let ChartData::Table { axis, series } = data else {
        panic!("expected a table, got {data:?}")
    };
    assert_eq!(axis.labels, vec!["eu", "us", "ap"]);
    assert_eq!(series[0].values, vec![Some(30.0), Some(20.0), Some(10.0)]);

    let trend = eng
        .snapshot(snap)
        .trend("qty".into(), "amount".into())
        .await
        .expect("trend")
        .expect("three points with x-variance support a fit");
    assert!(
        (trend.slope - 10.0).abs() < 1e-9,
        "amount is 10 * qty, so the fit says so: {trend:?}"
    );

    assert!(eng.snapshot(snap).live());
}

#[tokio::test]
async fn a_mem_store_serves_every_read_the_spool_does_and_touches_no_disk() {
    let scratch: PathBuf =
        env::temp_dir().join(format!("strata_snapshot_store_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let shared = env::temp_dir().join("strata_snapshots");

    {
        let eng = Engine::builder()
            .with_snapshot_store(LocalIpcSnapshotStore::new_in(&scratch))
            .build();
        let snap = run(&eng).await;
        reads(&eng, snap).await;
        assert!(
            entries(&scratch).iter().any(|e| e.ends_with(".arrow")),
            "the control store spooled the result it was serving"
        );
    }

    let before_scratch = entries(&scratch);
    let before_shared = entries(&shared);
    {
        let eng = Engine::builder()
            .with_snapshot_store(MemSnapshotStore::new())
            .build();
        let snap = run(&eng).await;
        reads(&eng, snap).await;
        assert_eq!(
            entries(&scratch),
            before_scratch,
            "a mem-store engine wrote nothing where the control store writes"
        );
        assert_eq!(
            entries(&shared),
            before_shared,
            "…and claimed nothing under the root the default would have"
        );
    }

    let _ = fs::remove_dir_all(&scratch);
}
