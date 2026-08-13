//! **Typed `COPY … TO`** (ED-07) — dispatched natively, behind the two checks only the Export
//! window used to provide. `docs/STATEMENTS_SPEC.md` §6.4.
//!
//! Nothing about the *write* is ours: `COPY` is DataFusion's own statement, its options are
//! DataFusion's, and every format Strata can read it can write. What the editor adds is the pair
//! of refusals the managed Export surface had been standing in for, both of which are about a
//! statement that would otherwise succeed and produce something wrong:
//!
//! - **A partition identifier has to be one bare word.** DataFusion 54's COPY parser renders each
//!   one with `Ident::to_string()` and the planner then looks it up by that string, so a quoted
//!   `PARTITIONED BY ("order date")` reaches `field_with_name` still carrying its quotes and fails
//!   about a column the user never named. Refused first, in the Export window's own words
//!   ([`partition_columns_are_bare_words`]).
//! - **A NULL in a partition column is silent corruption.** DataFusion 54 has no
//!   `__HIVE_DEFAULT_PARTITION__`: it files the row under a *neighbouring* value's directory, so
//!   the output reads back claiming a value it never had. The Export window answers this from the
//!   snapshot write pass's free counts; a typed COPY has no snapshot behind it, so it pays for one
//!   extra scan ([`no_null_partition_values`]) — the honest price of the same guarantee over an
//!   arbitrary source.
//!
//! The reserved-name half is the router's ([`classify`](crate::engine::sql::classify)): a
//! `__snap_` relation anywhere in the source refuses with `Blocked::ReservedName`, which is what
//! keeps `COPY (SELECT * FROM __snap_3) TO …` from writing `__strata_ord` into a user's file.
//!
//! The Export window is **unchanged** and remains the snapshot-backed, race-free path: it writes
//! the immutable table the grid is paging, so the file matches what was on screen. A typed COPY
//! reads live tables, and reads them twice when it is partitioned — the gate is a pre-flight, not
//! a lock, and it says so here rather than pretending otherwise.

use std::borrow::Cow;
use std::env;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use datafusion::arrow::array::Int64Array;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::count::count_all;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::{ident, SQLOptions, SessionContext};
use datafusion::sql::parser::Statement as DFStatement;

use crate::engine::export::{
    copy_row_count, partition_columns_are_bare_words, partition_null_refusal,
};
use crate::engine::query::snapshots_root;
use crate::engine::sql::StmtKind;
use crate::project::strata_dir;
use crate::util::plural;

use super::{DataRoot, StatementOutcome};

/// Write a typed `COPY … TO`'s source to disk and report the rows it wrote.
///
/// **The plan that was gated is the plan that runs.** The statement is planned once — planning a
/// `COPY` executes nothing, since execution lives only in `execute_logical_plan` — and that one
/// value is what the NULL gate counts over and what is then driven. Re-dispatching the user's text
/// through `ctx.sql` would judge one plan and execute another, which is the rule the `INSERT` arm
/// already keeps. Driving the plan *is* `ctx.sql` minus the re-parse: `execute_logical_plan`
/// special-cases `Ddl` and `Statement` and hands everything else, `Copy` included, to exactly this.
pub async fn copy_to(
    ctx: &SessionContext,
    stmt: DFStatement,
    root: &DataRoot,
) -> Result<StatementOutcome, String> {
    let DFStatement::CopyTo(copy) = &stmt else {
        // The router classified this as a `COPY` off the parsed statement. Anything else is the
        // two disagreeing.
        return Err(format!(
            "{} did not parse as a copy",
            StmtKind::Copy.label()
        ));
    };
    // Before planning, so the refusal is ours: the planner's own failure for a quoted identifier
    // names a column the user never wrote, which is the message this check exists to replace.
    partition_columns_are_bare_words(&copy.partitioned_by, ctx)?;

    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    let LogicalPlan::Copy(copying) = &plan else {
        return Err(format!("{} did not plan as a copy", StmtKind::Copy.label()));
    };
    // Copied out before the plan is driven, since driving it consumes the plan. `partition_by` is
    // the planner's *resolved* set — the names as the input schema spells them, not as the
    // statement did — which is what the gate below has to count.
    let target = copying.output_url.clone();
    let partition_by = copying.partition_by.clone();
    let input = Arc::clone(&copying.input);

    // **A write only ever leaves Strata's own storage alone.** The reserved-name half of this
    // statement is the router's and covers the *source*; nothing until here has looked at where
    // the write lands. A `COPY … TO '<project>/.strata/tables/sales/extra.arrow'` drops a file
    // inside an internal table's directory, which the next scan of that table lists: schema-matched
    // it is phantom rows, mismatched it is a table that has started failing — silent corruption
    // either way, and the rule is that silent corruption is refused rather than warned about.
    //
    // The *parsed* target, resolved, exactly as `INSERT` gates the target its plan names: a
    // relative `output_url` is the process's cwd away from an absolute one, and comparing the two
    // as text would let `.strata/../.strata/tables` through.
    refuse_owned_target(&target, root)?;

    // Defense in depth behind the router's classification, per spec §4: a write and nothing else.
    // `verify_plan` visits subqueries, so DDL smuggled into the source query dies here even though
    // the classification in front of `Engine::run` already refused it.
    SQLOptions::new()
        .with_allow_dml(true)
        .with_allow_ddl(false)
        .with_allow_statements(false)
        .verify_plan(&plan)
        .map_err(|e| e.to_string())?;

    no_null_partition_values(ctx, &input, &partition_by).await?;

    let batches = DataFrame::new(ctx.state(), plan)
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    // The sink reports what it wrote in the same single `count` column the export and the CTAS
    // spool read.
    let rows = copy_row_count(&batches);

    Ok(StatementOutcome {
        message: format!("Exported {} to '{target}'", plural(rows, "row")),
        count: Some(rows as u64),
        // A `COPY` writes a file and changes nothing the catalog holds. History and the event log
        // still record it, exactly as they record any successful run.
        effect: None,
    })
}

/// Refuse a `COPY` whose target lands in storage Strata owns — the project's `.strata/` directory
/// (internal table data, the session, the conversations) or the snapshot spool.
///
/// **The two fenced roots are the two places a stray file changes what Strata later reads.** A
/// file under `.strata/tables/<slug>/` is listed by that table's next scan; one under the snapshot
/// spool is read back as a result. Everywhere else on the disk is the user's own, and a `COPY` that
/// overwrites their file is the statement doing what it says.
///
/// **Resolved, never compared as text.** A relative `output_url` is the process's cwd away from an
/// absolute one, and `'.strata/../.strata/tables'` names the fenced directory without sharing its
/// prefix. The target need not exist yet, so `canonicalize` cannot be asked about it directly: the
/// path is made absolute, its `.` and `..` segments are folded away, and both sides are then
/// anchored on the deepest ancestor that *does* exist — which is what makes a symlinked project
/// folder compare equal to the path the fence was built from.
fn refuse_owned_target(target: &str, root: &DataRoot) -> Result<(), String> {
    // A target with a scheme belongs to an object store, not to this machine's filesystem.
    // `file:` is the one scheme that *is* a local path, so it is stripped rather than skipped.
    //
    // The scheme has to be **shaped** like one (RFC 3986: a letter, then letters, digits, `+`,
    // `-` or `.`), not merely be whatever precedes the first `://`. Splitting alone read the
    // whole of `<project>/.strata/tables/sales/x://y` as a scheme and waved the target through —
    // a local path with those three characters in a file name skipped this fence entirely.
    let local = match target.split_once("://") {
        Some((scheme, rest)) if is_url_scheme(scheme) => {
            match scheme.eq_ignore_ascii_case("file") {
                true => Cow::Owned(format!("/{}", rest.trim_start_matches('/'))),
                false => return Ok(()),
            }
        }
        _ => Cow::Borrowed(target),
    };
    let path = resolve(Path::new(local.as_ref()));

    let mut fenced = vec![(PathBuf::from(snapshots_root()), "holds query results")];
    if let Some(root) = root {
        fenced.push((strata_dir(root), "holds this project's own data"));
    }
    for (dir, what) in fenced {
        if path.starts_with(resolve(&dir)) {
            return Err(format!(
                "{} can't write into '{}', which {what}",
                StmtKind::Copy.label(),
                dir.display(),
            ));
        }
    }
    Ok(())
}

/// Whether `s` is shaped like a URL scheme — RFC 3986's `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
///
/// A path separator can never appear in one, which is the whole point: it is what tells
/// `s3://bucket` from a local file whose name happens to contain `://`.
fn is_url_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `path` as an absolute path with `.` and `..` folded away, anchored on the deepest ancestor that
/// exists. See [`refuse_owned_target`] for why each of the three steps is there.
fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir().unwrap_or_default().join(path)
    };
    let mut folded = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            // At the root this is a no-op, which is what a filesystem does with it too.
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other),
        }
    }
    let mut existing: &Path = &folded;
    loop {
        if let Ok(real) = existing.canonicalize() {
            let rest = folded.strip_prefix(existing).unwrap_or(Path::new(""));
            return real.join(rest);
        }
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return folded,
        }
    }
}

/// Refuse a partitioned `COPY` whose partition columns contain NULLs, in the Export window's
/// wording ([`partition_null_refusal`]).
///
/// **One extra scan, and it is the whole cost of generality.** The Export window reads exact null
/// counts the snapshot's write pass already produced, for free; a typed COPY's source is any query
/// at all, so the only way to know is to ask. Counted over the *planned input* rather than a
/// rendered `SELECT`, so the thing measured is the thing that will be written.
///
/// The shape is `profile::aggregates`': positional, total first, then one non-null count per
/// partition column. `count(col)` already skips nulls, so a null count is a subtraction and the
/// fallible `ExprFunctionExt` FILTER builder is not needed.
///
/// **Proceed only on an exact zero** — the Export window's rule, kept for its reason: a count that
/// could not be read is not a count of zero, and both readings are a reason to decline. A missing
/// *total* is different, and loud: nothing was measured at all.
async fn no_null_partition_values(
    ctx: &SessionContext,
    input: &LogicalPlan,
    partition_by: &[String],
) -> Result<(), String> {
    if partition_by.is_empty() {
        return Ok(());
    }
    // **Once per distinct name.** `PARTITIONED BY (region, region)` is a statement DataFusion
    // plans without complaint, and two identical `count` expressions collide in the aggregate's
    // own output schema — so counting per *entry* would refuse it with a schema error naming a
    // query the user never wrote. It is also the same question twice.
    let mut names: Vec<&str> = Vec::with_capacity(partition_by.len());
    for name in partition_by {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    // `ident`, not `col`: `col` parses its argument and lower-cases it, and a partition column's
    // name came out of the user's own data. The names resolved during planning, so an unqualified
    // reference to each is unambiguous here by construction.
    let mut exprs = vec![count_all()];
    exprs.extend(names.iter().map(|name| count(ident(*name))));

    let batches = DataFrame::new(ctx.state(), input.clone())
        .aggregate(Vec::new(), exprs)
        .map_err(|e| e.to_string())?
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    let read = |index: usize| -> Option<i64> {
        let batch = batches.first()?;
        batch
            .columns()
            .get(index)?
            .as_any()
            .downcast_ref::<Int64Array>()
            .filter(|a| !a.is_empty())
            .map(|a| a.value(0))
    };

    let Some(rows) = read(0) else {
        return Err(format!(
            "{} could not count the partition columns' NULL values",
            StmtKind::Copy.label()
        ));
    };
    for (index, name) in names.iter().enumerate() {
        if read(index + 1) != Some(rows) {
            return Err(partition_null_refusal(name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::{env, process};

    use super::is_url_scheme;
    use crate::engine::sql::Blocked;
    use crate::engine::{Engine, RunOutcome, RunTag, StatementReport, WsId};
    use crate::project::{save_defs, ProjectDefs};

    /// **A scheme is a scheme, and a path with `://` in it is a path.** Reading everything before
    /// the first `://` as a scheme waved `…/x://y` through the ownership fence as though it named
    /// an object store, which is how a local target inside `.strata/` could skip the check.
    #[test]
    fn only_a_real_scheme_reads_as_a_url() {
        for yes in ["s3", "gs", "http", "https", "file", "s3a", "x+y", "a-b.c"] {
            assert!(is_url_scheme(yes), "{yes}");
        }
        for no in [
            "",
            "3s",     // must start with a letter
            "/tmp/a", // a path is not a scheme
            "sales/eu",
            "/proj/.strata/tables/sales/x", // the shape that skipped the fence
            "a b",
            "a_b", // `_` is not in the scheme charset
        ] {
            assert!(!is_url_scheme(no), "{no}");
        }
    }

    /// The fence itself, over the two shapes that matter: a remote target is not ours to judge,
    /// and a local one carrying `://` is still a local one.
    #[test]
    fn a_local_target_with_a_colon_slash_slash_is_still_fenced() {
        let root = env::temp_dir().join(format!("strata-copy-fence-{}", process::id()));
        let strata = crate::project::strata_dir(&root);
        let owned = strata.join("tables/sales/x://y");
        let data_root: super::DataRoot = Some(root.clone());

        super::refuse_owned_target(&owned.to_string_lossy(), &data_root)
            .expect_err("a local path inside .strata is refused whatever is in its name");
        // A genuine remote target is an object store's, and not this check's business.
        super::refuse_owned_target("s3://acme-lake/out.parquet", &data_root)
            .expect("a remote target is not local storage");
        // And an ordinary local target outside the project is the user's own.
        super::refuse_owned_target(
            &root.join("out.parquet").to_string_lossy(),
            &Some(root.join("elsewhere")),
        )
        .expect("the user's own file");
    }

    /// Run one statement and take its report — anything else is a test that asked the wrong
    /// question.
    async fn statement(eng: &Engine, sql: &str) -> Result<StatementReport, String> {
        match eng.run(WsId(1), RunTag(1), sql.into(), 10).await? {
            RunOutcome::Statement(report) => Ok(report),
            RunOutcome::Rows(..) => panic!("{sql} ran as a query"),
        }
    }

    /// The values a query returns, as text.
    async fn read(eng: &Engine, sql: &str) -> Vec<Vec<String>> {
        let RunOutcome::Rows(output, _) = eng
            .run(WsId(2), RunTag(2), sql.into(), 100)
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
        let eng = Engine::new(BTreeMap::new());
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

    /// **The reserved namespace, from the router.** A snapshot table carries `__strata_ord`, and
    /// nothing but Strata's own readers may ever see it — so a typed COPY that names one is
    /// refused before it can write bookkeeping into a user's file.
    #[tokio::test]
    async fn a_copy_out_of_a_snapshot_is_refused_and_writes_nothing() {
        let root = scratch("reserved");
        let eng = Engine::new(BTreeMap::new());
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
                Blocked::ReservedName.editor_message()
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
        let eng = Engine::new(BTreeMap::new());
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

        // The same statement over the same table, once the NULL is filtered out of the source.
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
        let eng = Engine::new(BTreeMap::new());
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
        let eng = Engine::new(BTreeMap::new());
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

        // And the gate still answers about a repeated column that does contain a NULL.
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
        let eng = Engine::new(BTreeMap::new());
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
        let eng = Engine::new(BTreeMap::new());
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
