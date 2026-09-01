//! Executing an `INSERT`'s input without letting the `Dml` node reach a planner: the rows the
//! remote arm drives through a provider's own sink ([`append_rows`]), and the stream the local
//! arm hands its table store ([`insert_stream`]).
//!
//! **The `Dml` node never reaches a planner.** DataFusion's physical planner answers a
//! `WriteOp::Insert` by resolving the target's provider and calling `insert_into` on it — exactly
//! what [`append_rows`] does — but the node has to survive the *optimizer* first, and
//! `datafusion-federation`'s rule federates any plan whose scans all belong to one remote source,
//! a `Dml` above them included. A federated node writes itself down as SQL to execute, and
//! `plan_to_sql` has no arm for a write: `INSERT INTO <workspace table> SELECT … FROM pg.…` came
//! back as `LogicalPlan` debug. Driving the input is the same plan, the same resolved
//! target, one node fewer.
//!
//! A `CopyTo` cannot be driven this way — its sink is the file format's, built by DataFusion's own
//! physical planner from the node — so the federation assembly keeps that node out of the rule's
//! reach instead ([`sources::sql`](crate::sources::sql)).

use std::sync::Arc;

use datafusion::catalog::TableProvider;
use datafusion::dataframe::DataFrame;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::logical_expr::dml::InsertOp;
use datafusion::logical_expr::LogicalPlan;
use datafusion::optimizer::optimize_projections::OptimizeProjections;
use datafusion::optimizer::{Optimizer, OptimizerContext};
use datafusion::physical_plan::coalesce_partitions::CoalescePartitionsExec;
use datafusion::physical_plan::{collect, ExecutionPlan, ExecutionPlanProperties};
use datafusion::prelude::SessionContext;

use crate::export::copy_row_count;

/// Append `input`'s rows through `provider`'s sink, and report how many landed. The schema check
/// is the provider's, which is why there is none here.
///
/// **The input is coalesced first.** `DataSinkExec` reads partition 0 and nothing else, and says
/// so: a plan built outside the physical optimizer has to arrive single-partition, or a
/// repartitioned scan would write a fraction of its rows and report the fraction as the whole.
pub(crate) async fn append_rows(
    ctx: &SessionContext,
    provider: Arc<dyn TableProvider>,
    input: &LogicalPlan,
) -> Result<u64, String> {
    let state = ctx.state();
    let planned = state
        .create_physical_plan(&collapse_projections(input)?)
        .await
        .map_err(|e| e.to_string())?;
    let single: Arc<dyn ExecutionPlan> = match planned.output_partitioning().partition_count() {
        1 => planned,
        _ => Arc::new(CoalescePartitionsExec::new(planned)),
    };

    let sink = provider
        .insert_into(&state, single, InsertOp::Append)
        .await
        .map_err(|e| e.to_string())?;
    let batches = collect(sink, ctx.task_ctx())
        .await
        .map_err(|e| e.to_string())?;
    Ok(copy_row_count(&batches) as u64)
}

/// Execute an `INSERT`'s input into the single stream the internal-table store appends —
/// [`append_rows`] minus the provider sink, for the arm whose writer is
/// [`InternalTableStore::append`](crate::tables::InternalTableStore::append).
///
/// The same [`collapse_projections`] runs first and for the same reason: what decides whether
/// anything is unparsed is where the rows are read from, and an input scanning a database
/// data source federates here exactly as it does under a provider sink. One stream rather than a
/// coalesce, because `execute_stream` already merges the partitions.
pub(crate) async fn insert_stream(
    ctx: &SessionContext,
    input: &LogicalPlan,
) -> Result<SendableRecordBatchStream, String> {
    DataFrame::new(ctx.state(), collapse_projections(input)?)
        .execute_stream()
        .await
        .map_err(|e| e.to_string())
}

/// Collapse the redundant projection DataFusion's `INSERT` planner leaves, **before** the
/// federation rule wraps the plan.
///
/// `INSERT INTO t SELECT a, b FROM u` plans as a renaming projection over the query's own
/// projection, and DataFusion's unparser renders `Projection -> Projection -> TableScan` as a
/// derived table (`… FROM (SELECT …) AS "derived_projection"`) while leaving the **outer** column
/// references carrying the scan's qualifier — so a remote source comes back from the server as
/// `missing FROM-clause entry for table "customers"`. No statement a user can *write* produces
/// that shape (a subquery carries an alias the outer refs then use); only a planner-built plan
/// does, which is why it surfaced on the remote arm first.
///
/// It belongs to the **input**, not to either sink: what decides whether anything is unparsed is
/// where the rows are read from, not where they land.
///
/// It has to be done here rather than through the executor's `logical_optimizer` hook, which is
/// otherwise exactly the seam for it: by the time that hook runs the plan is already inside the
/// federation crate's extension node, so a rule walking it rewrites nothing. And ahead of
/// [`append_rows`]'s `create_physical_plan`, since the federation rule sits early in the optimizer
/// and `OptimizeProjections` runs near the end of it.
///
/// One rule rather than the default optimizer — the rest of that pass is about *execution*, and
/// this is about what can be written down. `create_physical_plan` still runs the full analyzer and
/// optimizer over the result.
///
/// **Retirable when <https://github.com/apache/datafusion/issues/13027> lands** — the unparser bug
/// this exists for, open upstream from the user-written side of the same shape.
fn collapse_projections(input: &LogicalPlan) -> Result<LogicalPlan, String> {
    Optimizer::with_rules(vec![Arc::new(OptimizeProjections::new())])
        .optimize(input.clone(), &OptimizerContext::new(), |_, _| {})
        .map_err(|e| e.to_string())
}

/// **The shape [`collapse_projections`] exists for, pinned on both sides** — that DataFusion's
/// `INSERT` planner still produces it, and that one `OptimizeProjections` pass still removes it.
///
/// Its own test rather than only the integration test's insert-from-a-remote-source, because that
/// one fails twelve minutes away in another binary and says only that a server rejected some SQL.
/// What can actually move under this is DataFusion: a planner that stops nesting the projections
/// makes the first assertion fail (the collapse becomes dead weight), and an `OptimizeProjections`
/// that stops merging them makes the second fail (the unparse breaks again). Neither needs a
/// database to notice, so neither should wait for one.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::datasource::empty::EmptyTable;

    use crate::builder::test_context;

    use super::*;

    /// A session with a source and a target whose columns differ in name — which is what makes
    /// the planner add its renaming projection, and is the real statement's shape
    /// (`INSERT INTO pg.public.loaded SELECT name, id FROM pg.public.customers`).
    fn session() -> SessionContext {
        let ctx = test_context(&BTreeMap::new());
        for (name, columns) in [("source", ["name", "id"]), ("target", ["tier", "total"])] {
            let schema = Arc::new(Schema::new(vec![
                Field::new(columns[0], DataType::Utf8, true),
                Field::new(columns[1], DataType::Int32, true),
            ]));
            ctx.register_table(name, Arc::new(EmptyTable::new(schema)))
                .expect("table");
        }
        ctx
    }

    /// How many `Projection` nodes `plan` holds, root included.
    fn projections(plan: &LogicalPlan) -> usize {
        let here = usize::from(matches!(plan, LogicalPlan::Projection(_)));
        plan.inputs().iter().map(|i| projections(i)).sum::<usize>() + here
    }

    #[tokio::test]
    async fn an_inserts_input_arrives_as_nested_projections_and_leaves_as_one() {
        let ctx = session();
        let plan = ctx
            .state()
            .create_logical_plan("INSERT INTO target SELECT name, id FROM source")
            .await
            .expect("planned");
        let LogicalPlan::Dml(dml) = &plan else {
            panic!("{plan:?}");
        };

        assert_eq!(
            projections(&dml.input),
            2,
            "the planner still stacks its renaming projection on the query's own: {}",
            dml.input.display_indent()
        );

        let collapsed = collapse_projections(&dml.input).expect("collapsed");
        assert_eq!(
            projections(&collapsed),
            1,
            "and the pair the unparser cannot render is gone: {}",
            collapsed.display_indent()
        );
        assert_eq!(
            collapsed.schema(),
            dml.input.schema(),
            "without moving what the sink is handed"
        );
    }
}
