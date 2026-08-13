//! The export round-trip against the real engine (P4-10 / D6): a Run materializes a
//! snapshot, `Engine::export` writes it, and what lands on disk is read back and checked.
//!
//! The point of driving this end-to-end rather than asserting on the generated SQL is that
//! **every option key here has to be one DataFusion actually accepts**. A wrong key is not a
//! silent no-op — `COPY` fails the statement — so a green test is proof the whole per-format
//! surface the export window offers is real.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use strata_core::engine::export::{
    Codec, Compression, Csv, ExportSpec, Format, Json, Parquet, Partition, Scope, Statistics,
    WriterVersion,
};
use strata_core::engine::{Engine, RunOutcome, RunTag, WsId};

/// Five rows, three columns, unsorted on `column1` so a sorted export is observable.
const SQL: &str = "SELECT * FROM (VALUES (3, 'c', true), (1, 'a', false), (5, 'e', true), (2, 'b', false), (4, 'd', true)) AS t";

/// An `Arc`, because `Engine::export` takes `&Arc<Self>`: the pin and the in-flight count it
/// claims are handed to the spawned write and have to outlive the call that started it.
fn engine() -> Arc<Engine> {
    Arc::new(Engine::new(Default::default()))
}

/// A unique scratch directory per test, removed on the way out by the caller.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strata-export-test-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn csv() -> Csv {
    Csv {
        header: true,
        delimiter: ',',
        null_value: String::new(),
        quote: '"',
        escape: None,
        double_quote: true,
        compression: Compression::None,
    }
}

fn spec(path: &Path, format: Format) -> ExportSpec {
    ExportSpec {
        path: path.to_string_lossy().into_owned(),
        scope: Scope::All,
        sort: None,
        format,
        partition: Partition::default(),
    }
}

/// Run `SQL` and hand back the engine plus its snapshot.
async fn snapshot(eng: &Engine) -> strata_model::SnapshotId {
    let (output, _) = eng
        .query(WsId(1), RunTag(1), SQL.into(), 2)
        .await
        .expect("run");
    output.snapshot.expect("a non-empty result snapshots")
}

#[tokio::test]
async fn csv_writes_every_option_the_window_offers() {
    let dir = scratch("csv-options");
    let out = dir.join("out.csv");
    let eng = engine();
    let snap = snapshot(&eng).await;

    let (path, rows) = eng
        .export(
            snap,
            spec(
                &out,
                Format::Csv(Csv {
                    header: true,
                    delimiter: ';',
                    null_value: "\\N".into(),
                    quote: '\'',
                    escape: Some('\\'),
                    double_quote: false,
                    compression: Compression::None,
                }),
            ),
        )
        .await
        .expect("csv export");

    assert_eq!(path, out.to_string_lossy());
    assert_eq!(rows, 5, "COPY reports the rows it wrote");

    let text = fs::read_to_string(&out).expect("read back");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 6, "header + 5 rows");
    assert!(
        lines[0].contains(';'),
        "the chosen delimiter is honoured: {:?}",
        lines[0]
    );
    assert!(
        !lines[0].contains(','),
        "the default delimiter is gone: {:?}",
        lines[0]
    );

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_header_can_be_turned_off() {
    let dir = scratch("csv-noheader");
    let out = dir.join("out.csv");
    let eng = engine();
    let snap = snapshot(&eng).await;

    eng.export(
        snap,
        spec(
            &out,
            Format::Csv(Csv {
                header: false,
                ..csv()
            }),
        ),
    )
    .await
    .expect("headerless export");

    let text = fs::read_to_string(&out).expect("read back");
    assert_eq!(text.lines().count(), 5, "5 rows, no header line");
    assert!(
        !text.contains("column1"),
        "no column-name row: {:?}",
        text.lines().next()
    );

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_active_sort_is_what_lands_on_disk() {
    let dir = scratch("csv-sorted");
    let out = dir.join("sorted.csv");
    let eng = engine();
    let snap = snapshot(&eng).await;

    let mut s = spec(&out, Format::Csv(csv()));
    s.sort = Some(("column1".into(), false));
    eng.export(snap, s).await.expect("sorted export");

    let text = fs::read_to_string(&out).expect("read back");
    let first_col: Vec<&str> = text
        .lines()
        .skip(1)
        .map(|l| l.split(',').next().unwrap())
        .collect();
    assert_eq!(
        first_col,
        vec!["5", "4", "3", "2", "1"],
        "descending, over the whole snapshot"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_page_scope_writes_only_that_window_of_the_sorted_order() {
    let dir = scratch("csv-page");
    let out = dir.join("page2.csv");
    let eng = engine();
    let snap = snapshot(&eng).await;

    let mut s = spec(&out, Format::Csv(csv()));
    s.sort = Some(("column1".into(), true));
    s.scope = Scope::Page {
        page: 2,
        page_size: 2,
    };
    let (_, rows) = eng.export(snap, s).await.expect("page export");
    assert_eq!(rows, 2);

    let text = fs::read_to_string(&out).expect("read back");
    let first_col: Vec<&str> = text
        .lines()
        .skip(1)
        .map(|l| l.split(',').next().unwrap())
        .collect();
    assert_eq!(
        first_col,
        vec!["3", "4"],
        "page 2 of the ascending order, not an arbitrary slice"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn parquet_accepts_the_full_tuning_surface() {
    let dir = scratch("parquet-options");
    let out = dir.join("out.parquet");
    let eng = engine();
    let snap = snapshot(&eng).await;

    let (_, rows) = eng
        .export(
            snap,
            spec(
                &out,
                Format::Parquet(Parquet {
                    compression: Codec::Zstd(9),
                    statistics: Statistics::Chunk,
                    max_row_group_size: 131_072,
                    writer_version: WriterVersion::V2,
                    dictionary: false,
                }),
            ),
        )
        .await
        .expect("parquet export");
    assert_eq!(rows, 5);

    let bytes = fs::read(&out).expect("read back");
    assert!(bytes.len() > 8, "not an empty file");
    assert_eq!(&bytes[..4], b"PAR1", "parquet magic at the head");
    assert_eq!(&bytes[bytes.len() - 4..], b"PAR1", "and a written footer");

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn every_parquet_codec_is_one_datafusion_knows() {
    let dir = scratch("parquet-codecs");
    let eng = engine();
    let snap = snapshot(&eng).await;

    for (n, codec) in [
        Codec::Uncompressed,
        Codec::Snappy,
        Codec::Lz4,
        Codec::Gzip(6),
        Codec::Brotli(5),
        Codec::Zstd(3),
    ]
    .into_iter()
    .enumerate()
    {
        let out = dir.join(format!("codec-{n}.parquet"));
        eng.export(
            snap,
            spec(
                &out,
                Format::Parquet(Parquet {
                    compression: codec,
                    statistics: Statistics::Page,
                    max_row_group_size: 1_048_576,
                    writer_version: WriterVersion::V1,
                    dictionary: true,
                }),
            ),
        )
        .await
        .unwrap_or_else(|e| panic!("codec {codec:?} rejected: {e}"));
    }

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn json_writes_ndjson_and_arrow_takes_no_options() {
    let dir = scratch("json-arrow");
    let eng = engine();
    let snap = snapshot(&eng).await;

    let json = dir.join("out.json");
    eng.export(
        snap,
        spec(
            &json,
            Format::Json(Json {
                compression: Compression::None,
            }),
        ),
    )
    .await
    .expect("json export");
    let text = fs::read_to_string(&json).expect("read back");
    assert_eq!(text.lines().count(), 5, "one object per line (NDJSON)");
    assert!(text.starts_with('{'), "not a wrapping array: {text:?}");

    let arrow = dir.join("out.arrow");
    eng.export(snap, spec(&arrow, Format::Arrow))
        .await
        .expect("arrow export");
    assert!(arrow.exists());

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn compression_is_accepted_for_csv_and_json() {
    let dir = scratch("compression");
    let eng = engine();
    let snap = snapshot(&eng).await;

    for (n, compression) in [
        Compression::Gzip,
        Compression::Zstd,
        Compression::Bzip2,
        Compression::Xz,
    ]
    .into_iter()
    .enumerate()
    {
        let out = dir.join(format!("out-{n}.csv{}", compression.extension()));
        eng.export(
            snap,
            spec(
                &out,
                Format::Csv(Csv {
                    compression,
                    ..csv()
                }),
            ),
        )
        .await
        .unwrap_or_else(|e| panic!("csv {compression:?} rejected: {e}"));
        assert!(out.exists());
    }

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn partitioning_writes_a_hive_tree_and_drops_the_columns_by_default() {
    let dir = scratch("partition");
    let out = dir.join("tree");
    let eng = engine();
    let snap = snapshot(&eng).await;

    let mut s = spec(&out, Format::Csv(csv()));
    s.partition = Partition {
        columns: vec!["column3".into()],
        keep_columns: false,
    };
    eng.export(snap, s).await.expect("partitioned export");

    let mut levels: Vec<String> = fs::read_dir(&out)
        .expect("tree root")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    levels.sort();
    assert_eq!(
        levels,
        vec!["column3=false", "column3=true"],
        "one directory level per distinct value, key=value"
    );

    let leaf = fs::read_dir(out.join("column3=true"))
        .expect("leaf dir")
        .filter_map(Result::ok)
        .next()
        .expect("a part file");
    let text = fs::read_to_string(leaf.path()).expect("read part");
    let header = text.lines().next().expect("header");
    assert!(
        !header.contains("column3"),
        "partition column removed from file contents: {header:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn keeping_partition_columns_puts_them_back_in_the_files() {
    let dir = scratch("partition-keep");
    let out = dir.join("tree");
    let eng = engine();
    let snap = snapshot(&eng).await;

    let mut s = spec(&out, Format::Csv(csv()));
    s.partition = Partition {
        columns: vec!["column3".into()],
        keep_columns: true,
    };
    eng.export(snap, s).await.expect("partitioned export");

    let leaf = fs::read_dir(out.join("column3=true"))
        .expect("leaf dir")
        .filter_map(Result::ok)
        .next()
        .expect("a part file");
    let text = fs::read_to_string(leaf.path()).expect("read part");
    let header = text.lines().next().expect("header");
    assert!(
        header.contains("column3"),
        "kept inside the files too: {header:?}"
    );

    assert_eq!(
        keep_partition_by_columns(&eng).await,
        "false",
        "the engine's own setting is untouched by an export"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// The engine's live value for `datafusion.execution.keep_partition_by_columns`, read the way a
/// user would — a typed `SHOW`, through the same `Engine::run` the editor uses.
async fn keep_partition_by_columns(eng: &Engine) -> String {
    let RunOutcome::Rows(output, _) = eng
        .run(
            WsId(9),
            RunTag(9),
            "SHOW datafusion.execution.keep_partition_by_columns".into(),
            10,
        )
        .await
        .expect("show")
    else {
        panic!("SHOW did not return rows");
    };
    output
        .rows
        .first()
        .and_then(|row| row.last())
        .map(|cell| cell.text.clone())
        .expect("a value row")
}

#[tokio::test]
async fn a_partition_column_that_isnt_a_bare_word_fails_before_planning() {
    let dir = scratch("partition-bad");
    let out = dir.join("tree");
    let eng = engine();
    let snap = snapshot(&eng).await;

    let mut s = spec(&out, Format::Csv(csv()));
    s.partition = Partition {
        columns: vec!["order date".into()],
        keep_columns: false,
    };
    let err = s;
    let err = eng.export(snap, err).await.expect_err("bad partition name");
    assert!(err.contains("single plain word"), "{err}");

    let _ = fs::remove_dir_all(&dir);
}

/// The reason [`Engine::pin_snapshot`] exists: an export window is opened *on a result*, the
/// user goes back and re-runs the query, and the export must still write the rows that were on
/// screen when they asked for them.
#[tokio::test]
async fn a_pin_keeps_a_snapshot_exportable_across_a_rerun() {
    let dir = scratch("pin-rerun");
    let out = dir.join("out.csv");
    let eng = engine();

    let (first, _) = eng
        .query(WsId(1), RunTag(1), SQL.into(), 2)
        .await
        .expect("run 1");
    let snap = first.snapshot.expect("snapshot");

    let pin = eng.pin_snapshot(snap);

    let (second, _) = eng
        .query(WsId(1), RunTag(2), SQL.into(), 2)
        .await
        .expect("run 2");
    assert_ne!(second.snapshot.unwrap(), snap, "a genuinely new snapshot");

    eng.fetch_page(snap, 1, 2, None)
        .await
        .expect("the pinned snapshot survived the re-run");
    let (_, rows) = eng
        .export(snap, spec(&out, Format::Csv(csv())))
        .await
        .expect("export of the pinned snapshot");
    assert_eq!(rows, 5, "the rows that were on screen, not the new run's");

    drop(pin);
    eng.fetch_page(snap, 1, 2, None)
        .await
        .expect_err("retired once the last hold released");

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_unpinned_snapshot_still_retires_at_the_rerun() {
    let eng = engine();
    let (first, _) = eng
        .query(WsId(1), RunTag(1), SQL.into(), 2)
        .await
        .expect("run 1");
    let snap = first.snapshot.expect("snapshot");

    eng.query(WsId(1), RunTag(2), SQL.into(), 2)
        .await
        .expect("run 2");

    eng.fetch_page(snap, 1, 2, None)
        .await
        .expect_err("no pin, so §4's immediate retire still applies");
}

#[tokio::test]
async fn the_last_hold_is_the_one_that_retires() {
    let eng = engine();
    let (first, _) = eng
        .query(WsId(1), RunTag(1), SQL.into(), 2)
        .await
        .expect("run 1");
    let snap = first.snapshot.expect("snapshot");

    let one = eng.pin_snapshot(snap);
    let two = eng.pin_snapshot(snap);
    assert_eq!(one.snapshot(), snap);

    eng.query(WsId(1), RunTag(2), SQL.into(), 2)
        .await
        .expect("run 2");

    drop(one);
    eng.fetch_page(snap, 1, 2, None)
        .await
        .expect("one hold left, still alive");

    drop(two);
    eng.fetch_page(snap, 1, 2, None)
        .await
        .expect_err("last hold released, deferred retire lands");
}

/// Closing the tab is the other retire path a pin has to survive.
#[tokio::test]
async fn a_pin_survives_the_owning_tab_closing() {
    let eng = engine();
    let (first, _) = eng
        .query(WsId(1), RunTag(1), SQL.into(), 2)
        .await
        .expect("run");
    let snap = first.snapshot.expect("snapshot");

    let pin = eng.pin_snapshot(snap);
    eng.cleanup_ws(WsId(1));
    eng.fetch_page(snap, 1, 2, None)
        .await
        .expect("still readable while held");

    drop(pin);
    eng.fetch_page(snap, 1, 2, None)
        .await
        .expect_err("retired with the last hold");
}

/// A pin taken and released with nothing else happening must not retire anything — the
/// deferral only fires for a retire that actually arrived.
#[tokio::test]
async fn releasing_a_pin_alone_retires_nothing() {
    let eng = engine();
    let (first, _) = eng
        .query(WsId(1), RunTag(1), SQL.into(), 2)
        .await
        .expect("run");
    let snap = first.snapshot.expect("snapshot");

    drop(eng.pin_snapshot(snap));
    eng.fetch_page(snap, 1, 2, None)
        .await
        .expect("still the workspace's current snapshot");
}

/// **A dropped export future hands its bookkeeping to the write, and gets it back when the write
/// ends.** `Engine::export` awaits, and its caller is a UI task dropped when the export window
/// closes — while the write itself is spawned and detaches.
///
/// Both halves are asserted here because each one was wrong at some point. A guard released
/// *after* the await would leave the engine permanently claiming work in flight and holding a
/// snapshot nothing can retire; a guard living in the **caller's future** — which is what shipped
/// until the pre-release review — releases the pin the moment the window closes, so a re-run in
/// the owning tab retires the snapshot the `COPY` is still streaming and the user's file ends
/// truncated with nothing to report it. The hold therefore rides on the spawned write: claimed
/// for exactly as long as there is a write, and released on every path that write can end by.
#[tokio::test]
async fn a_dropped_export_holds_its_pin_until_the_write_ends() {
    let dir = scratch("export-cancelled");
    let out = dir.join("out.csv");
    let eng = engine();

    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    eng.watch_inflight(flag.clone());

    let (first, _) = eng
        .query(WsId(1), RunTag(1), SQL.into(), 2)
        .await
        .expect("run");
    let snap = first.snapshot.expect("snapshot");

    {
        use std::future::Future;

        let fut = eng.export(snap, spec(&out, Format::Csv(csv())));
        futures::pin_mut!(fut);
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        let _ = fut.as_mut().poll(&mut cx);
    }

    let mut settled = false;
    for _ in 0..400 {
        if !flag.load(std::sync::atomic::Ordering::Relaxed) {
            settled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        settled,
        "the hold must be released when the write ends, or the engine claims work forever"
    );

    let written = fs::read_to_string(&out).expect("the dropped caller still left a file");
    assert_eq!(
        written.lines().count(),
        6,
        "a detached write must still finish its file: {written:?}"
    );

    eng.query(WsId(1), RunTag(2), SQL.into(), 2)
        .await
        .expect("re-run");
    eng.fetch_page(snap, 1, 2, None)
        .await
        .expect_err("no pin left holding it open");

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn exporting_without_a_snapshot_says_so_plainly() {
    let dir = scratch("no-snapshot");
    let out = dir.join("out.csv");
    let eng = engine();

    let err = eng
        .export(
            strata_model::SnapshotId(999),
            spec(&out, Format::Csv(csv())),
        )
        .await
        .expect_err("no such snapshot");
    assert!(err.contains("No results to export"), "{err}");

    let _ = fs::remove_dir_all(&dir);
}

/// **A partition column containing NULLs is refused, not written.**
///
/// DataFusion 54 has no Hive `__HIVE_DEFAULT_PARTITION__` for a NULL: it files the row under a
/// neighbouring value's directory, so it reads back claiming a value it never had. Since nothing
/// on our side can steer that, the export declines instead.
///
/// The check costs nothing and scans nothing: `query::materialize` streams every batch to write
/// the snapshot and sums `Array::null_count` as it goes, so the exact per-column count is already
/// in hand (`query::SnapshotStats`). A typed `COPY`, which has no snapshot behind it, reaches the
/// same refusal by counting — see `ddl::copy`.
#[tokio::test]
async fn a_partition_column_with_nulls_is_refused() {
    let dir = scratch("partition-null");
    let out = dir.join("tree");
    let eng = engine();

    let (output, _) = eng
        .query(
            WsId(1),
            RunTag(1),
            "SELECT * FROM (VALUES (1, 'emea'), (2, NULL), (3, 'amer')) AS t(id, region)".into(),
            100,
        )
        .await
        .expect("run");
    let snap = output.snapshot.expect("snapshot");

    let mut s = spec(&out, Format::Csv(csv()));
    s.partition = Partition {
        columns: vec!["region".into()],
        keep_columns: false,
    };
    let err = eng
        .export(snap, s)
        .await
        .expect_err("region contains a NULL");
    assert!(err.contains("Can't partition by 'region'"), "{err}");
    assert!(err.contains("NULL"), "{err}");
    assert!(
        !out.exists(),
        "and nothing is written — the refusal comes before the COPY"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// The other side of the gate: a column with no NULLs partitions as before.
#[tokio::test]
async fn a_partition_column_without_nulls_is_allowed() {
    let dir = scratch("partition-no-null");
    let out = dir.join("tree");
    let eng = engine();

    let (output, _) = eng
        .query(
            WsId(1),
            RunTag(1),
            "SELECT * FROM (VALUES (1, 'emea'), (2, 'apac'), (3, 'amer')) AS t(id, region)".into(),
            100,
        )
        .await
        .expect("run");
    let snap = output.snapshot.expect("snapshot");

    let mut s = spec(&out, Format::Csv(csv()));
    s.partition = Partition {
        columns: vec!["region".into()],
        keep_columns: false,
    };
    let (_, rows) = eng.export(snap, s).await.expect("no NULLs, so it writes");
    assert_eq!(rows, 3);

    let mut levels: Vec<String> = fs::read_dir(&out)
        .expect("tree root")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    levels.sort();
    assert_eq!(
        levels,
        vec!["region=amer", "region=apac", "region=emea"],
        "one directory per distinct value"
    );

    let _ = fs::remove_dir_all(&dir);
}
