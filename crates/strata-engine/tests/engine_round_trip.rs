//! The query round-trip against the real engine (P2-01 acceptance): a Run materializes
//! an immutable snapshot; page reads target that snapshot (stable under re-reads); a
//! re-run makes a *new* snapshot and retires the old; cleanup retires everything.
//!
//! Driven straight through the `Engine` facade — no UI framework involved, exactly as a
//! freya-query capability calls it. The test runtime awaits the engine's `JoinHandle`s
//! across runtimes, which is the same executor-agnostic await the Freya executor does.

use std::collections::BTreeMap;
use std::sync::Arc;

use strata_arrow::config::{display_subset, DisplayStamp};
use strata_engine::{ConfigOutcome, Engine, RunRows, RunTag, WsId};
use strata_model::PageQuery;

/// Five rows, three columns, unsorted on `column1` so the sort read is observable.
const SQL: &str = "SELECT * FROM (VALUES (3, 'c', true), (1, 'a', false), (5, 'e', true), (2, 'b', false), (4, 'd', true)) AS t";

fn engine() -> Arc<Engine> {
    Engine::builder().build()
}

fn ws(n: u128) -> WsId {
    WsId(n)
}

fn tag(n: u128) -> RunTag {
    RunTag(n)
}

#[tokio::test]
async fn run_materializes_a_snapshot_and_pages_read_it() {
    let eng = engine();

    let RunRows { output, batch, .. } = eng
        .ws(ws(1))
        .query(tag(1), SQL.into(), 2)
        .await
        .expect("run");
    let snapshot = output
        .snapshot
        .expect("a non-empty result materializes a snapshot");
    assert_eq!(output.total, 5);
    assert_eq!(output.page, 1);
    assert_eq!(output.rows.len(), 2, "page 1 rides with the run");
    assert_eq!(output.columns.len(), 3);
    assert_eq!(batch.num_rows(), 2);

    let rows = eng
        .snapshot(snapshot)
        .page(
            PageQuery {
                page: 2,
                page_size: 2,
                sort: None,
            },
            DisplayStamp::default(),
        )
        .await
        .expect("page 2")
        .rows;
    assert_eq!(rows.len(), 2);
    let rows = eng
        .snapshot(snapshot)
        .page(
            PageQuery {
                page: 3,
                page_size: 2,
                sort: None,
            },
            DisplayStamp::default(),
        )
        .await
        .expect("page 3")
        .rows;
    assert_eq!(rows.len(), 1);

    let again = eng
        .snapshot(snapshot)
        .page(
            PageQuery {
                page: 3,
                page_size: 2,
                sort: None,
            },
            DisplayStamp::default(),
        )
        .await
        .expect("page 3 again")
        .rows;
    assert_eq!(rows[0][0].text, again[0][0].text);

    let sorted = eng
        .snapshot(snapshot)
        .page(
            PageQuery {
                page: 1,
                page_size: 2,
                sort: Some(("column1".into(), false)),
            },
            DisplayStamp::default(),
        )
        .await
        .expect("sorted page")
        .rows;
    assert_eq!(sorted[0][0].text, "5");
    assert_eq!(sorted[1][0].text, "4");
}

#[tokio::test]
async fn a_rerun_makes_a_new_snapshot_and_retires_the_old() {
    let eng = engine();

    let first = eng
        .ws(ws(1))
        .query(tag(1), SQL.into(), 2)
        .await
        .expect("run 1")
        .output;
    let old = first.snapshot.unwrap();

    let second = eng
        .ws(ws(1))
        .query(tag(2), SQL.into(), 2)
        .await
        .expect("run 2")
        .output;
    let new = second.snapshot.unwrap();

    assert_ne!(
        old, new,
        "identical SQL still materializes a distinct snapshot"
    );
    eng.snapshot(new)
        .page(
            PageQuery {
                page: 1,
                page_size: 2,
                sort: None,
            },
            DisplayStamp::default(),
        )
        .await
        .expect("new snapshot readable");
    eng.snapshot(old)
        .page(
            PageQuery {
                page: 1,
                page_size: 2,
                sort: None,
            },
            DisplayStamp::default(),
        )
        .await
        .expect_err("old snapshot is retired on re-run dispatch");
}

#[tokio::test]
async fn workspaces_are_independent_and_cleanup_retires() {
    let eng = engine();

    let a = eng
        .ws(ws(1))
        .query(tag(1), SQL.into(), 2)
        .await
        .expect("ws 1")
        .output;
    let b = eng
        .ws(ws(2))
        .query(tag(2), SQL.into(), 2)
        .await
        .expect("ws 2")
        .output;
    let (snap_a, snap_b) = (a.snapshot.unwrap(), b.snapshot.unwrap());
    assert_ne!(snap_a, snap_b);

    eng.ws(ws(1)).cleanup();
    eng.snapshot(snap_a)
        .page(
            PageQuery {
                page: 1,
                page_size: 2,
                sort: None,
            },
            DisplayStamp::default(),
        )
        .await
        .expect_err("ws 1 retired");
    eng.snapshot(snap_b)
        .page(
            PageQuery {
                page: 1,
                page_size: 2,
                sort: None,
            },
            DisplayStamp::default(),
        )
        .await
        .expect("ws 2 untouched");
}

#[tokio::test]
async fn an_empty_result_materializes_nothing() {
    let eng = engine();
    let output = eng
        .ws(ws(1))
        .query(tag(1), format!("{SQL} WHERE column1 > 100"), 2)
        .await
        .expect("empty run")
        .output;
    assert_eq!(output.total, 0);
    assert!(
        output.snapshot.is_none(),
        "no rows → no snapshot, nothing to page"
    );
    assert_eq!(output.columns.len(), 3, "schema still delivered");
}

#[tokio::test]
async fn a_failed_run_errors_and_keeps_nothing() {
    let eng = engine();
    eng.ws(ws(1))
        .query(tag(1), "SELECT * FROM no_such_table".into(), 2)
        .await
        .expect_err("unknown table fails");
    eng.ws(ws(1))
        .query(tag(2), "CREATE TABLE t (a INT)".into(), 2)
        .await
        .expect_err("DDL is blocked");
}

#[tokio::test]
async fn cancel_is_scoped_to_the_dispatched_run() {
    let eng = engine();
    assert!(eng.ws(ws(1)).cancel(tag(99)).is_none());

    let output = eng
        .ws(ws(1))
        .query(tag(1), SQL.into(), 2)
        .await
        .expect("run")
        .output;
    assert!(output.snapshot.is_some());
    assert!(eng.ws(ws(1)).cancel(tag(1)).is_none());
}

/// **A page renders through the stamp it was handed, not through the engine's live config.**
///
/// Two reads of the same page differing only in the stamp come back differently, and the one
/// handed the run's own stamp is unmoved by the config change between them.
#[tokio::test]
async fn a_page_renders_through_the_stamp_it_was_handed() {
    let eng = engine();
    let run = eng
        .ws(ws(1))
        .query(tag(1), "SELECT CAST(NULL AS INT) AS n".into(), 10)
        .await
        .expect("run");
    let snapshot = run.output.snapshot.expect("a row is a snapshot");
    assert_eq!(run.output.rows[0][0].text, "NULL", "the built-in default");

    let overrides: BTreeMap<String, String> =
        [("datafusion.format.null".to_string(), "∅".to_string())]
            .into_iter()
            .collect();
    assert_eq!(eng.set_config(overrides.clone()), ConfigOutcome::Applied);

    let window = || PageQuery {
        page: 1,
        page_size: 10,
        sort: None,
    };
    let unmoved = eng
        .snapshot(snapshot)
        .page(window(), run.display.clone())
        .await
        .expect("page under the run's own stamp");
    assert_eq!(
        unmoved.rows[0][0].text, "NULL",
        "the config moved under the engine; the stamp did not"
    );

    let restamped = eng
        .snapshot(snapshot)
        .page(window(), display_subset(&overrides))
        .await
        .expect("page under the new stamp");
    assert_eq!(restamped.rows[0][0].text, "∅");
    assert!(
        restamped.rows[0][0].null,
        "only the text changes — the flag the grid dims on stays"
    );

    assert_ne!(
        run.display,
        eng.display(),
        "and the two stamps differ, which is what tells a surface its page 1 has aged"
    );
}
