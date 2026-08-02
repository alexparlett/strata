//! The ordinal makes every snapshot read deterministic (`docs/SNAPSHOT_SPEC.md` §9) —
//! regression for the measured failure: above `repartition_file_min_size` (10 MB) a bare
//! `LIMIT/OFFSET` read had no order, so the **same page re-read returned different rows**
//! (page 1 of a 3M-row snapshot arrived starting at row 1 843 201 on one read and row 101 on
//! the next), and a 200k-row snapshot with a text column paged stably but starting at row
//! 57 345 — pages 2+ disagreeing with the spooled page 1. The page cache then froze whichever
//! answer a read got.
//!
//! The fixtures here are deliberately **over** the split threshold — the failure was invisible
//! below it, which is why no test ever saw it.

use strata_core::engine::export::{Compression, Csv, ExportSpec, Format, Partition, Scope};
use strata_core::engine::{Engine, RunTag, WsId};
use strata_model::{Cell, SnapshotId};

/// Wide enough (an md5 column) that 3M rows cross 10 MB many times over. `ORDER BY i` makes
/// the *result* order the generation order, so a page's contents are predictable — without
/// it the query has no defined order and the snapshot's job is only to freeze whichever one
/// the engine produced (see `an_unordered_query_pages_the_order_the_spool_froze`).
const BIG: &str = "SELECT i, md5(i::text) AS h FROM generate_series(1, 3000000) t(i) ORDER BY i";

async fn snapshot(eng: &Engine, sql: &str) -> SnapshotId {
    let (out, _) = eng
        .query(WsId(1), RunTag(1), sql.into(), 10)
        .await
        .expect("run");
    out.snapshot.expect("snapshot")
}

fn ints(rows: &[Vec<Cell>], col: usize) -> Vec<i64> {
    rows.iter().map(|r| r[col].text.parse().unwrap()).collect()
}

/// **Unsorted pages are stable and in result order**, wherever in the snapshot they fall,
/// however often they are re-read.
#[tokio::test]
async fn pages_over_the_split_threshold_are_stable_and_in_result_order() {
    let eng = Engine::new(Default::default());
    let snap = snapshot(&eng, BIG).await;

    let page_size = 100usize;
    for page in [1usize, 2, 15_000, 30_000] {
        let (first, _) = eng
            .fetch_page(snap, page, page_size, None)
            .await
            .expect("page");
        let (again, _) = eng
            .fetch_page(snap, page, page_size, None)
            .await
            .expect("page again");
        let a = ints(&first, 0);
        let b = ints(&again, 0);
        assert_eq!(a, b, "page {page} must read the same twice");
        let start = ((page - 1) * page_size + 1) as i64;
        let expected: Vec<i64> = (start..start + page_size as i64).collect();
        assert_eq!(a, expected, "page {page} must hold rows {start}..");
    }
}

/// **A user sort with duplicate keys is stable across page windows** — the ordinal is the
/// tie-break, so page 2 of a sorted read continues page 1 exactly instead of re-rolling the
/// tie.
#[tokio::test]
async fn a_sorted_read_is_stable_across_page_windows_on_ties() {
    let eng = Engine::new(Default::default());
    // Every row ties on `k`, so the sort is *all* tie-break — and the source is ordered, so
    // the tie-break's answer is predictable.
    let snap = snapshot(
        &eng,
        "SELECT 0 AS k, i, md5(i::text) AS h FROM generate_series(1, 3000000) t(i) ORDER BY i",
    )
    .await;

    let sort = Some(("k".to_string(), true));
    let (one, _) = eng
        .fetch_page(snap, 1, 100, sort.clone())
        .await
        .expect("sorted page 1");
    let (two, _) = eng
        .fetch_page(snap, 2, 100, sort.clone())
        .await
        .expect("sorted page 2");
    let (two_again, _) = eng
        .fetch_page(snap, 2, 100, sort)
        .await
        .expect("sorted page 2 again");
    assert_eq!(ints(&two, 1), ints(&two_again, 1), "re-reads agree");
    let mut both = ints(&one, 1);
    both.extend(ints(&two, 1));
    let expected: Vec<i64> = (1..=200).collect();
    assert_eq!(
        both, expected,
        "page 2 continues page 1 — the tie is broken by result order, not by the scan"
    );
}

/// **An unordered query has no order to promise — the snapshot freezes the one the engine
/// produced**, and every read then agrees with it: `fetch_page`'s page 1 is exactly the page
/// the run delivered from the spool, and re-reads agree with each other. (This is where the
/// old failure lived: the spooled page 1 said one thing and `fetch_page` said another, so
/// rows duplicated and vanished as the user paged.)
#[tokio::test]
async fn an_unordered_query_pages_the_order_the_spool_froze() {
    let eng = Engine::new(Default::default());
    let (out, _) = eng
        .query(
            WsId(1),
            RunTag(1),
            "SELECT i, md5(i::text) AS h FROM generate_series(1, 3000000) t(i)".into(),
            100,
        )
        .await
        .expect("run");
    let snap = out.snapshot.expect("snapshot");

    let spooled: Vec<String> = out.rows.iter().map(|r| r[0].text.clone()).collect();
    let (fetched, _) = eng.fetch_page(snap, 1, 100, None).await.expect("page 1");
    let read: Vec<String> = fetched.iter().map(|r| r[0].text.clone()).collect();
    assert_eq!(
        read, spooled,
        "fetch_page's page 1 is the page the run delivered"
    );

    let (p2a, _) = eng.fetch_page(snap, 2, 100, None).await.expect("page 2");
    let (p2b, _) = eng
        .fetch_page(snap, 2, 100, None)
        .await
        .expect("page 2 again");
    assert_eq!(ints(&p2a, 0), ints(&p2b, 0), "re-reads agree");
    assert!(
        !ints(&p2a, 0).iter().any(|v| read.contains(&v.to_string())),
        "page 2 shares no row with page 1"
    );
}

/// **The ordinal never reaches a user-visible surface**: not the result schema, not a page
/// batch, not an exported file.
#[tokio::test]
async fn the_ordinal_is_bookkeeping_and_never_leaks() {
    let eng = Engine::new(Default::default());
    let (out, page1) = eng
        .query(
            WsId(1),
            RunTag(1),
            "SELECT i, i * 2 AS d FROM generate_series(1, 5) t(i)".into(),
            10,
        )
        .await
        .expect("run");
    let snap = out.snapshot.expect("snapshot");

    let names: Vec<&str> = out.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["i", "d"], "the result schema is the user's");
    assert_eq!(page1.schema().fields().len(), 2, "page 1 batch too");

    let (_, batch) = eng.fetch_page(snap, 1, 10, None).await.expect("page");
    let schema = batch.schema();
    let fetched: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(fetched, vec!["i", "d"], "a fetched page projects it away");

    let dir = std::env::temp_dir().join(format!("strata_ord_export_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("out.csv").to_string_lossy().into_owned();
    eng.export(
        snap,
        ExportSpec {
            path: path.clone(),
            format: Format::Csv(Csv {
                header: true,
                delimiter: ',',
                null_value: String::new(),
                quote: '"',
                escape: None,
                double_quote: true,
                compression: Compression::None,
            }),
            scope: Scope::All,
            sort: None,
            partition: Partition::default(),
        },
    )
    .await
    .expect("export");
    let written = std::fs::read_to_string(&path).expect("exported file");
    let header = written.lines().next().expect("header");
    assert_eq!(
        header, "i,d",
        "a COPY writes the user's columns and nothing else"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A result that already has a `__strata_ord` column keeps it — the bookkeeping name
/// escalates around the user's, and the user's survives every surface with its data intact.
#[tokio::test]
async fn a_user_column_named_like_the_ordinal_survives() {
    let eng = Engine::new(Default::default());
    let (out, _) = eng
        .query(
            WsId(1),
            RunTag(1),
            "SELECT i * 10 AS __strata_ord FROM generate_series(1, 3) t(i)".into(),
            10,
        )
        .await
        .expect("run");
    let snap = out.snapshot.expect("snapshot");
    assert_eq!(out.columns[0].name, "__strata_ord");

    let (rows, batch) = eng.fetch_page(snap, 1, 10, None).await.expect("page");
    assert_eq!(batch.schema().fields().len(), 1, "only the user's column");
    assert_eq!(
        ints(&rows, 0),
        vec![10, 20, 30],
        "the user's data, in result order"
    );
}
