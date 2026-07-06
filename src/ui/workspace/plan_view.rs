//! The EXPLAIN plan view (S12): a toolbar (Physical/Logical tabs, summary, ANALYZE
//! badge, Raw/Tree toggle) over an indented tree of operator cards — or the raw
//! plan text. ANALYZE forces the physical "Plan with Metrics" and adds per-node
//! rows/time, a time-share bar, and a HOTSPOT badge for the slowest operators.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::state::{AppState, PlanTab};

#[component]
pub(crate) fn PlanView() -> Element {
    let state = use_context::<Signal<AppState>>();
    let (plan, tab, raw) = {
        let s = state.read();
        let Some(plan) = s.plan.clone() else {
            return rsx! { div {} };
        };
        (plan, s.plan_tab, s.plan_raw)
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
    let nodes = if eff_physical { &plan.physical } else { &plan.logical };
    let raw_text = if eff_physical { &plan.physical_text } else { &plan.logical_text };
    let max_ms = plan.max_ms();

    // Summary reflects the *active* tab: the logical tab never shows metrics, even
    // during an ANALYZE run.
    let summary = format!(
        "{} · {} operators",
        if !eff_physical {
            "Logical plan"
        } else if analyze {
            "Plan with metrics"
        } else {
            "Physical plan"
        },
        nodes.len()
    );

    let phys_cls = if eff_physical { "plan-tab on" } else { "plan-tab" };
    let log_cls = if !eff_physical { "plan-tab on" } else { "plan-tab" };
    let raw_label = if raw { "Tree" } else { "Raw" };

    rsx! {
        div { class: "res-plan",
            div { class: "plan-tb",
                if show_tabs {
                    div { class: "plan-tabs",
                        button { class: "{phys_cls}", onclick: move |_| dispatch(state, Action::SetPlanTab(PlanTab::Physical)), "Physical" }
                        button { class: "{log_cls}", onclick: move |_| dispatch(state, Action::SetPlanTab(PlanTab::Logical)), "Logical" }
                    }
                }
                span { class: "plan-summary mono", "{summary}" }
                if analyze && eff_physical {
                    span { class: "plan-analyze mono", "ANALYZE" }
                }
                div { class: "spacer" }
                button { class: "btn sm", style: "height:28px;", onclick: move |_| dispatch(state, Action::TogglePlanRaw),
                    svg { width: "13", height: "13", "viewBox": "0 0 24 24", fill: "none",
                        stroke: "currentColor", "stroke-width": "1.8", "stroke-linecap": "round", "stroke-linejoin": "round",
                        path { d: "M4 7h16M4 12h10M4 17h13" }
                    }
                    "{raw_label}"
                }
            }
            if raw {
                div { class: "plan-body ps-scroll",
                    pre { class: "plan-raw mono", "{raw_text}" }
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
fn plan_node_card(
    n: &crate::plan::PlanNode,
    idx: usize,
    analyze: bool,
    max_ms: f64,
) -> Element {
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
                    span { class: "plan-sq", style: "background:{color};" }
                    span { class: "plan-name mono", style: "color:{color};", "{n.name}" }
                    if hot {
                        span { class: "plan-hot mono", "HOTSPOT" }
                    }
                }
                if !n.detail.is_empty() {
                    div { class: "plan-detail mono", "{n.detail}" }
                }
                if has_metrics {
                    div { class: "plan-metrics",
                        div { class: "plan-metrics-row",
                            span { class: "plan-rows mono", "{rows_label} rows" }
                            span { class: "plan-ms mono", "{n.ms_label}" }
                            if !n.extra.is_empty() {
                                span { class: "plan-extra mono", "{n.extra}" }
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
