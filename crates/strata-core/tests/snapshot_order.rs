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

use std::sync::Arc;

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
    let eng = Arc::new(Engine::new(Default::default()));
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

/// **A query that already has its own partitioned window keeps it, and the ordinal stays
/// global.** The hazard this was written for is gone by construction: the ordinal used to be a
/// `row_number() OVER ()` appended to the *plan*, where it could in principle be merged into the
/// user's window spec and number within their partitions. It is now numbered by the writer from
/// the count already spooled (`docs/SNAPSHOT_SPEC.md` §9), so nothing can merge it into anything.
/// The assertion is kept because it still pins the half that matters and always did — a user's
/// own window is evaluated as they wrote it, and the ordinal is one global sequence over the
/// stream the writer consumes.
#[tokio::test]
async fn a_users_partitioned_window_survives_beneath_the_ordinal() {
    let eng = Engine::new(Default::default());
    let snap = snapshot(
        &eng,
        "SELECT i, md5(i::text) AS h, \
                row_number() OVER (PARTITION BY i % 4 ORDER BY i) AS rn \
         FROM generate_series(1, 3000000) t(i) ORDER BY i",
    )
    .await;

    for page in [1usize, 15_000] {
        let (first, _) = eng.fetch_page(snap, page, 100, None).await.expect("page");
        let (again, _) = eng
            .fetch_page(snap, page, 100, None)
            .await
            .expect("page again");
        let i = ints(&first, 0);
        assert_eq!(i, ints(&again, 0), "page {page} reads the same twice");
        let start = ((page - 1) * 100 + 1) as i64;
        let expected: Vec<i64> = (start..start + 100).collect();
        assert_eq!(i, expected, "page {page} is in the user's ORDER BY");
        let rn = ints(&first, 2);
        for (i, rn) in i.iter().zip(rn) {
            assert_eq!(rn, (i - 1) / 4 + 1, "user rn for i={i}");
        }
    }
}

/// The same, with **no outer ORDER BY**: the result order is whatever the engine produced,
/// the snapshot freezes it, and the user's window values stay row-consistent — checkable
/// per row regardless of arrival order.
#[tokio::test]
async fn an_unordered_partitioned_window_stays_row_consistent() {
    let eng = Engine::new(Default::default());
    let (out, _) = eng
        .query(
            WsId(1),
            RunTag(1),
            "SELECT i, md5(i::text) AS h, \
                    row_number() OVER (PARTITION BY i % 4 ORDER BY i) AS rn \
             FROM generate_series(1, 3000000) t(i)"
                .into(),
            100,
        )
        .await
        .expect("run");
    let snap = out.snapshot.expect("snapshot");

    let spooled: Vec<String> = out.rows.iter().map(|r| r[0].text.clone()).collect();
    let (fetched, _) = eng.fetch_page(snap, 1, 100, None).await.expect("page 1");
    let read: Vec<String> = fetched.iter().map(|r| r[0].text.clone()).collect();
    assert_eq!(read, spooled, "page 1 is the page the run delivered");

    for page in [1usize, 20_000] {
        let (rows, _) = eng.fetch_page(snap, page, 100, None).await.expect("page");
        let (again, _) = eng
            .fetch_page(snap, page, 100, None)
            .await
            .expect("page again");
        assert_eq!(ints(&rows, 0), ints(&again, 0), "page {page} stable");
        for (i, rn) in ints(&rows, 0).iter().zip(ints(&rows, 2)) {
            assert_eq!(rn, (i - 1) / 4 + 1, "user rn for i={i}");
        }
    }
}

/// The compound case: the user's own window is **aliased to the ordinal's name**. The
/// bookkeeping escalates around it; the user's column keeps its values and the pages keep
/// the user's order.
#[tokio::test]
async fn a_user_window_aliased_like_the_ordinal_keeps_its_values() {
    let eng = Engine::new(Default::default());
    let snap = snapshot(
        &eng,
        "SELECT i, md5(i::text) AS h, \
                row_number() OVER (ORDER BY i DESC) AS __strata_ord \
         FROM generate_series(1, 200000) t(i) ORDER BY i",
    )
    .await;

    let (rows, batch) = eng.fetch_page(snap, 2, 100, None).await.expect("page 2");
    let schema = batch.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(
        names,
        vec!["i", "h", "__strata_ord"],
        "the user's column survives under its own name"
    );
    let i = ints(&rows, 0);
    let expected: Vec<i64> = (101..=200).collect();
    assert_eq!(i, expected, "pages follow the user's ORDER BY i");
    for (i, rn) in i.iter().zip(ints(&rows, 2)) {
        assert_eq!(rn, 200_000 - i + 1, "user's __strata_ord for i={i}");
    }
}

/// **A typed `EXPLAIN` still runs, and spools without an ordinal.** The reason is no longer a
/// constraint: DataFusion requires Explain/Analyze at the plan root, which is what made the old
/// plan-level window fail them outright, and the writer-side ordinal has no such problem. The
/// exclusion is now a choice — a handful of plan rows cannot reach the nondeterminism the ordinal
/// exists for — so what this pins is the statement class the managed-DDL policy promises the
/// editor can run, and that its pages read back.
#[tokio::test]
async fn explain_runs_and_pages_without_an_ordinal() {
    let eng = Engine::new(Default::default());
    for sql in ["EXPLAIN SELECT 1", "EXPLAIN ANALYZE SELECT 1"] {
        let (out, _) = eng
            .query(WsId(1), RunTag(1), sql.into(), 10)
            .await
            .unwrap_or_else(|e| panic!("{sql} must run: {e}"));
        assert!(out.total > 0, "{sql} returns plan rows");
        let snap = out.snapshot.expect("plan rows materialize");
        let (rows, batch) = eng.fetch_page(snap, 1, 10, None).await.expect("page");
        assert!(!rows.is_empty());
        let schema = batch.schema();
        assert!(
            schema.fields().iter().all(|f| f.name() != "__strata_ord"),
            "no ordinal leaks from an ordinal-less snapshot"
        );
    }
}

/// **A result with duplicate column names still reads.** The registered table resolves
/// columns by name, so an ordinal appended after two same-named columns mis-mapped every
/// later read; such a result now spools ordinal-less and reads exactly as it did at base.
#[tokio::test]
async fn duplicate_named_columns_still_read() {
    let eng = Engine::new(Default::default());
    let (out, _) = eng
        .query(
            WsId(1),
            RunTag(1),
            "SELECT a.i, b.i FROM generate_series(1, 3) AS a(i) \
             JOIN generate_series(1, 3) AS b(i) ON a.i = b.i"
                .into(),
            10,
        )
        .await
        .expect("run");
    assert_eq!(out.total, 3);
    let names: Vec<&str> = out.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["i", "i"],
        "both columns survive in the result schema"
    );
    let snap = out.snapshot.expect("snapshot");
    let (rows, _) = eng
        .fetch_page(snap, 1, 10, None)
        .await
        .expect("a duplicate-named result must stay readable");
    assert_eq!(rows.len(), 3);
}

/// **A partitioned export is as ordinal-free as a flat one** — the task's contract named
/// both, and the partitioned path additionally crosses `PARTITIONED BY` and
/// `keep_partition_by_columns`.
#[tokio::test]
async fn a_partitioned_export_never_writes_the_ordinal() {
    let eng = Arc::new(Engine::new(Default::default()));
    let (out, _) = eng
        .query(
            WsId(1),
            RunTag(1),
            "SELECT i % 2 AS p, i FROM generate_series(1, 10) t(i)".into(),
            10,
        )
        .await
        .expect("run");
    let snap = out.snapshot.expect("snapshot");

    let dir = std::env::temp_dir().join(format!("strata_ord_part_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    eng.export(
        snap,
        ExportSpec {
            path: dir.to_string_lossy().into_owned(),
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
            partition: Partition {
                columns: vec!["p".into()],
                keep_columns: false,
            },
        },
    )
    .await
    .expect("partitioned export");

    let mut files = 0;
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("walk").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files += 1;
                let text = std::fs::read_to_string(&path).expect("read part file");
                assert!(
                    !text.contains("__strata_ord"),
                    "{path:?} must not carry bookkeeping"
                );
            }
        }
    }
    assert!(
        files >= 2,
        "the export actually partitioned ({files} files)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
