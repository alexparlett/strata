//! The query round-trip against the real engine (P2-01 acceptance): a Run materializes
//! an immutable snapshot; page reads target that snapshot (stable under re-reads); a
//! re-run makes a *new* snapshot and retires the old; cleanup retires everything.
//!
//! Driven straight through the `Engine` facade — no UI framework involved, exactly as a
//! freya-query capability calls it. The test runtime awaits the engine's `JoinHandle`s
//! across runtimes, which is the same executor-agnostic await the Freya executor does.

use strata_core::engine::{Engine, RunTag, WsId};

/// Five rows, three columns, unsorted on `column1` so the sort read is observable.
const SQL: &str = "SELECT * FROM (VALUES (3, 'c', true), (1, 'a', false), (5, 'e', true), (2, 'b', false), (4, 'd', true)) AS t";

fn engine() -> Engine {
    Engine::new(Default::default())
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

    let (output, batch) = eng.query(ws(1), tag(1), SQL.into(), 2).await.expect("run");
    let snapshot = output.snapshot.expect("a non-empty result materializes a snapshot");
    assert_eq!(output.total, 5);
    assert_eq!(output.page, 1);
    assert_eq!(output.rows.len(), 2, "page 1 rides with the run");
    assert_eq!(output.columns.len(), 3);
    assert_eq!(batch.num_rows(), 2);

    // Page reads target the snapshot: bounded windows, exact tail.
    let (rows, _) = eng.fetch_page(snapshot, 2, 2, None).await.expect("page 2");
    assert_eq!(rows.len(), 2);
    let (rows, _) = eng.fetch_page(snapshot, 3, 2, None).await.expect("page 3");
    assert_eq!(rows.len(), 1);

    // Same read again — the snapshot is immutable, so the same key yields the same rows
    // (this is what makes the UI-side cache keyed by (snapshot, page, …) sound).
    let (again, _) = eng.fetch_page(snapshot, 3, 2, None).await.expect("page 3 again");
    assert_eq!(rows[0][0].text, again[0][0].text);

    // A sorted read orders over the WHOLE snapshot before the page window.
    let (sorted, _) = eng
        .fetch_page(snapshot, 1, 2, Some(("column1".into(), false)))
        .await
        .expect("sorted page");
    assert_eq!(sorted[0][0].text, "5");
    assert_eq!(sorted[1][0].text, "4");
}

#[tokio::test]
async fn a_rerun_makes_a_new_snapshot_and_retires_the_old() {
    let eng = engine();

    let (first, _) = eng.query(ws(1), tag(1), SQL.into(), 2).await.expect("run 1");
    let old = first.snapshot.unwrap();

    let (second, _) = eng.query(ws(1), tag(2), SQL.into(), 2).await.expect("run 2");
    let new = second.snapshot.unwrap();

    assert_ne!(old, new, "identical SQL still materializes a distinct snapshot");
    eng.fetch_page(new, 1, 2, None).await.expect("new snapshot readable");
    eng.fetch_page(old, 1, 2, None)
        .await
        .expect_err("old snapshot is retired on re-run dispatch");
}

#[tokio::test]
async fn workspaces_are_independent_and_cleanup_retires() {
    let eng = engine();

    let (a, _) = eng.query(ws(1), tag(1), SQL.into(), 2).await.expect("ws 1");
    let (b, _) = eng.query(ws(2), tag(2), SQL.into(), 2).await.expect("ws 2");
    let (snap_a, snap_b) = (a.snapshot.unwrap(), b.snapshot.unwrap());
    assert_ne!(snap_a, snap_b);

    // Closing one tab retires only its snapshot.
    eng.cleanup_ws(ws(1));
    eng.fetch_page(snap_a, 1, 2, None).await.expect_err("ws 1 retired");
    eng.fetch_page(snap_b, 1, 2, None).await.expect("ws 2 untouched");
}

#[tokio::test]
async fn an_empty_result_materializes_nothing() {
    let eng = engine();
    let (output, _) = eng
        .query(ws(1), tag(1), format!("{SQL} WHERE column1 > 100"), 2)
        .await
        .expect("empty run");
    assert_eq!(output.total, 0);
    assert!(output.snapshot.is_none(), "no rows → no snapshot, nothing to page");
    assert_eq!(output.columns.len(), 3, "schema still delivered");
}

#[tokio::test]
async fn a_failed_run_errors_and_keeps_nothing() {
    let eng = engine();
    eng.query(ws(1), tag(1), "SELECT * FROM no_such_table".into(), 2)
        .await
        .expect_err("unknown table fails");
    // DDL/DML are blocked by policy at the engine boundary.
    eng.query(ws(1), tag(2), "CREATE TABLE t (a INT)".into(), 2)
        .await
        .expect_err("DDL is blocked");
}

#[tokio::test]
async fn cancel_is_scoped_to_the_dispatched_run() {
    let eng = engine();
    // Nothing in flight → a stale cancel is a no-op.
    assert!(eng.cancel(ws(1), tag(99)).is_none());

    let (output, _) = eng.query(ws(1), tag(1), SQL.into(), 2).await.expect("run");
    assert!(output.snapshot.is_some());
    // The run settled, so its tag no longer cancels anything.
    assert!(eng.cancel(ws(1), tag(1)).is_none());
}
