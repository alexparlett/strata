//! EXPLAIN query-plan model (S12) — the UI-facing plan model, with **no
//! DataFusion dependency**. `engine::run_explain` walks DataFusion's own typed
//! `LogicalPlan` / `ExecutionPlan` trees and each operator's live `MetricsSet`
//! and builds these types directly, so there is **no plan-text parsing** anywhere.
//!
//! Split into focused submodules (all public items re-exported here, so callers
//! keep using `crate::plan::X`):
//! - [`tree`] — the operator tree: [`PlanKind`], [`PlanNode`], [`PlanTab`], [`QueryPlan`].
//! - [`metrics`] — typed [`Metric`]s, tier-3 grouping ([`metric_group`],
//!   [`group_color`]), tier-2 [`insights`], and the derived [`self_time_ms`].
//! - [`detail`] — the card's key/value [`detail_parts`] + tree [`guide_rails`].
//! - [`sql`] — EXPLAIN text helpers ([`is_explain`], [`as_explain`], [`split_name_detail`]).
//! - [`fmt`] — unit-aware formatters shared by the engine and the view.

mod detail;
mod fmt;
mod metrics;
mod sql;
mod tree;

pub use detail::{detail_parts, guide_rails, DetailPart};
pub use fmt::{fmt_bytes, fmt_int, fmt_ms, fmt_ns};
pub use metrics::{
    group_color, insights, metric_group, self_time_ms, Insight, InsightTone, Metric, MetricKind,
    METRIC_GROUPS,
};
pub use sql::{as_explain, is_explain, split_name_detail};
pub use tree::{PlanKind, PlanNode, PlanTab, QueryPlan};
