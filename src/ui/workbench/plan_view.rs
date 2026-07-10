//! The EXPLAIN plan view (S12): a toolbar (Physical/Logical toggle, ANALYZE badge,
//! icon-only Raw/Tree toggle) over an indented tree of operator cards — or the raw
//! plan text. ANALYZE forces the physical "Plan with Metrics" and adds per-node
//! rows/time, a time-share bar, and a HOTSPOT badge for the slowest operators.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::plan::PlanTab;
use crate::session::WorkspaceId;
use crate::state::AppState;
use crate::ui::components::{
    Badge, Dot, IconButton, IconButtonVariant, Meta, Micro, MonoValue, Path, Readout, Segment,
    SegmentOption, Spacer,
};
use crate::ui::icons::{IconName, IconSize};

#[component]
pub(crate) fn PlanView(ws_id: WorkspaceId) -> Element {
    let state = use_context::<Signal<AppState>>();
    let (plan, tab, raw) = {
        let Some(entry) = crate::runs::RUNS.resolve().get(ws_id) else {
            return rsx! { div {} };
        };
        let run = entry.read();
        let Some(plan) = run.plan.clone() else {
            return rsx! { div {} };
        };
        (plan, run.plan_tab, run.plan_raw)
    };

    let analyze = plan.analyze;
    let has_logical = !plan.logical.is_empty();
    let has_physical = !plan.physical.is_empty();
    // Honour the selected tab, falling back to whichever tree is present. ANALYZE
    // defaults to physical (the metrics plan) but the logical tab stays available.
    let eff_physical = if !has_physical {
        false
    } else if !has_logical {
        true
    } else {
        tab == PlanTab::Physical
    };
    // Offer the Physical/Logical switch whenever both trees exist — incl. ANALYZE.
    let show_tabs = has_logical && has_physical;
    let nodes = if eff_physical {
        &plan.physical
    } else {
        &plan.logical
    };
    let raw_text = if eff_physical {
        &plan.physical_text
    } else {
        &plan.logical_text
    };
    let max_ms = plan.max_ms();

    // (The "N operators · mode" summary lives in the results status bar now, not the
    // plan header — matches the design.)
    // Icon-only Raw/Tree toggle (consistent with the results toolbar's icon buttons);
    // the title carries the action since there's no label.
    let raw_title = if raw {
        "Show the plan tree"
    } else {
        "Show the raw plan text"
    };

    rsx! {
        div { class: "res-plan",
            div { class: "plan-tb",
                if show_tabs {
                    Segment {
                        value: if eff_physical { "physical" } else { "logical" },
                        compact: true,
                        on_select: move |v: String| dispatch(state, Action::SetPlanTab(
                            if v == "logical" { PlanTab::Logical } else { PlanTab::Physical },
                        )),
                        options: vec![
                            SegmentOption::new("physical", "Physical"),
                            SegmentOption::new("logical", "Logical"),
                        ],
                    }
                }
                if analyze && eff_physical {
                    Badge { color: "var(--t-map)", "ANALYZE" }
                }
                Spacer {}
                IconButton { icon: IconName::Lines,
                    variant: IconButtonVariant::Toggle,
                    on: raw,
                    title: "{raw_title}",
                    onclick: move |_| dispatch(state, Action::TogglePlanRaw),
                }
            }
            if raw {
                div { class: "plan-body ps-scroll",
                    Readout { class: "plan-raw mono", "{raw_text}" }
                }
            } else {
                div { class: "plan-body ps-scroll",
                    for (i, n) in nodes.iter().enumerate() {
                        {plan_node_card(n, i, analyze, max_ms)}
                    }
                }
            }
        }
    }
}

/// One operator card in the plan tree, indented by depth and coloured by kind.
/// A plain fn (called once per node) — no hooks, so no need for a component.
fn plan_node_card(n: &crate::plan::PlanNode, idx: usize, analyze: bool, max_ms: f64) -> Element {
    let color = n.kind.color();
    let indent = n.depth * 22;
    let has_metrics = analyze && n.ms_val.is_some();
    let ms = n.ms_val.unwrap_or(0.0);
    let hot = analyze && ms >= max_ms * 0.6;
    let bar_pct = if has_metrics {
        ((ms / max_ms) * 100.0).round().max(3.0)
    } else {
        0.0
    };
    let rows_label = n.rows.map(fmt_int).unwrap_or_default();

    rsx! {
        div { key: "p{idx}", class: "plan-row", style: "padding-left:{indent}px;",
            div { class: "plan-card", style: "border-left-color:{color};",
                div { class: "plan-card-head",
                    Dot { color: "{color}", square: true, size: 6 }
                    MonoValue { class: "plan-name mono", style: "color:{color};", "{n.name}" }
                    if hot {
                        Micro { class: "plan-hot mono", "HOTSPOT" }
                    }
                }
                if !n.detail.is_empty() {
                    Path { class: "plan-detail mono", "{n.detail}" }
                }
                if has_metrics {
                    div { class: "plan-metrics",
                        div { class: "plan-metrics-row",
                            Meta { class: "plan-rows mono", "{rows_label} rows" }
                            Meta { class: "plan-ms mono", "{n.ms_label}" }
                            if !n.extra.is_empty() {
                                Meta { class: "plan-extra mono", "{n.extra}" }
                            }
                        }
                        div { class: "plan-bar",
                            div { class: "plan-bar-fill", style: "width:{bar_pct}%;background:{color};" }
                        }
                    }
                }
            }
        }
    }
}

/// Group a non-negative integer with thin thousands separators (e.g. 48213 →
/// "48,213") for the plan's row counts.
fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
