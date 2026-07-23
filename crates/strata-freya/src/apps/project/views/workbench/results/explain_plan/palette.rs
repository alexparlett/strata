//! The plan view's colour resolution: [`PlanPalette`] bundles the resolved `explain_plan`
//! theme with the semantic sheet slots it borrows, and maps every categorical dimension —
//! operator kind, metric unit, tier-3 group, insight tone — onto them. The mapping mirrors
//! the core's CSS-var palette (`PlanKind::color` & friends, which the Dioxus app consumes
//! directly) — the palette *values* live in the theme file, the mapping here.

use freya::prelude::*;

use strata_core::engine::plan::{InsightTone, MetricKind, PlanKind};

use super::ExplainPlanTheme;

/// The resolved plan dress every card reads: the `explain_plan` component theme plus the
/// semantic sheet slots the palette borrows (accent · error · secondary text). The mapping
/// fns mirror the core's CSS-var palette exactly (`PlanKind::color`, `MetricKind::color`,
/// `group_color`, `InsightTone::color`) — one mapping source, two frontends.
#[derive(Clone, PartialEq)]
pub struct PlanPalette {
    pub theme: ExplainPlanTheme,
    pub accent: Color,
    pub error: Color,
    pub count_color: Color,
}

impl PlanPalette {
    /// A node's accent colour (core: `PlanKind::color`).
    pub fn kind(&self, kind: PlanKind) -> Color {
        match kind {
            PlanKind::Source => self.theme.type_str_color,
            PlanKind::Join => self.theme.type_bool_color,
            PlanKind::Exchange => self.theme.type_num_color,
            PlanKind::Agg => self.theme.type_ts_color,
            PlanKind::Sort => self.theme.type_struct_color,
            PlanKind::Proj => self.accent,
            PlanKind::Limit => self.theme.type_map_color,
            PlanKind::Util => self.theme.color,
        }
    }

    /// A tier-3 value colour (core: `MetricKind::color`).
    pub fn metric(&self, kind: MetricKind) -> Color {
        match kind {
            MetricKind::Time => self.theme.warm_color,
            MetricKind::Bytes | MetricKind::Memory => self.theme.type_list_color,
            MetricKind::Count => self.count_color,
            MetricKind::Ratio => self.theme.type_str_color,
        }
    }

    /// A tier-3 group-header bar colour (core: `group_color`).
    pub fn group(&self, group: &str) -> Color {
        match group {
            "Output" => self.accent,
            "Time" => self.theme.warm_color,
            "I/O" => self.theme.type_str_color,
            "Pruning" | "Join" => self.theme.type_bool_color,
            "Memory & spill" => self.theme.type_list_color,
            "Exchange" => self.theme.type_num_color,
            "Errors" => self.error,
            _ => self.theme.color,
        }
    }

    /// A tier-2 insight tone colour (core: `InsightTone::color`).
    pub fn tone(&self, tone: InsightTone) -> Color {
        match tone {
            InsightTone::Err => self.error,
            InsightTone::Warn => self.theme.warm_color,
            InsightTone::Ok => self.theme.type_str_color,
            InsightTone::Info => self.theme.type_list_color,
        }
    }
}
