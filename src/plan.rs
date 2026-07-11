//! EXPLAIN query-plan **model** (S12).
//!
//! This module is the UI-facing plan model — no DataFusion dependency. The
//! engine (`engine::run_explain`) walks DataFusion's own typed `LogicalPlan` /
//! `ExecutionPlan` trees and each operator's live `MetricsSet`, and builds these
//! [`PlanNode`]s directly — so there is **no plan-text parsing** anywhere. The
//! shared string helpers ([`split_name_detail`], [`fmt_ms`]) live here so the
//! engine and any tests use the same formatting.

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
    /// Output rows (`MetricsSet::output_rows`) — ANALYZE only.
    pub rows: Option<u64>,
    /// Compute time in milliseconds (`MetricsSet::elapsed_compute`, ns → ms) —
    /// ANALYZE only. Drives the time-share bar and the hotspot threshold.
    pub ms_val: Option<f64>,
    /// `ms_val` formatted for display (e.g. `2.1 ms`).
    pub ms_label: String,
    /// Every other metric (`bytes_scanned`, `time_elapsed_*`, pruning counters, …)
    /// joined with `·`, each value formatted by its `MetricValue` (units intact).
    pub extra: String,
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

    /// Largest per-node compute time across the physical tree (min 1.0), for
    /// normalising the time-share bars and the hotspot threshold.
    pub fn max_ms(&self) -> f64 {
        self.physical
            .iter()
            .filter_map(|n| n.ms_val)
            .fold(1.0_f64, f64::max)
    }
}

/// Does this SQL start with `EXPLAIN` (ignoring leading comments / parens)?
pub fn is_explain(sql: &str) -> bool {
    let mut bare = String::new();
    for line in sql.lines() {
        let l = match line.find("--") {
            Some(i) => &line[..i],
            None => line,
        };
        bare.push_str(l);
        bare.push(' ');
    }
    bare.trim_start_matches(['(', ' ', '\t', '\n', '\r'])
        .to_ascii_lowercase()
        .starts_with("explain")
}

/// Rewrite `sql` to run under `EXPLAIN` / `EXPLAIN ANALYZE` (E4): strip any existing
/// leading `EXPLAIN [ANALYZE] [VERBOSE]` keyword sequence, then prepend the requested
/// prefix. The editor's Explain-plan / Explain-analyze buttons rewrite the buffer with
/// this so the mode is explicit + user-editable, then run.
pub fn as_explain(sql: &str, analyze: bool) -> String {
    // Strip a leading keyword (word-boundary, case-insensitive), returning the rest.
    fn strip<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
        s.get(..kw.len())
            .filter(|h| h.eq_ignore_ascii_case(kw))
            .filter(|_| {
                s[kw.len()..]
                    .chars()
                    .next()
                    .map_or(true, |c| c.is_whitespace())
            })
            .map(|_| s[kw.len()..].trim_start())
    }
    let mut body = sql.trim_start();
    if let Some(rest) = strip(body, "explain") {
        body = rest;
        if let Some(rest) = strip(body, "analyze") {
            body = rest;
        }
        if let Some(rest) = strip(body, "verbose") {
            body = rest;
        }
    }
    let prefix = if analyze {
        "EXPLAIN ANALYZE"
    } else {
        "EXPLAIN"
    };
    if body.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}\n{body}")
    }
}

/// Split a `"<Name>: <detail>"` operator line (from DataFusion's `display()` /
/// `one_line()`) into (name, detail). No colon → all name.
pub fn split_name_detail(line: &str) -> (String, String) {
    match line.find(':') {
        Some(i) => (
            line[..i].trim().to_string(),
            line[i + 1..].trim().to_string(),
        ),
        None => (line.trim().to_string(), String::new()),
    }
}

/// Format a millisecond value for the per-node time chip.
pub fn fmt_ms(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0} ms")
    } else if v >= 1.0 {
        format!("{v:.1} ms")
    } else if v > 0.0 {
        format!("{v:.3} ms")
    } else {
        "0 ms".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_explain() {
        assert!(is_explain("EXPLAIN SELECT 1"));
        assert!(is_explain("  explain analyze select * from t"));
        assert!(is_explain("-- plan\nEXPLAIN SELECT 1"));
        assert!(!is_explain("SELECT * FROM explain_table"));
    }

    #[test]
    fn as_explain_strips_and_reapplies() {
        assert_eq!(as_explain("SELECT 1", false), "EXPLAIN\nSELECT 1");
        assert_eq!(as_explain("SELECT 1", true), "EXPLAIN ANALYZE\nSELECT 1");
        assert_eq!(
            as_explain("EXPLAIN SELECT 1", true),
            "EXPLAIN ANALYZE\nSELECT 1"
        );
        assert_eq!(
            as_explain("explain analyze select 1", false),
            "EXPLAIN\nselect 1"
        );
        assert_eq!(
            as_explain("  EXPLAIN VERBOSE select 1", false),
            "EXPLAIN\nselect 1"
        );
        // Don't strip an identifier that merely starts with "explain".
        assert_eq!(
            as_explain("SELECT * FROM explain_t", false),
            "EXPLAIN\nSELECT * FROM explain_t"
        );
    }

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

    #[test]
    fn splits_name_and_detail() {
        let (n, d) = split_name_detail("ParquetExec: file_groups={…}, projection=[a, b]");
        assert_eq!(n, "ParquetExec");
        assert_eq!(d, "file_groups={…}, projection=[a, b]");
        let (n, d) = split_name_detail("CoalesceBatchesExec");
        assert_eq!(n, "CoalesceBatchesExec");
        assert_eq!(d, "");
    }

    #[test]
    fn formats_ms() {
        assert_eq!(fmt_ms(0.0), "0 ms");
        assert_eq!(fmt_ms(0.0006), "0.001 ms");
        assert_eq!(fmt_ms(2.14), "2.1 ms");
        assert_eq!(fmt_ms(842.0), "842 ms");
    }
}
