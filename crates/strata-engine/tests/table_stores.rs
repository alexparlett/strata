//! `InternalTableStore` at the facade: the statements a frontend dispatches work whatever the
//! tables are kept in, and a [`MemTableStore`] engine performs them with nothing on disk.
//!
//! The two halves are one test on purpose. "Writes no spool" is only worth asserting beside a
//! store that demonstrably does: the control engine runs exactly the same statements and fills
//! its project's `.strata/tables/`, and the mem engine then runs them again while its own
//! project directory stays exactly as it was.

use std::path::{Path, PathBuf};
use std::{env, fs, process};

use strata_engine::{Engine, MemTableStore, RunOutcome, RunTag, StatementReport, WsId};

/// A scratch project folder of our own, per engine.
fn scratch(tag: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("strata_table_stores_{}_{tag}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

async fn statement(eng: &Engine, sql: &str) -> StatementReport {
    match eng
        .ws(WsId(1))
        .run(RunTag(1), sql.into(), 10)
        .await
        .expect(sql)
    {
        RunOutcome::Statement(report) => report,
        RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
    }
}

async fn read_n(eng: &Engine, sql: &str) -> Vec<String> {
    let RunOutcome::Rows(output, _) = eng
        .ws(WsId(2))
        .run(RunTag(2), sql.into(), 100)
        .await
        .expect(sql)
    else {
        panic!("{sql} did not return rows");
    };
    output
        .rows
        .into_iter()
        .map(|row| row.into_iter().next().expect("one column").text)
        .collect()
}

/// The whole internal-table life a frontend drives — create, read, append, read the union,
/// drop — asserted on the answers rather than on the calls succeeding.
async fn statements(eng: &Engine) {
    let created = statement(
        eng,
        "CREATE TABLE t AS SELECT * FROM (VALUES (1), (2)) AS v(n)",
    )
    .await;
    assert_eq!(created.message, "Table 't' created, 2 rows");
    assert!(eng.catalog().is_internal("t"));

    let inserted = statement(eng, "INSERT INTO t VALUES (3)").await;
    assert_eq!(inserted.message, "Inserted 1 row into 't'");
    assert_eq!(
        read_n(eng, "SELECT n FROM t ORDER BY n").await,
        vec!["1", "2", "3"],
        "a scan through the registered provider sees the appended unit"
    );

    let dropped = statement(eng, "DROP TABLE t").await;
    assert_eq!(dropped.message, "Table 't' and its data were deleted");
    assert!(!eng.catalog().is_internal("t"));
}

/// The tables directory a project's default store would fill.
fn tables_dir(root: &Path) -> PathBuf {
    root.join(".strata").join("tables")
}

#[tokio::test]
async fn a_mem_table_store_engine_runs_the_statements_with_nothing_on_disk() {
    let control_root = scratch("control");
    let control = Engine::builder().with_data_dir(&control_root).build();
    statements(&control).await;
    assert!(
        tables_dir(&control_root).is_dir(),
        "the default store worked under the project: the directory outlives the dropped table"
    );

    let mem_root = scratch("mem");
    let eng = Engine::builder()
        .with_data_dir(&mem_root)
        .with_table_store(MemTableStore::new())
        .build();
    statements(&eng).await;
    assert!(
        !tables_dir(&mem_root).exists(),
        "and the mem engine served the same statements without touching the project"
    );

    let _ = fs::remove_dir_all(&control_root);
    let _ = fs::remove_dir_all(&mem_root);
}
