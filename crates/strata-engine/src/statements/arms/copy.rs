//! **Typed `COPY … TO`** — DataFusion's own write, composed into a [`CopyJob`] like every other.
//! `docs/STATEMENTS_SPEC.md` §6.4.
//!
//! Nothing about the *write* is ours. What this arm adds is one refusal, about a statement that
//! would otherwise succeed and produce something wrong: **a partition identifier has to be one
//! bare word.** DataFusion's COPY parser renders each with `Ident::to_string()` and the planner
//! looks it up by that string, so a quoted `PARTITIONED BY ("order date")` fails about a column
//! the user never named. It is asked here rather than in the shared write path because it can
//! only be asked before planning — by the time a job exists the planner has already resolved
//! those names and thrown its own message.
//!
//! The owned-storage fence and the NULL-partition refusal are [`run_copy`]'s, which is what makes
//! them the same two refusals the Export window and the agent answer to. A typed COPY's evidence
//! for the second is [`NullEvidence::Count`]: it reads live tables and has no snapshot's free
//! counts, so it pays for one extra scan — a pre-flight, not a lock.
//!
//! The reserved-name half is the router's: a `__snap_` relation anywhere in the source refuses
//! with `Fault::ReservedName`, which keeps `COPY (SELECT * FROM __snap_3) TO …` from writing
//! `__strata_ord` into a user's file.

use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::SQLOptions;
use datafusion::sql::parser::Statement as DFStatement;

use crate::export::partition_columns_are_bare_words;
use crate::policy::Principal;
use crate::statements::copy_job::{run_copy, CopyJob, NullEvidence};
use crate::statements::ctx::StmtCtx;
use crate::statements::pipeline::Qualified;
use crate::statements::report::StatementOutcome;
use crate::statements::StmtKind;
use strata_core::util::plural;

/// Write a typed `COPY … TO`'s source to disk and report the rows it wrote.
///
/// **The plan that was gated is the plan that runs.** The statement is planned once — planning a
/// `COPY` executes nothing, since execution lives only in `execute_logical_plan` — and the node's
/// own five values become the [`CopyJob`] that is then driven. Re-dispatching the user's text
/// through `ctx.sql` would judge one plan and execute another, which is the rule the `INSERT` arm
/// already keeps.
pub async fn copy_to(
    cx: &StmtCtx,
    _who: &Principal,
    stmt: &Qualified,
) -> Result<StatementOutcome, String> {
    let ctx = &cx.ctx;
    let DFStatement::CopyTo(copy) = &**stmt else {
        return Err(format!(
            "{} did not parse as a copy",
            StmtKind::Copy.label()
        ));
    };
    partition_columns_are_bare_words(&copy.partitioned_by, ctx)?;

    let plan = ctx
        .state()
        .statement_to_plan((**stmt).clone())
        .await
        .map_err(|e| e.to_string())?;

    SQLOptions::new()
        .with_allow_dml(true)
        .with_allow_ddl(false)
        .with_allow_statements(false)
        .verify_plan(&plan)
        .map_err(|e| e.to_string())?;

    let LogicalPlan::Copy(copying) = plan else {
        return Err(format!("{} did not plan as a copy", StmtKind::Copy.label()));
    };
    let target = copying.output_url.clone();
    let job = CopyJob {
        input: copying.input,
        target: copying.output_url,
        file_type: copying.file_type,
        options: copying.options,
        partition_by: copying.partition_by,
    };
    let rows = run_copy(
        ctx,
        job,
        &cx.owned,
        NullEvidence::Count,
        StmtKind::Copy.label(),
    )
    .await?;

    Ok(StatementOutcome {
        message: format!("Exported {} to '{target}'", plural(rows, "row")),
        count: Some(rows as u64),
        effect: None,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::{env, process};

    use crate::formats::fake::TestFormat;
    use crate::statements::Fault;
    use crate::{Engine, RunOutcome, RunRows, RunTag, StatementReport, WsId};
    use strata_core::project::{save_defs, ProjectDefs};

    /// Run one statement and take its report — anything else is a test that asked the wrong
    /// question.
    async fn statement(eng: &Engine, sql: &str) -> Result<StatementReport, String> {
        match eng
            .ws(WsId(1))
            .run(RunTag(1), sql.into(), 10)
            .await
            .map_err(|e| e.to_string())?
        {
            RunOutcome::Statement(report) => Ok(report),
            RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
        }
    }

    /// The values a query returns, as text.
    async fn read(eng: &Engine, sql: &str) -> Vec<Vec<String>> {
        let RunOutcome::Rows(RunRows { output, .. }) = eng
            .ws(WsId(2))
            .run(RunTag(2), sql.into(), 100)
            .await
            .expect("query")
        else {
            panic!("{sql} did not return rows");
        };
        output
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(|cell| cell.text).collect())
            .collect()
    }

    /// **Every format the export window writes, written from the editor instead.** Driven end to
    /// end rather than asserted against generated SQL, because the claim is that a typed COPY is
    /// DataFusion's own statement: what proves it is that the file exists, holds the rows, and
    /// reads back through the engine.
    #[tokio::test]
    async fn an_unpartitioned_copy_writes_the_file_and_counts_its_rows() {
        let root = scratch("flat");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);

        for (format, name) in [
            ("CSV", "out.csv"),
            ("PARQUET", "out.parquet"),
            ("JSON", "out.json"),
            ("ARROW", "out.arrow"),
        ] {
            let out = root.join(name);
            let report = statement(
                &eng,
                &format!(
                    "COPY (SELECT * FROM (VALUES (1, 'a'), (2, 'b'), (3, 'c')) AS t(n, s)) \
                     TO '{}' STORED AS {format}",
                    out.display()
                ),
            )
            .await
            .expect("exported");
            assert_eq!(
                report.message,
                format!("Exported 3 rows to '{}'", out.display())
            );
            assert_eq!(report.count, Some(3));
            assert_eq!(report.effect, None, "a COPY changes no catalog state");
            assert!(out.exists(), "{name} was written");
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// **An embedder's format writes through DataFusion's own COPY.** A registrant that brings a
    /// writer has it registered on the session under its own name, so `STORED AS <name>` plans and
    /// writes with nothing here knowing the format exists — and the file it wrote reads back
    /// through the same registrant.
    #[tokio::test]
    async fn a_registered_format_that_brought_a_writer_is_copied_to() {
        let root = scratch("extension");
        let eng = Engine::builder().with_format(TestFormat).build();
        eng.set_data_dir(&root);
        let out = root.join("out.testfmt");

        let report = statement(
            &eng,
            &format!(
                "COPY (SELECT * FROM (VALUES (1, 'a'), (2, 'b')) AS t(n, s)) TO '{}' \
                 STORED AS testfmt",
                out.display()
            ),
        )
        .await
        .expect("exported");
        assert_eq!(report.count, Some(2));
        assert!(out.exists(), "the registrant's writer wrote the file");

        statement(
            &eng,
            &format!(
                "CREATE EXTERNAL TABLE back STORED AS testfmt LOCATION '{}'",
                out.display()
            ),
        )
        .await
        .expect("registered");
        assert_eq!(
            read(&eng, "SELECT s FROM back ORDER BY n").await,
            [["a"], ["b"]]
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// **The reserved namespace, from the router.** A snapshot table carries `__strata_ord`, and
    /// nothing but Strata's own readers may ever see it — so a typed COPY that names one is
    /// refused before it can write bookkeeping into a user's file.
    #[tokio::test]
    async fn a_copy_out_of_a_snapshot_is_refused_and_writes_nothing() {
        let root = scratch("reserved");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        let out = root.join("leak.csv");

        for source in ["(SELECT * FROM __snap_1)", "__snap_1"] {
            assert_eq!(
                statement(
                    &eng,
                    &format!("COPY {source} TO '{}' STORED AS CSV", out.display())
                )
                .await
                .expect_err("refused"),
                Fault::ReservedName.message()
            );
        }
        assert!(!out.exists(), "a refusal writes nothing");
        let _ = fs::remove_dir_all(&root);
    }

    /// **A NULL in a partition column is refused, and nothing is written.**
    ///
    /// DataFusion 54 files such a row under a neighbouring value's directory, so the output would
    /// read back claiming a value it never had. The Export window answers this from the snapshot
    /// write pass's counts; here there is no snapshot, so the gate counts — and the sentence the
    /// user reads is the same one, from the same function.
    #[tokio::test]
    async fn a_partitioned_copy_over_a_null_refuses_and_writes_the_tree_once_it_cannot() {
        let root = scratch("partition");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        statement(
            &eng,
            "CREATE TABLE sales AS SELECT * FROM \
             (VALUES (1, 'emea'), (2, NULL), (3, 'amer')) AS t(id, region)",
        )
        .await
        .expect("created");

        let out = root.join("tree");
        let err = statement(
            &eng,
            &format!(
                "COPY sales TO '{}' STORED AS CSV PARTITIONED BY (region)",
                out.display()
            ),
        )
        .await
        .expect_err("region contains a NULL");
        assert!(err.contains("Can't partition by 'region'"), "{err}");
        assert!(err.contains("NULL"), "{err}");
        assert!(!out.exists(), "the refusal comes before the COPY");

        let report = statement(
            &eng,
            &format!(
                "COPY (SELECT * FROM sales WHERE region IS NOT NULL) TO '{}' \
                 STORED AS CSV PARTITIONED BY (region)",
                out.display()
            ),
        )
        .await
        .expect("exported");
        assert_eq!(report.count, Some(2));
        assert_eq!(levels(&out), vec!["region=amer", "region=emea"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// A partition column with no NULLs at all needs no filter, and the gate stays out of the way
    /// of an unpartitioned COPY over the very same NULL-bearing column.
    #[tokio::test]
    async fn the_gate_only_looks_at_partition_columns() {
        let root = scratch("gate-scope");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        statement(
            &eng,
            "CREATE TABLE t AS SELECT * FROM \
             (VALUES (1, 'emea'), (2, NULL), (3, 'amer')) AS t(id, region)",
        )
        .await
        .expect("created");

        let flat = root.join("flat.csv");
        statement(
            &eng,
            &format!("COPY t TO '{}' STORED AS CSV", flat.display()),
        )
        .await
        .expect("a NULL is only a problem in a directory name");
        assert!(flat.exists());

        let tree = root.join("tree");
        statement(
            &eng,
            &format!(
                "COPY t TO '{}' STORED AS CSV PARTITIONED BY (id)",
                tree.display()
            ),
        )
        .await
        .expect("id has no NULLs");
        assert_eq!(levels(&tree), vec!["id=1", "id=2", "id=3"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// **The gate must not refuse a statement DataFusion would run.** A column named twice in
    /// `PARTITIONED BY` plans without complaint, but two identical `count` expressions collide in
    /// the pre-flight aggregate's own output schema — so counting per entry rather than per
    /// distinct name failed the statement with a schema error about `count(t.region)`, a query
    /// the user never wrote.
    #[tokio::test]
    async fn a_column_partitioned_by_twice_is_counted_once() {
        let root = scratch("repeat");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        statement(&eng, "CREATE TABLE t AS SELECT 1 AS id, 'emea' AS region")
            .await
            .expect("created");

        let out = root.join("tree");
        statement(
            &eng,
            &format!(
                "COPY t TO '{}' STORED AS CSV PARTITIONED BY (region, region)",
                out.display()
            ),
        )
        .await
        .expect("the same question twice is still one question");

        let err = statement(
            &eng,
            &format!(
                "COPY (SELECT * FROM (VALUES (1, 'emea'), (2, NULL)) AS v(id, region)) TO '{}' \
                 STORED AS CSV PARTITIONED BY (region, region)",
                root.join("tree2").display()
            ),
        )
        .await
        .expect_err("region contains a NULL");
        assert!(err.contains("Can't partition by 'region'"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    /// **A quoted partition identifier cannot work and is said so plainly.** DataFusion's COPY
    /// parser keeps the quotes in the string it hands the planner, which then matches no field —
    /// so without this the user reads a message about a column named `"region"`, quotes included.
    #[tokio::test]
    async fn a_quoted_partition_identifier_is_refused_in_the_export_windows_words() {
        let root = scratch("quoted");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        statement(&eng, "CREATE TABLE t AS SELECT 1 AS id, 'emea' AS region")
            .await
            .expect("created");

        let out = root.join("tree");
        let err = statement(
            &eng,
            &format!(
                "COPY t TO '{}' STORED AS CSV PARTITIONED BY (\"region\")",
                out.display()
            ),
        )
        .await
        .expect_err("quoted");
        assert_eq!(
            err,
            "Can't partition by '\"region\"': PARTITIONED BY takes unquoted column names, so a \
             partition column has to be a single plain word"
        );
        assert!(!out.exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// **A typed COPY leaves the engine's options where it found them**, exactly as the export
    /// window now does — `keep_partition_by_columns` is the one option a partitioned write has an
    /// opinion about, and neither surface writes it into the session.
    #[tokio::test]
    async fn a_partitioned_copy_does_not_move_the_engines_own_options() {
        let root = scratch("options");
        let eng = Engine::builder().build();
        eng.set_data_dir(&root);
        statement(&eng, "CREATE TABLE t AS SELECT 1 AS id, 'emea' AS region")
            .await
            .expect("created");

        let before = read(&eng, "SHOW datafusion.execution.keep_partition_by_columns").await;
        statement(
            &eng,
            &format!(
                "COPY t TO '{}' STORED AS CSV PARTITIONED BY (region) \
                 OPTIONS ('execution.keep_partition_by_columns' 'true')",
                root.join("tree").display()
            ),
        )
        .await
        .expect("exported");
        assert_eq!(
            read(&eng, "SHOW datafusion.execution.keep_partition_by_columns").await,
            before,
            "the statement's own option is the statement's own"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The Hive directory names under `dir`, sorted.
    fn levels(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("tree root")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A scratch project folder of our own, per test — the tag is load-bearing because these run
    /// concurrently in one process.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_copy_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        save_defs(&dir, &ProjectDefs::default()).unwrap();
        dir
    }
}
