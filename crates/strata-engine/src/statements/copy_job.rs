//! The one write path: a [`CopyJob`] is gated and driven, whoever composed it.
//!
//! Three surfaces write a result to a path — the Export window, a typed `COPY … TO`
//! (`arms::copy`) and the agent's `export_result` — and all three compose the same five values
//! and hand them here. What arrives is what runs: the plan that is gated is the plan that is
//! executed, which is why nothing on this path re-renders a statement as text.
//!
//! **The gates are the job's, so a new way of composing one cannot skip them.** The owned-storage
//! fence and the NULL-partition refusal both run here. The third gate — that a partition column is
//! one bare word — is each caller's, because the only useful place to ask is before the names
//! reach a planner.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::Int64Array;
use datafusion::common::file_options::file_type::FileType;
use datafusion::dataframe::DataFrame;
use datafusion::functions_aggregate::count::count_all;
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::logical_expr::dml::CopyTo;
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::{ident, SessionContext};

use crate::export::{copy_row_count, refuse_owned_target, Owned};
use crate::snapshots::SnapshotStats;

/// One write, in the five values DataFusion's own `COPY` node is made of.
///
/// A description rather than a plan: [`run_copy`] builds the node, so nothing between composing a
/// job and running it can be holding a plan that skipped a gate.
pub(crate) struct CopyJob {
    /// The rows to write.
    pub input: Arc<LogicalPlan>,
    /// Where they go — one file when the path names one, a directory otherwise.
    pub target: String,
    /// The writer, out of the session's own file-format registry — so a format the editor can
    /// name is one the Export window can write.
    pub file_type: Arc<dyn FileType>,
    /// Write options, already namespaced (`format.has_header`,
    /// `execution.keep_partition_by_columns`) — the spelling the planner's own option map is in.
    pub options: HashMap<String, String>,
    /// Hive partition columns, outermost first. Empty is a flat write.
    pub partition_by: Vec<String>,
}

/// How this job's partition columns can be shown to hold no NULLs.
///
/// One rule, two ways of answering it, because the callers hold genuinely different facts. The
/// sentence the user reads is the same either way ([`partition_null_refusal`]).
pub(crate) enum NullEvidence<'a> {
    /// The counts the write pass already produced, free and exact ([`SnapshotStats::nulls`]) —
    /// what a job over a settled result can answer from without scanning anything.
    Snapshot(&'a SnapshotStats),
    /// Nothing counted yet, so count. A typed `COPY`'s source is any query at all, which makes one
    /// extra pre-flight scan the whole cost of generality.
    Count,
}

/// Gate `job` and write it, answering with the rows that landed.
///
/// `subject` is what the fence's refusal is about, because the user reads it as being about the
/// thing they did: `COPY` for the typed statement, `Export` for the window and the agent.
pub(crate) async fn run_copy(
    ctx: &SessionContext,
    job: CopyJob,
    owned: &[Owned],
    nulls: NullEvidence<'_>,
    subject: &str,
) -> Result<usize, String> {
    refuse_owned_target(&job.target, owned, subject)?;
    no_null_partition_values(ctx, &job, nulls).await?;

    let plan = LogicalPlan::Copy(CopyTo::new(
        job.input,
        job.target,
        job.partition_by,
        job.file_type,
        job.options,
    ));
    let batches = DataFrame::new(ctx.state(), plan)
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    Ok(copy_row_count(&batches))
}

/// Refuse a partitioned write whose partition columns contain NULLs.
///
/// **Why this is a hard block and not a warning.** A directory name cannot hold a NULL, and
/// DataFusion 54 does not use the Hive convention (`__HIVE_DEFAULT_PARTITION__`) for one: it
/// files the row under a *neighbouring* value's directory, so it reads back claiming a value it
/// never had. That is silent data corruption in the user's own output, discoverable only by
/// comparing against the source — so the write declines rather than warns.
///
/// **Proceed only on an exact zero**, whichever evidence answered: a count that could not be read
/// is not a count of zero, and both readings are a reason to decline.
async fn no_null_partition_values(
    ctx: &SessionContext,
    job: &CopyJob,
    nulls: NullEvidence<'_>,
) -> Result<(), String> {
    if job.partition_by.is_empty() {
        return Ok(());
    }
    match nulls {
        NullEvidence::Snapshot(stats) => from_the_write_pass(job, stats),
        NullEvidence::Count => by_counting(ctx, job).await,
    }
}

/// The counts the snapshot's write pass observed, read by column position.
///
/// **Nothing is gained by asking the file.** The snapshot is Arrow IPC, which carries no column
/// statistics at all, but `Array::null_count` is a stored field and the write pass streams every
/// batch anyway — so the exact per-column count is a running sum over data already in hand.
///
/// The position is the job input's, which is the result's own columns with the ordinal projected
/// away: the order [`SnapshotStats::nulls`] counts in.
fn from_the_write_pass(job: &CopyJob, stats: &SnapshotStats) -> Result<(), String> {
    for name in &job.partition_by {
        let index = job
            .input
            .schema()
            .fields()
            .iter()
            .position(|f| f.name() == name)
            .ok_or_else(|| format!("Can't partition by '{name}': the result has no such column"))?;
        if stats.nulls.get(index).copied() != Some(0) {
            return Err(partition_null_refusal(name));
        }
    }
    Ok(())
}

/// One pre-flight aggregate over the job's own input, so the thing measured is the thing that
/// will be written.
///
/// The shape is `profile::aggregates`': positional, total first, then one non-null count per
/// partition column. `count(col)` already skips nulls, so a null count is a subtraction and the
/// fallible `ExprFunctionExt` FILTER builder is not needed. A column named twice in
/// `PARTITIONED BY` is counted **once**: two identical `count` expressions collide in the
/// aggregate's own output schema, which failed the statement with a schema error about a query
/// the user never wrote.
///
/// A missing *total* is its own case, and loud: nothing was measured at all.
async fn by_counting(ctx: &SessionContext, job: &CopyJob) -> Result<(), String> {
    let mut names: Vec<&str> = Vec::with_capacity(job.partition_by.len());
    for name in &job.partition_by {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    let mut exprs = vec![count_all()];
    exprs.extend(names.iter().map(|name| count(ident(*name))));

    let batches = DataFrame::new(ctx.state(), (*job.input).clone())
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
        return Err("Could not count the partition columns' NULL values".to_string());
    };
    for (index, name) in names.iter().enumerate() {
        if read(index + 1) != Some(rows) {
            return Err(partition_null_refusal(name));
        }
    }
    Ok(())
}

/// Why a partition column containing NULLs is refused, in the one wording every surface uses.
///
/// Two mechanisms, one sentence: the fact the user is told is the same fact whether it came from
/// a write pass's counts or from a pre-flight scan, and a second phrasing of it would read like a
/// second rule.
fn partition_null_refusal(name: &str) -> String {
    format!(
        "Can't partition by '{name}': it contains NULL values, and a NULL has no folder name — \
         those rows would be written under another value and read back wrong. Partition by a \
         column with no NULLs, or filter them out of the query first"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use datafusion::arrow::array::{Int64Array, StringArray};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;

    use super::*;
    use crate::builder::test_context;
    use crate::export::Format;

    /// A job over a two-column result — enough for the write-pass gate, which reads nothing but
    /// the input's schema and the counts it is handed.
    async fn job(partition_by: &[&str]) -> CopyJob {
        let ctx = test_context(&BTreeMap::new());
        let schema = Arc::new(Schema::new(vec![
            Field::new("amount", DataType::Int64, true),
            Field::new("name", DataType::Utf8, true),
        ]));
        let rows = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(StringArray::from(vec![Some("a"), None])),
            ],
        )
        .expect("a result batch");
        CopyJob {
            input: Arc::new(
                ctx.read_batch(rows)
                    .expect("a frame over the rows")
                    .into_unoptimized_plan(),
            ),
            target: "/tmp/out.csv".into(),
            file_type: Format::Arrow
                .file_type(&ctx.state())
                .expect("arrow is a registered writer"),
            options: HashMap::new(),
            partition_by: partition_by.iter().copied().map(String::from).collect(),
        }
    }

    /// **The snapshot's counts are read by column position, and the position is the job input's.**
    /// The ordinal is sorted by and then projected away before the job is built, so the input's
    /// fields line up with what the write pass counted — the assumption the whole free-evidence
    /// path rests on.
    #[tokio::test]
    async fn the_write_passs_counts_are_read_by_the_inputs_own_column_order() {
        let stats = SnapshotStats {
            nulls: vec![0, 4],
            ord: Some("__strata_ord".into()),
        };
        from_the_write_pass(&job(&["amount"]).await, &stats).expect("amount has no NULLs");

        let err = from_the_write_pass(&job(&["name"]).await, &stats).expect_err("name has four");
        assert_eq!(err, partition_null_refusal("name"));
        assert!(err.contains("read back wrong"), "{err}");
    }

    /// A partition column the result does not have is its own refusal, not a NULL one — there is
    /// no position to read the counts at.
    #[tokio::test]
    async fn a_partition_column_the_result_lacks_is_named_as_missing() {
        let stats = SnapshotStats {
            nulls: vec![0, 0],
            ord: None,
        };
        assert_eq!(
            from_the_write_pass(&job(&["region"]).await, &stats).expect_err("no such column"),
            "Can't partition by 'region': the result has no such column"
        );
    }

    /// **An unknown null count is not a zero.** [`SnapshotStats`] is exact by construction, so a
    /// short `nulls` means the evidence does not cover that column — and the rule is "proceed only
    /// on an exact zero", which declines rather than guessing.
    #[tokio::test]
    async fn evidence_that_does_not_cover_the_column_declines() {
        let stats = SnapshotStats {
            nulls: vec![0],
            ord: None,
        };
        assert_eq!(
            from_the_write_pass(&job(&["name"]).await, &stats).expect_err("uncounted"),
            partition_null_refusal("name")
        );
    }

    /// A flat write asks nothing at all — the gate is about directory names, and there are none.
    #[tokio::test]
    async fn an_unpartitioned_job_is_not_asked_about_nulls() {
        let ctx = test_context(&BTreeMap::new());
        let stats = SnapshotStats {
            nulls: vec![9, 9],
            ord: None,
        };
        no_null_partition_values(&ctx, &job(&[]).await, NullEvidence::Snapshot(&stats))
            .await
            .expect("nothing is partitioned by");
    }
}
