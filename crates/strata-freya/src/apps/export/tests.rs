//! **End-to-end: what the window is showing → what lands on disk.**
//!
//! The other two suites each cover half of this. `model`'s tests stop at the [`ExportSpec`],
//! and `strata-core`'s `engine_export.rs` starts from one. Neither would catch the half that
//! actually breaks in use: a draft that *looks* right producing a spec that writes the wrong
//! file — a delimiter that never reaches the writer, a scope that ignores the sort, a
//! partition toggle that doesn't gate.
//!
//! So each test here drives the real path a press of Export takes — [`ExportDraft`] →
//! [`ExportDraft::spec`] → [`Engine::export`] → a file in a temp directory → read back and
//! asserted. The only thing stubbed is the file dialog, which contributes the path.
//!
//! `block_on` stands in for the UI executor: the engine's `JoinHandle`s are executor-agnostic,
//! so this is the same await the footer performs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
use futures::executor::block_on;
use strata_core::engine::export::Compression;
use strata_core::engine::{column_info, Engine, RunTag, WsId};
use strata_model::{Cell, ColumnInfo, SnapshotId};

use super::model::{
    CodecChoice, Edit, ExportDraft, ExportTarget, FormatId, NullChoice, ScopeChoice,
};

/// Five rows, unsorted on `id`, with a NULL and a comma-bearing value so quoting and null
/// handling are observable in the file.
///
/// `region` carries the NULL; `tier` deliberately does not. Partitioning is refused on a column
/// containing NULLs, so the partition tests need a column that has none — and keeping both here
/// means the refusal is testable from this side too.
const SQL: &str = "SELECT * FROM (VALUES \
     (3, 'charlie', 'apac', 'gold'), \
     (1, 'alpha', 'emea', 'silver'), \
     (5, 'echo, jr', 'amer', 'gold'), \
     (2, 'bravo', NULL, 'silver'), \
     (4, 'delta', 'emea', 'gold')) AS t(id, name, region, tier)";

/// A scratch directory per test, removed by the caller on the way out.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strata-export-e2e-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn col(name: &str, dtype: DataType) -> ColumnInfo {
    column_info(&Field::new(name, dtype, true))
}

/// Run `SQL` and build the [`ExportTarget`] the results pane would hand the window — the real
/// snapshot, the real schema and row count, the real page-1 rows.
fn open_on_a_result(engine: &Arc<Engine>, sort: Option<(String, bool)>) -> ExportTarget {
    let (output, _) = block_on(engine.query(WsId(1), RunTag(1), SQL.into(), 100)).expect("run");
    ExportTarget {
        snapshot: output.snapshot.expect("a non-empty result snapshots"),
        columns: output.columns.clone(),
        total: output.total,
        sort,
        page: 1,
        page_size: 100,
        label: "cross-file join".into(),
        sample: output.rows,
    }
}

/// Press Export: build the spec for `path` exactly as the footer does, and write it.
fn export_to(
    engine: &Arc<Engine>,
    draft: &ExportDraft,
    target: &ExportTarget,
    path: &Path,
) -> usize {
    let spec = draft
        .spec(target, path.to_string_lossy().into_owned())
        .expect("the draft builds a spec");
    let (_, rows) = block_on(engine.export(target.snapshot, spec)).expect("export");
    rows
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .expect("read the exported file")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn the_default_draft_writes_a_plain_csv() {
    let dir = scratch("default-csv");
    let out = dir.join("out.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let rows = export_to(&engine, &ExportDraft::default(), &target, &out);
    assert_eq!(rows, 5);

    let lines = lines(&out);
    assert_eq!(lines[0], "id,name,region,tier", "header on by default");
    assert_eq!(lines.len(), 6);
    assert!(lines.iter().any(|l| l == "2,bravo,,silver"), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("\"echo, jr\"")),
        "{lines:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn editing_the_delimiter_in_the_window_reaches_the_file() {
    let dir = scratch("delimiter");
    let out = dir.join("out.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let mut draft = ExportDraft::default();
    draft.apply(Edit::CsvDelimiter("|".into()));

    export_to(&engine, &draft, &target, &out);
    let lines = lines(&out);
    assert_eq!(lines[0], "id|name|region|tier");
    assert!(
        lines.iter().any(|l| l == "5|echo, jr|amer|gold"),
        "{lines:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_tab_delimiter_typed_as_an_escape_lands_as_a_real_tab() {
    let dir = scratch("tab-delimiter");
    let out = dir.join("out.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let mut draft = ExportDraft::default();
    draft.apply(Edit::CsvDelimiter("\\t".into()));

    export_to(&engine, &draft, &target, &out);
    let lines = lines(&out);
    assert_eq!(
        lines[0], "id\tname\tregion\ttier",
        "one tab byte, not a backslash-t"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn turning_the_header_off_removes_the_column_row() {
    let dir = scratch("no-header");
    let out = dir.join("out.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let mut draft = ExportDraft::default();
    draft.apply(Edit::CsvHeader(false));

    export_to(&engine, &draft, &target, &out);
    let lines = lines(&out);
    assert_eq!(lines.len(), 5, "five rows, no header: {lines:?}");
    assert!(!lines[0].contains("name"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_chosen_null_text_is_what_a_null_cell_becomes() {
    let dir = scratch("null-text");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    for (choice, custom, expected) in [
        (NullChoice::Null, None, "2,bravo,NULL,silver"),
        (NullChoice::NaN, None, "2,bravo,NaN,silver"),
        (NullChoice::Custom, Some("\\N"), "2,bravo,\\N,silver"),
    ] {
        let out = dir.join(format!("{choice:?}.csv"));
        let mut draft = ExportDraft::default();
        draft.apply(Edit::CsvNull(choice));
        if let Some(text) = custom {
            draft.apply(Edit::CsvNullCustom(text.into()));
        }
        export_to(&engine, &draft, &target, &out);
        let lines = lines(&out);
        assert!(
            lines.iter().any(|l| l == expected),
            "{choice:?} → {expected:?}, got {lines:?}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_grids_sort_is_the_order_in_the_file() {
    let dir = scratch("sorted");
    let out = dir.join("out.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, Some(("id".into(), false)));

    export_to(&engine, &ExportDraft::default(), &target, &out);
    let ids: Vec<String> = lines(&out)
        .into_iter()
        .skip(1)
        .map(|l| l.split(',').next().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["5", "4", "3", "2", "1"]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn this_page_writes_only_the_page_the_grid_is_showing() {
    let dir = scratch("page-scope");
    let out = dir.join("page2.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let mut target = open_on_a_result(&engine, Some(("id".into(), true)));
    target.page = 2;
    target.page_size = 2;

    let mut draft = ExportDraft::default();
    draft.apply(Edit::Scope(ScopeChoice::Page));

    let rows = export_to(&engine, &draft, &target, &out);
    assert_eq!(rows, 2);
    let ids: Vec<String> = lines(&out)
        .into_iter()
        .skip(1)
        .map(|l| l.split(',').next().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["3", "4"], "page 2 of the ascending order");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn switching_the_format_card_changes_what_is_written() {
    let dir = scratch("formats");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let json = dir.join("out.json");
    let mut draft = ExportDraft {
        format: FormatId::Json,
        ..Default::default()
    };
    export_to(&engine, &draft, &target, &json);
    let text = fs::read_to_string(&json).expect("read json");
    assert_eq!(text.lines().count(), 5);
    assert!(text.starts_with('{'), "not an array: {text}");

    let parquet = dir.join("out.parquet");
    draft.format = FormatId::Parquet;
    export_to(&engine, &draft, &target, &parquet);
    let bytes = fs::read(&parquet).expect("read parquet");
    assert_eq!(&bytes[..4], b"PAR1");
    assert_eq!(&bytes[bytes.len() - 4..], b"PAR1");

    let arrow = dir.join("out.arrow");
    draft.format = FormatId::Arrow;
    export_to(&engine, &draft, &target, &arrow);
    assert!(arrow.metadata().expect("arrow file").len() > 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn every_parquet_codec_the_window_offers_writes_a_readable_file() {
    let dir = scratch("parquet-codecs");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    for codec in [
        CodecChoice::Zstd,
        CodecChoice::Snappy,
        CodecChoice::Gzip,
        CodecChoice::Brotli,
        CodecChoice::Lz4,
        CodecChoice::Uncompressed,
    ] {
        let out = dir.join(format!("{codec:?}.parquet"));
        let mut draft = ExportDraft {
            format: FormatId::Parquet,
            ..Default::default()
        };
        draft.apply(Edit::PqCodec(codec));
        let rows = export_to(&engine, &draft, &target, &out);
        assert_eq!(rows, 5, "{codec:?}");
        let bytes = fs::read(&out).expect("read parquet");
        assert_eq!(&bytes[bytes.len() - 4..], b"PAR1", "{codec:?} truncated");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_compressed_csv_lands_under_the_suffix_the_window_suggested() {
    let dir = scratch("compressed");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let mut draft = ExportDraft::default();
    draft.apply(Edit::CsvCompression(Compression::Gzip));
    let suggested = draft.suggested_name(&target);
    assert_eq!(suggested, "cross-file_join.csv.gz");

    let out = dir.join(&suggested);
    export_to(&engine, &draft, &target, &out);
    assert!(out.exists(), "written under the name the user was offered");
    let bytes = fs::read(&out).expect("read");
    assert_eq!(&bytes[..2], &[0x1f, 0x8b], "gzip magic");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_partition_toggle_is_what_decides_between_a_file_and_a_tree() {
    let dir = scratch("partition-gate");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let flat = dir.join("flat.csv");
    let mut draft = ExportDraft::default();
    draft.partition.columns = vec!["tier".into()];
    export_to(&engine, &draft, &target, &flat);
    assert!(flat.is_file(), "toggle off → a file, not a directory");

    let tree = dir.join("tree");
    draft.partition.enabled = true;
    export_to(&engine, &draft, &target, &tree);
    assert!(tree.is_dir(), "toggle on → a directory");
    let mut levels: Vec<String> = fs::read_dir(&tree)
        .expect("tree")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    levels.sort();
    assert_eq!(
        levels,
        vec!["tier=gold", "tier=silver"],
        "one per distinct value"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_selected_order_is_the_directory_nesting_order() {
    let dir = scratch("partition-order");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let mut draft = ExportDraft::default();
    draft.partition.enabled = true;
    draft.partition.columns = vec!["tier".into(), "id".into()];

    let tree = dir.join("tree");
    export_to(&engine, &draft, &target, &tree);

    let mut inner: Vec<String> = fs::read_dir(tree.join("tier=gold"))
        .expect("outer level is tier")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    inner.sort();
    assert!(
        inner.iter().all(|name| name.starts_with("id=")),
        "inner level is id: {inner:?}"
    );
    assert_eq!(inner, vec!["id=3", "id=4", "id=5"], "the gold rows");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn keeping_partition_columns_is_visible_in_the_written_rows() {
    let dir = scratch("partition-keep");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let read_header = |tree: &Path| -> String {
        let leaf = fs::read_dir(tree.join("tier=gold"))
            .expect("leaf")
            .filter_map(Result::ok)
            .next()
            .expect("a part file");
        fs::read_to_string(leaf.path())
            .expect("part")
            .lines()
            .next()
            .expect("header")
            .to_string()
    };

    let mut draft = ExportDraft::default();
    draft.partition.enabled = true;
    draft.partition.columns = vec!["tier".into()];

    let dropped = dir.join("dropped");
    export_to(&engine, &draft, &target, &dropped);
    assert!(
        !read_header(&dropped).contains("tier"),
        "off by default: the column lives in the directory name"
    );

    let kept = dir.join("kept");
    draft.partition.keep_columns = true;
    export_to(&engine, &draft, &target, &kept);
    assert!(
        read_header(&kept).contains("region"),
        "kept inside the files"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// A partition column containing NULLs is refused outright, because DataFusion would file
/// those rows under a neighbouring value and they would read back wrong. The engine answers it
/// from the snapshot's parquet footer, so this costs no scan.
#[test]
fn partitioning_on_a_column_with_nulls_is_refused() {
    let dir = scratch("partition-null");
    let out = dir.join("tree");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let mut draft = ExportDraft::default();
    draft.partition.enabled = true;
    draft.partition.columns = vec!["region".into()];

    let spec = draft
        .spec(&target, out.to_string_lossy().into_owned())
        .expect("the draft itself is fine");
    let err = block_on(engine.export(target.snapshot, spec)).expect_err("region has a NULL");
    assert!(err.contains("Can't partition by 'region'"), "{err}");
    assert!(!out.exists(), "nothing written");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_draft_the_engine_would_choke_on_is_refused_before_any_file_is_made() {
    let dir = scratch("bad-draft");
    let out = dir.join("out.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let mut draft = ExportDraft::default();
    draft.apply(Edit::CsvDelimiter("||".into()));
    let err = draft
        .spec(&target, out.to_string_lossy().into_owned())
        .expect_err("refused");
    assert!(err.contains("single character"), "{err}");
    assert!(!out.exists(), "nothing was created");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_preview_matches_the_file_the_same_draft_writes() {
    let dir = scratch("preview-truth");
    let out = dir.join("out.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let mut draft = ExportDraft::default();
    draft.apply(Edit::CsvDelimiter(";".into()));
    draft.apply(Edit::CsvNull(NullChoice::Null));

    let preview = super::preview::build(&draft, &target);
    export_to(&engine, &draft, &target, &out);
    let written = lines(&out);

    for (n, line) in preview.lines().enumerate() {
        assert_eq!(line, written[n], "preview line {n} disagrees with the file");
    }

    let _ = fs::remove_dir_all(&dir);
}

/// The window's own promise: it exports the run it was opened on, whatever the tab does next.
#[test]
fn a_rerun_behind_the_window_does_not_change_what_it_writes() {
    let dir = scratch("pinned-rerun");
    let out = dir.join("out.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);

    let pin = engine.pin_snapshot(target.snapshot);

    block_on(engine.query(
        WsId(1),
        RunTag(2),
        "SELECT 99 AS id, 'later' AS name, 'zzz' AS region".into(),
        100,
    ))
    .expect("re-run");

    let rows = export_to(&engine, &ExportDraft::default(), &target, &out);
    assert_eq!(rows, 5);
    let written = lines(&out);
    assert_eq!(written[0], "id,name,region,tier");
    assert!(
        !written.iter().any(|l| l.contains("later")),
        "the newer run's rows are not in this file: {written:?}"
    );

    drop(pin);
    let _ = fs::remove_dir_all(&dir);
}

/// A guard on the honesty rule: the preview may only show rows the result actually has.
#[test]
fn the_preview_only_ever_shows_rows_the_run_returned() {
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);
    let preview = super::preview::build(&ExportDraft::default(), &target);

    let names: Vec<&str> = target
        .sample
        .iter()
        .map(|row| row[1].text.as_str())
        .collect();
    for line in preview.lines().skip(1) {
        let name = line.split(',').nth(1).unwrap_or_default();
        assert!(
            names.contains(&name) || name.starts_with('"'),
            "preview row {line:?} names a value the run never returned"
        );
    }
}

/// `Cell` is only constructed by the engine here, so this pins the assumption the preview
/// makes about a NULL cell rather than trusting it.
#[test]
fn a_null_cell_arrives_flagged_rather_than_as_the_text_null() {
    let engine = Arc::new(Engine::new(Default::default()));
    let target = open_on_a_result(&engine, None);
    let null_cell: &Cell = target
        .sample
        .iter()
        .find(|row| row[0].text == "2")
        .map(|row| &row[2])
        .expect("the row with a NULL region");
    assert!(null_cell.null, "flagged, not stringly-typed");
}

/// The snapshot id is the window's whole identity; a target built on one that no longer exists
/// must fail loudly rather than writing an empty file.
#[test]
fn exporting_a_snapshot_that_is_gone_writes_nothing_and_says_why() {
    let dir = scratch("gone");
    let out = dir.join("out.csv");
    let engine = Arc::new(Engine::new(Default::default()));
    let mut target = open_on_a_result(&engine, None);
    target.snapshot = SnapshotId(9999);

    let spec = ExportDraft::default()
        .spec(&target, out.to_string_lossy().into_owned())
        .expect("the draft is fine; the snapshot is not");
    let err = block_on(engine.export(target.snapshot, spec)).expect_err("no such snapshot");
    assert!(err.contains("No results to export"), "{err}");
    assert!(!out.exists(), "nothing was created");

    let _ = fs::remove_dir_all(&dir);
}

/// Only scalar columns reach the AVAILABLE pane, and this is why: the ones it hides are the
/// ones a directory name can't sensibly carry.
#[test]
fn the_partitionable_columns_are_the_ones_a_directory_name_can_hold() {
    let engine = Arc::new(Engine::new(Default::default()));
    let mut target = open_on_a_result(&engine, None);
    target.columns.push(col(
        "created_at",
        DataType::Timestamp(TimeUnit::Millisecond, None),
    ));
    target.columns.push(col(
        "payload",
        DataType::Struct(vec![Field::new("a", DataType::Utf8, true)].into()),
    ));

    let offered: Vec<&str> = target
        .partitionable()
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(offered, vec!["id", "name", "region", "tier"]);
}
