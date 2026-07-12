//! Typed operator metrics and everything derived from them: [`MetricKind`] /
//! [`Metric`], the tier-3 grouping ([`METRIC_GROUPS`], [`metric_group`],
//! [`group_color`]), the tier-2 [`insights`] ([`Insight`] / [`InsightTone`]), and
//! the derived per-node [`self_time_ms`]. Mirrors the v19 design's metric model.

use super::fmt::{fmt_bytes, fmt_int, fmt_ns};
use super::tree::PlanKind;

/// The unit-class of a metric value, so the UI can format and group it without
/// re-deriving units (mirrors the `type` in EXPLAIN_PLAN_SPEC §4/§8). The engine
/// tags each [`Metric`] with one, derived from DataFusion's `MetricValue` variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MetricKind {
    Count,
    Time,
    Bytes,
    Memory,
    Ratio,
}

impl MetricKind {
    /// Format a raw aggregated `value` into a unit-aware, ready-to-print label
    /// (time values are nanoseconds; bytes/memory are bytes; counts are plain).
    /// `Ratio` has no single numeric unit — the engine passes DataFusion's own
    /// display string as the label, so that arm here is only a fallback.
    pub fn format(self, value: u64) -> String {
        match self {
            MetricKind::Time => fmt_ns(value),
            MetricKind::Bytes | MetricKind::Memory => fmt_bytes(value),
            MetricKind::Count => fmt_int(value),
            MetricKind::Ratio => value.to_string(),
        }
    }

    /// The value colour (a CSS var) for the tier-3 grid — the v19 type palette.
    pub fn color(self) -> &'static str {
        match self {
            MetricKind::Time => "var(--warm)",
            MetricKind::Bytes | MetricKind::Memory => "var(--t-list)",
            MetricKind::Count => "var(--text2)",
            MetricKind::Ratio => "var(--t-str)",
        }
    }
}

/// One typed, pre-labelled operator metric (EXPLAIN_PLAN_SPEC §8). Built by the
/// engine from a DataFusion `MetricValue`; the UI never re-derives units.
#[derive(Clone, Debug, PartialEq)]
pub struct Metric {
    pub name: String,
    /// Raw aggregated value (`MetricValue::as_usize`): ns for time, bytes for
    /// bytes/memory, otherwise a plain count. Drives zero-detection + self-time.
    pub value: u64,
    pub kind: MetricKind,
    /// Unit-aware display label (`15.6 ms`, `605 B`, `48,213`).
    pub label: String,
    /// `value == 0` — lets the UI hide/deprioritise the (many) zero counters.
    pub zero: bool,
}

/// Fixed tier-3 group order (EXPLAIN_PLAN_SPEC §8; matches the v19 design mock).
pub const METRIC_GROUPS: [&str; 9] = [
    "Output",
    "Time",
    "I/O",
    "Pruning",
    "Memory & spill",
    "Exchange",
    "Join",
    "Errors",
    "Other",
];

/// Which tier-3 group a metric falls in — mirrors the v19 design's `metricGroup`
/// exactly (name-keyed, first match wins).
pub fn metric_group(name: &str) -> &'static str {
    if name == "output_rows" || name == "output_batches" {
        "Output"
    } else if name.ends_with("error") || name.ends_with("errors") {
        "Errors"
    } else if name == "bytes_scanned" {
        "I/O"
    } else if name.starts_with("row_groups_")
        || name.starts_with("page_index_rows_")
        || name.starts_with("pushdown_rows_")
    {
        "Pruning"
    } else if name == "peak_mem_used"
        || name == "build_mem_used"
        || name.starts_with("spill")
        || name == "skipped_aggregation_rows"
    {
        "Memory & spill"
    } else if name == "repartition_time" || name == "send_time" || name == "fetch_time" {
        "Exchange"
    } else if name == "build_time"
        || name == "join_time"
        || name == "build_input_rows"
        || name == "input_rows"
    {
        "Join"
    } else if name == "elapsed_compute" || name.contains("time") {
        "Time"
    } else {
        "Other"
    }
}

/// The colour (a CSS var) for a tier-3 group's header bar — the v19 palette.
pub fn group_color(group: &str) -> &'static str {
    match group {
        "Output" => "var(--accent)",
        "Time" => "var(--warm)",
        "I/O" => "var(--t-str)",
        "Pruning" => "var(--t-bool)",
        "Memory & spill" => "var(--t-list)",
        "Exchange" => "var(--t-num)",
        "Join" => "var(--t-bool)",
        "Errors" => "var(--red)",
        _ => "var(--dim)",
    }
}

/// Tone of a tier-2 insight callout — drives its colour (v19 palette): `Err` red,
/// `Warn` amber, `Ok` green, `Info` blue.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InsightTone {
    Err,
    Warn,
    Ok,
    Info,
}

impl InsightTone {
    /// The pill text colour as a CSS var.
    pub fn color(self) -> &'static str {
        match self {
            InsightTone::Err => "var(--red)",
            InsightTone::Warn => "var(--warm)",
            InsightTone::Ok => "var(--t-str)",
            InsightTone::Info => "var(--t-list)",
        }
    }
}

/// A tier-2 "insight" callout — a non-zero signal worth surfacing above the full
/// metrics grid (EXPLAIN_PLAN_SPEC §6.3), tone-coded.
#[derive(Clone, Debug, PartialEq)]
pub struct Insight {
    pub text: String,
    pub tone: InsightTone,
}

/// Derive the tier-2 insight callouts from a node's typed metrics — mirrors the
/// v19 design's `planInsights` exactly: non-zero errors, spills, row-group
/// pruning/matching (statistics + bloom), pushdown, peak/build memory,
/// selectivity — in that priority order. Pure over [`Metric`] so it's unit-tested.
pub fn insights(metrics: &[Metric]) -> Vec<Insight> {
    let val = |name: &str| {
        metrics
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.value)
            .unwrap_or(0)
    };
    let label = |name: &str| {
        metrics
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.label.clone())
    };
    let mut out = Vec::new();

    // Non-zero error counters, each surfaced loudly.
    for m in metrics {
        if (m.name.ends_with("error") || m.name.ends_with("errors")) && m.value > 0 {
            out.push(Insight {
                text: format!("{} {}", fmt_int(m.value), m.name.replace('_', " ")),
                tone: InsightTone::Err,
            });
        }
    }
    // Spills (memory pressure).
    if val("spilled_bytes") > 0 {
        out.push(Insight {
            text: format!("spilled {}", label("spilled_bytes").unwrap_or_default()),
            tone: InsightTone::Warn,
        });
    } else if val("spill_count") > 0 {
        let n = val("spill_count");
        out.push(Insight {
            text: format!("{n} spill{}", if n == 1 { "" } else { "s" }),
            tone: InsightTone::Warn,
        });
    }
    // Row-group pruning / matching (statistics + bloom filter).
    let pv = val("row_groups_pruned_statistics") + val("row_groups_pruned_bloom_filter");
    let mv = val("row_groups_matched_statistics") + val("row_groups_matched_bloom_filter");
    if pv > 0 {
        out.push(Insight {
            text: format!("pruned {pv}/{} row groups", pv + mv),
            tone: InsightTone::Ok,
        });
    } else if mv > 0 {
        out.push(Insight {
            text: format!("matched {mv} row group{}", if mv == 1 { "" } else { "s" }),
            tone: InsightTone::Info,
        });
    }
    // Pushdown filter removed rows.
    if val("pushdown_rows_pruned") > 0 {
        out.push(Insight {
            text: format!("pushdown removed {} rows", fmt_int(val("pushdown_rows_pruned"))),
            tone: InsightTone::Ok,
        });
    }
    // Memory high-water marks.
    if val("peak_mem_used") > 0 {
        out.push(Insight {
            text: format!("peak {}", label("peak_mem_used").unwrap_or_default()),
            tone: InsightTone::Info,
        });
    }
    if val("build_mem_used") > 0 {
        out.push(Insight {
            text: format!("build {}", label("build_mem_used").unwrap_or_default()),
            tone: InsightTone::Info,
        });
    }
    // Filter selectivity (shown whenever present).
    if let Some(l) = label("selectivity") {
        out.push(Insight {
            text: format!("selectivity {l}"),
            tone: InsightTone::Info,
        });
    }
    out
}

/// Per-node **self-time** in milliseconds (EXPLAIN_PLAN_SPEC §7) computed from the
/// typed metric list: the real "work done here", picked per operator kind because
/// there is no single reliable time field. `fetch_time`/`send_time` (exchange wait,
/// not work) are deliberately never used. Returns `None` when the node carries no
/// metrics (plain EXPLAIN); otherwise `Some` (0.0 if the kind's metric is absent).
pub fn self_time_ms(kind: PlanKind, metrics: &[Metric]) -> Option<f64> {
    if metrics.is_empty() {
        return None;
    }
    let ns = |name: &str| metrics.iter().find(|m| m.name == name).map(|m| m.value);
    let ns_val = match kind {
        PlanKind::Source => ns("time_elapsed_processing")
            .or_else(|| ns("time_elapsed_scanning_total"))
            .or_else(|| ns("elapsed_compute"))
            .unwrap_or(0),
        PlanKind::Join => match (ns("build_time"), ns("join_time")) {
            (None, None) => ns("elapsed_compute").unwrap_or(0),
            (b, j) => b.unwrap_or(0) + j.unwrap_or(0),
        },
        PlanKind::Exchange => ns("repartition_time").unwrap_or(0),
        _ => ns("elapsed_compute").unwrap_or(0),
    };
    Some(ns_val as f64 / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PlanKind;

    fn m(name: &str, value: u64, kind: MetricKind) -> Metric {
        Metric {
            name: name.to_string(),
            value,
            kind,
            label: kind.format(value),
            zero: value == 0,
        }
    }

    #[test]
    fn self_time_picks_per_kind() {
        // Source prefers time_elapsed_processing over the misleading elapsed_compute.
        let src = vec![
            m("elapsed_compute", 1, MetricKind::Time),
            m("time_elapsed_processing", 15_594_334, MetricKind::Time),
        ];
        assert!((self_time_ms(PlanKind::Source, &src).unwrap() - 15.594334).abs() < 1e-6);
        // Join sums build + probe.
        let join = vec![
            m("build_time", 216_000, MetricKind::Time),
            m("join_time", 146_000, MetricKind::Time),
        ];
        assert!((self_time_ms(PlanKind::Join, &join).unwrap() - 0.362).abs() < 1e-6);
        // Exchange uses repartition_time, never fetch/send wait.
        let ex = vec![
            m("repartition_time", 29_000, MetricKind::Time),
            m("fetch_time", 337_000_000, MetricKind::Time),
        ];
        assert!((self_time_ms(PlanKind::Exchange, &ex).unwrap() - 0.029).abs() < 1e-6);
        // A compute op falls back to elapsed_compute.
        let agg = vec![m("elapsed_compute", 4_790_000, MetricKind::Time)];
        assert!((self_time_ms(PlanKind::Agg, &agg).unwrap() - 4.79).abs() < 1e-6);
        // No metrics (plain EXPLAIN) → None.
        let empty: Vec<Metric> = Vec::new();
        assert_eq!(self_time_ms(PlanKind::Source, &empty), None);
    }

    #[test]
    fn groups_metrics_by_bucket() {
        // Mirrors the v19 design's `metricGroup`.
        assert_eq!(metric_group("output_rows"), "Output");
        assert_eq!(metric_group("output_batches"), "Output");
        assert_eq!(metric_group("bytes_scanned"), "I/O");
        assert_eq!(metric_group("output_bytes"), "Other");
        assert_eq!(metric_group("metadata_load_time"), "Time");
        assert_eq!(metric_group("elapsed_compute"), "Time");
        assert_eq!(metric_group("repartition_time"), "Exchange");
        assert_eq!(metric_group("spilled_bytes"), "Memory & spill");
        assert_eq!(metric_group("peak_mem_used"), "Memory & spill");
        assert_eq!(metric_group("row_groups_pruned_statistics"), "Pruning");
        assert_eq!(metric_group("build_mem_used"), "Memory & spill");
        assert_eq!(metric_group("join_time"), "Join");
        assert_eq!(metric_group("file_open_errors"), "Errors");
    }

    #[test]
    fn insights_only_nonzero_signal() {
        let metrics = vec![
            m("row_groups_matched_statistics", 2, MetricKind::Count),
            m("row_groups_pruned_statistics", 0, MetricKind::Count),
            m("peak_mem_used", 3481, MetricKind::Memory),
            m("file_open_errors", 0, MetricKind::Count),
            m("bytes_scanned", 605, MetricKind::Bytes),
        ];
        let got = insights(&metrics);
        // matched + peak surface; the zeros (pruned/errors) and plain bytes don't.
        assert_eq!(got.len(), 2);
        assert!(got
            .iter()
            .any(|i| i.text == "matched 2 row groups" && i.tone == InsightTone::Info));
        assert!(got.iter().any(|i| i.text == "peak 3.4 KB"));
    }

    #[test]
    fn pruned_insight_shows_fraction() {
        let metrics = vec![
            m("row_groups_pruned_statistics", 3, MetricKind::Count),
            m("row_groups_matched_statistics", 1, MetricKind::Count),
        ];
        let got = insights(&metrics);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "pruned 3/4 row groups");
        assert_eq!(got[0].tone, InsightTone::Ok);
    }
}
