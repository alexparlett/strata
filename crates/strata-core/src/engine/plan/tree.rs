//! The operator tree: [`PlanKind`] (broad category → accent colour), [`PlanNode`]
//! (one flattened operator with its metrics), [`PlanTab`], and the whole
//! [`QueryPlan`] the engine hands the view.

use super::metrics::Metric;

/// Broad operator category, used only for the node's accent colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlanKind {
    Source,
    Join,
    Exchange,
    Agg,
    Sort,
    Proj,
    Limit,
    Util,
}

impl PlanKind {
    /// Classify by operator name (physical `*Exec` names and logical node names),
    /// incl. the file-format source execs (`ParquetExec`, `CsvExec`, …) which
    /// don't contain "scan"/"source".
    pub fn classify(name: &str) -> Self {
        let s = name.to_ascii_lowercase();
        let is_source = [
            "source",
            "scan",
            "parquet",
            "csv",
            "avro",
            "json",
            "arrow",
            "memoryexec",
        ]
            .iter()
            .any(|k| s.contains(k));
        if is_source {
            PlanKind::Source
        } else if s.contains("join") {
            PlanKind::Join
        } else if s.contains("repartition") || s.contains("coalescepartitions") {
            PlanKind::Exchange
        } else if s.contains("aggregate") {
            PlanKind::Agg
        } else if s.contains("sort") {
            PlanKind::Sort
        } else if s.contains("projection") {
            PlanKind::Proj
        } else if s.contains("limit") {
            PlanKind::Limit
        } else {
            PlanKind::Util
        }
    }

    /// The node's accent colour as a CSS variable (the categorical type-colour
    /// palette, so it follows the active theme).
    pub fn color(self) -> &'static str {
        match self {
            PlanKind::Source => "var(--t-str)",
            PlanKind::Join => "var(--t-bool)",
            PlanKind::Exchange => "var(--t-num)",
            PlanKind::Agg => "var(--t-ts)",
            PlanKind::Sort => "var(--t-struct)",
            PlanKind::Proj => "var(--accent)",
            PlanKind::Limit => "var(--t-map)",
            PlanKind::Util => "var(--dim)",
        }
    }
}

/// One operator in a plan, flattened with its tree `depth`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanNode {
    pub name: String,
    pub detail: String,
    pub kind: PlanKind,
    pub depth: usize,
    /// Output rows (`MetricsSet::output_rows`) — ANALYZE only; `None` on operators
    /// that don't emit a row count (e.g. `RepartitionExec`) or plain EXPLAIN.
    pub rows: Option<u64>,
    /// Derived per-node **self-time** in ms (EXPLAIN_PLAN_SPEC §7) — the one
    /// comparable "work done here" number; drives the time chip, the time-share bar
    /// and the hotspot. `None` on plain EXPLAIN (no metrics). See
    /// [`self_time_ms`](super::self_time_ms).
    pub self_ms: Option<f64>,
    /// `self_ms` formatted for display (e.g. `2.1 ms`).
    pub self_label: String,
    /// Typed, pre-labelled metrics (ANALYZE only; empty otherwise). The UI tiers
    /// these — see [`Metric`], [`insights`](super::insights),
    /// [`metric_group`](super::metric_group).
    pub metrics: Vec<Metric>,
}

/// Which plan the EXPLAIN view shows (physical vs logical). `EXPLAIN ANALYZE`
/// forces Physical (the "Plan with Metrics").
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum PlanTab {
    #[default]
    Physical,
    Logical,
}

/// A parsed EXPLAIN result: logical + physical trees (physical carries metrics
/// for ANALYZE) plus each tree's raw indent text for the Raw toggle.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct QueryPlan {
    pub logical: Vec<PlanNode>,
    pub physical: Vec<PlanNode>,
    pub logical_text: String,
    pub physical_text: String,
    pub analyze: bool,
}

impl QueryPlan {
    /// True once at least one tree is present — gates the plan view.
    pub fn is_some(&self) -> bool {
        !self.logical.is_empty() || !self.physical.is_empty()
    }

    /// Largest per-node self-time across the physical tree (min 1.0), for
    /// normalising the time-share bars and the hotspot threshold.
    pub fn max_ms(&self) -> f64 {
        self.physical
            .iter()
            .filter_map(|n| n.self_ms)
            .fold(1.0_f64, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_operators() {
        assert_eq!(PlanKind::classify("ParquetExec"), PlanKind::Source);
        assert_eq!(PlanKind::classify("DataSourceExec"), PlanKind::Source);
        assert_eq!(PlanKind::classify("TableScan"), PlanKind::Source);
        assert_eq!(PlanKind::classify("HashJoinExec"), PlanKind::Join);
        assert_eq!(PlanKind::classify("RepartitionExec"), PlanKind::Exchange);
        assert_eq!(PlanKind::classify("AggregateExec"), PlanKind::Agg);
        assert_eq!(PlanKind::classify("SortExec"), PlanKind::Sort);
        assert_eq!(PlanKind::classify("ProjectionExec"), PlanKind::Proj);
        assert_eq!(PlanKind::classify("GlobalLimitExec"), PlanKind::Limit);
        assert_eq!(PlanKind::classify("CoalesceBatchesExec"), PlanKind::Util);
    }
}
