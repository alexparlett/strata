//! `EXPLAIN [ANALYZE]` → a structured [`crate::plan::QueryPlan`], walked from
//! DataFusion's own typed logical/physical plans (no plan-text parsing).

use datafusion::logical_expr::LogicalPlan;
use datafusion::physical_plan::display::DisplayableExecutionPlan;
use datafusion::physical_plan::metrics::MetricValue;
use datafusion::physical_plan::{collect, displayable, ExecutionPlan};
use datafusion::prelude::*;

use crate::plan::{PlanKind, PlanNode, QueryPlan};

/// Build a structured [`QueryPlan`] for an `EXPLAIN [ANALYZE]` statement by
/// walking DataFusion's own typed plans — **no plan-text parsing**.
///
/// We plan the EXPLAIN, unwrap the `Explain`/`Analyze` wrapper to the real inner
/// `LogicalPlan`, re-plan it to a physical `ExecutionPlan`, and (for ANALYZE)
/// execute it so each operator's live `MetricsSet` is populated. Then we walk the
/// logical and physical trees into `PlanNode`s, reading each node's name,
/// one-line detail, and metrics directly from the DataFusion types.
pub async fn run_explain(ctx: &SessionContext, sql: &str) -> Result<QueryPlan, String> {
    let opts = SQLOptions::new()
        .with_allow_dml(false)
        .with_allow_ddl(false)
        .with_allow_statements(false);

    let df = ctx
        .sql_with_options(sql, opts)
        .await
        .map_err(|e| e.to_string())?;

    // Unwrap `EXPLAIN`/`EXPLAIN ANALYZE` to the plan being explained.
    let (inner, analyze) = match df.logical_plan() {
        LogicalPlan::Explain(e) => (e.plan.as_ref(), false),
        LogicalPlan::Analyze(a) => (a.input.as_ref(), true),
        other => (other, false),
    };

    let mut plan = QueryPlan {
        analyze,
        logical: walk_logical(inner),
        logical_text: inner.display_indent().to_string(),
        ..Default::default()
    };

    // Re-plan the inner logical plan to physical. `SessionState` has an inherent
    // `create_physical_plan` in DataFusion 43 (no `Session` trait import needed).
    let state = ctx.state();
    let physical = state
        .create_physical_plan(inner)
        .await
        .map_err(|e| e.to_string())?;

    // ANALYZE: run the query so live metrics land on the plan's operators.
    if analyze {
        let _ = collect(physical.clone(), ctx.task_ctx())
            .await
            .map_err(|e| e.to_string())?;
    }

    plan.physical = walk_physical(physical.as_ref());
    plan.physical_text = if analyze {
        DisplayableExecutionPlan::with_metrics(physical.as_ref())
            .indent(false)
            .to_string()
    } else {
        displayable(physical.as_ref()).indent(false).to_string()
    };

    if !plan.is_some() {
        return Err("Could not build the query plan".to_string());
    }
    Ok(plan)
}

/// Flatten a logical plan into depth-tagged `PlanNode`s. `LogicalPlan::display`
/// renders one node without its children (e.g. `Projection: id`).
fn walk_logical(root: &LogicalPlan) -> Vec<PlanNode> {
    fn go(p: &LogicalPlan, depth: usize, out: &mut Vec<PlanNode>) {
        let (name, detail) = crate::plan::split_name_detail(p.display().to_string().trim());
        out.push(PlanNode {
            kind: PlanKind::classify(&name),
            name,
            detail,
            depth,
            rows: None,
            self_ms: None,
            self_label: String::new(),
            metrics: Vec::new(),
        });
        for c in p.inputs() {
            go(c, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    go(root, 0, &mut out);
    out
}

/// Flatten a physical plan into depth-tagged `PlanNode`s, reading each operator's
/// one-line display and (if executed) its metrics.
fn walk_physical(root: &dyn ExecutionPlan) -> Vec<PlanNode> {
    fn go(p: &dyn ExecutionPlan, depth: usize, out: &mut Vec<PlanNode>) {
        let line = displayable(p).one_line().to_string();
        let (name, detail) = crate::plan::split_name_detail(line.trim());
        let kind = PlanKind::classify(&name);
        let (rows, metrics) = node_metrics(p);
        // Derive the one comparable per-node time (EXPLAIN_PLAN_SPEC §7) from the
        // typed metrics — logic lives in `crate::plan`, pure over `Metric`.
        let self_ms = crate::plan::self_time_ms(kind, &metrics);
        let self_label = self_ms.map(crate::plan::fmt_ms).unwrap_or_default();
        out.push(PlanNode {
            kind,
            name,
            detail,
            depth,
            rows,
            self_ms,
            self_label,
            metrics,
        });
        for c in p.children() {
            go(c.as_ref(), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    go(root, 0, &mut out);
    out
}

/// Read a physical operator's metrics: output rows (the `rows` field) plus every
/// other named metric as a typed, pre-labelled [`crate::plan::Metric`] — classified
/// by `MetricValue` variant so the UI can format + group without unit math. The raw
/// `elapsed_compute` timestamps are dropped; `output_rows` becomes `rows`.
fn node_metrics(p: &dyn ExecutionPlan) -> (Option<u64>, Vec<crate::plan::Metric>) {
    let Some(ms) = p.metrics() else {
        return (None, Vec::new());
    };
    let ms = ms.aggregate_by_name();
    let rows = ms.output_rows().map(|r| r as u64);

    let mut metrics = Vec::new();
    for m in ms.iter() {
        let mv = m.value();
        // `output_rows` is *also* kept in the list (tier-3 "Output" group) — it just
        // additionally surfaces as the headline `rows`. Timestamps aren't metrics.
        if mv.is_timestamp() {
            continue;
        }
        let kind = metric_kind(mv);
        let value = mv.as_usize() as u64;
        // Ratio/pruning have no single scalar unit → keep DataFusion's own display
        // string; everything else gets our unit-aware label.
        let label = match kind {
            crate::plan::MetricKind::Ratio => mv.to_string(),
            k => k.format(value),
        };
        metrics.push(crate::plan::Metric {
            name: mv.name().to_string(),
            value,
            kind,
            label,
            zero: value == 0,
        });
    }
    (rows, metrics)
}

/// Classify a DataFusion `MetricValue` into the UI's [`crate::plan::MetricKind`],
/// by variant first (robust — `elapsed_compute`'s name has no "time" in it), then a
/// name heuristic for the generic operator-defined `Count`/`Gauge` metrics.
fn metric_kind(v: &MetricValue) -> crate::plan::MetricKind {
    use crate::plan::MetricKind as K;
    match v {
        MetricValue::ElapsedCompute(_) | MetricValue::Time { .. } => K::Time,
        MetricValue::SpilledBytes(_) | MetricValue::OutputBytes(_) => K::Bytes,
        MetricValue::CurrentMemoryUsage(_) => K::Memory,
        MetricValue::Gauge { name, .. } if name.contains("mem") => K::Memory,
        MetricValue::Ratio { .. } | MetricValue::PruningMetrics { .. } => K::Ratio,
        MetricValue::Count { name, .. } if name.contains("bytes") => K::Bytes,
        MetricValue::Count { name, .. } if name.contains("mem") => K::Memory,
        MetricValue::Count { name, .. } if name.contains("time") => K::Time,
        _ => K::Count,
    }
}
