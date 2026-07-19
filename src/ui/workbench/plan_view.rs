//! The EXPLAIN plan view (S12, v3 — matches the v19 design mock): a toolbar
//! (Physical/Logical tabs, ANALYZE badge, icon-only Raw/Tree toggle) over an
//! indented tree of operator cards — or the raw plan text. Each card parses the
//! operator's one-line `detail` into a key→value definition grid, and under
//! `EXPLAIN ANALYZE` carries a three-tier metrics block (EXPLAIN_PLAN_SPEC §6):
//! headline (rows · self-time · bytes · time-share bar) → non-zero insight
//! callouts → a collapsed, grouped metrics box. All values arrive pre-typed and
//! pre-labelled from the engine (`crate::plan`) — the view does no unit math.

use dioxus::prelude::*;

use crate::action::{dispatch, Action};
use crate::session::WorkspaceId;
use crate::ui::components::{
    Badge, Dot, Icon, IconButton, IconButtonVariant, Micro, MonoValue, Readout, Segment,
    SegmentOption, Spacer,
};
use crate::ui::icons::{IconName, IconSize};
use strata_core::engine::plan::{PlanKind, PlanTab};

#[component]
pub fn PlanView(ws_id: WorkspaceId) -> Element {
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

    // Icon-only Raw/Tree toggle (consistent with the results toolbar's icon buttons);
    // the title carries the action since there's no label. The operator-count summary
    // lives in the results status bar, not this header.
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
                        on_select: move |v: String| dispatch(Action::SetPlanTab(
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
                    onclick: move |_| dispatch(Action::TogglePlanRaw),
                }
            }
            if raw {
                div { class: "plan-body ps-scroll",
                    Readout { class: "plan-raw mono", "{raw_text}" }
                }
            } else {
                div { class: "plan-body ps-scroll",
                    for (i, n) in nodes.iter().enumerate() {
                        PlanNodeCard {
                            key: "{i}",
                            node: n.clone(),
                            rails: crate::plan::guide_rails(nodes, i),
                            max_ms,
                        }
                    }
                }
            }
        }
    }
}

/// One operator card: tree rails + a kind-coloured card with a parsed-detail grid
/// and (under ANALYZE) the three-tier metrics block. A component (not a plain fn)
/// so each card owns its detail-expand / metrics-collapse / show-zeros state.
#[component]
fn PlanNodeCard(node: crate::plan::PlanNode, rails: Vec<bool>, max_ms: f64) -> Element {
    let mut detail_open = use_signal(|| false);
    let mut full_open = use_signal(|| false);
    let mut show_zeros = use_signal(|| false);

    let color = node.kind.color();
    let show_metrics = !node.metrics.is_empty();
    let self_ms = node.self_ms.unwrap_or(0.0);
    let hot = show_metrics && self_ms >= max_ms * 0.6;
    let bar_pct = if show_metrics && max_ms > 0.0 {
        ((self_ms / max_ms) * 100.0).round().clamp(3.0, 100.0)
    } else {
        0.0
    };
    let rows_label = node.rows.map(crate::plan::fmt_int);
    // Bytes headline is a source concept only.
    let bytes_label = if node.kind == PlanKind::Source {
        node.metrics
            .iter()
            .find(|m| m.name == "bytes_scanned")
            .map(|m| m.label.clone())
    } else {
        None
    };
    // (text, colour) — resolve the tone colour here; rsx interpolation can't call it.
    let ins: Vec<(String, &'static str)> = crate::plan::insights(&node.metrics)
        .into_iter()
        .map(|i| (i.text, i.tone.color()))
        .collect();
    let metric_total = node.metrics.len();
    let zero_count = node.metrics.iter().filter(|m| m.zero).count();

    // Detail → key/value parts; collapsed shows the first two (design rule).
    let all_parts = crate::plan::detail_parts(&node.detail);
    let detail_long = all_parts.len() > 2 || node.detail.chars().count() > 110;
    let detail_shown: Vec<crate::plan::DetailPart> = if detail_long && !detail_open() {
        all_parts.iter().take(2).cloned().collect()
    } else {
        all_parts.clone()
    };
    let detail_caret = if detail_open() { "▾" } else { "▸" };

    // Tier-3 grouped grid — pre-format each row (rsx interpolation is a format
    // string; can't call helpers inline). Fixed group order from the design.
    let show_all = show_zeros();
    let groups: Vec<(&'static str, &'static str, Vec<(String, String, &'static str, bool)>)> =
        crate::plan::METRIC_GROUPS
            .iter()
            .filter_map(|g| {
                let rows: Vec<(String, String, &'static str, bool)> = node
                    .metrics
                    .iter()
                    .filter(|m| show_all || !m.zero)
                    .filter(|m| crate::plan::metric_group(&m.name) == *g)
                    .map(|m| (m.name.clone(), m.label.clone(), m.kind.color(), m.zero))
                    .collect();
                (!rows.is_empty()).then_some((*g, crate::plan::group_color(g), rows))
            })
            .collect();
    let metrics_caret = if full_open() { "▾" } else { "▸" };
    let zeros_label = if show_all {
        "hide zeros".to_string()
    } else {
        format!("show zeros ({zero_count})")
    };

    rsx! {
        div { class: "plan-row",
            for (l, on) in rails.iter().enumerate() {
                div { key: "{l}", class: if *on { "plan-guide on" } else { "plan-guide" } }
            }
            div { class: "plan-card", style: "border-left-color:{color};",
                div { class: "plan-card-head",
                    Dot { color: "{color}", square: true, size: 6 }
                    MonoValue { class: "plan-name mono", style: "color:{color};", "{node.name}" }
                    if hot {
                        Micro { class: "plan-hot mono", "HOTSPOT" }
                    }
                }
                // Parsed detail — a key/value definition grid.
                if !detail_shown.is_empty() {
                    div { class: "plan-detail-grid",
                        for (di, dp) in detail_shown.iter().enumerate() {
                            if dp.has_key {
                                span { key: "k{di}", class: "plan-dk mono", "{dp.key}" }
                                span { key: "v{di}", class: "plan-dv mono", "{dp.val}" }
                            } else {
                                span { key: "f{di}", class: "plan-dv-full mono", "{dp.val}" }
                            }
                        }
                    }
                    if detail_long {
                        button {
                            class: "plan-link plan-detail-toggle",
                            onclick: move |_| { let v = !detail_open(); detail_open.set(v); },
                            span { class: "plan-caret", "{detail_caret}" }
                            "Detail"
                        }
                    }
                }
                if show_metrics {
                    // Tier 1 — headline: rows · self-time · bytes · time-share bar.
                    div { class: "plan-tier1",
                        div { class: "plan-stats",
                            if let Some(r) = rows_label.clone() {
                                span { class: "plan-stat",
                                    span { class: "mono plan-stat-v", "{r}" }
                                    " rows"
                                }
                            }
                            span { class: "plan-stat plan-stat-time",
                                Icon { name: IconName::Clock, size: IconSize::Xs }
                                span { class: "mono plan-stat-v", "{node.self_label}" }
                            }
                            if let Some(b) = bytes_label.clone() {
                                span { class: "plan-stat plan-stat-bytes mono", "{b}" }
                            }
                        }
                        div { class: "plan-bar",
                            div { class: "plan-bar-fill", style: "width:{bar_pct}%;background:{color};" }
                        }
                    }
                    // Tier 2 — insight callouts (non-zero signal only), tone-coloured.
                    if !ins.is_empty() {
                        div { class: "plan-insights",
                            for (k, (itext, icolor)) in ins.iter().enumerate() {
                                span {
                                    key: "{k}",
                                    class: "plan-insight mono",
                                    style: "color:{icolor};",
                                    "{itext}"
                                }
                            }
                        }
                    }
                    // Tier 3 — full metrics, grouped + collapsed by default.
                    div { class: "plan-metrics-wrap",
                        button {
                            class: "plan-link plan-metrics-toggle",
                            onclick: move |_| { let v = !full_open(); full_open.set(v); },
                            span { class: "plan-caret", "{metrics_caret}" }
                            "Metrics ({metric_total})"
                        }
                        if full_open() {
                            div { class: "plan-metrics-box",
                                for (g, gcolor, rows) in groups.iter() {
                                    div { key: "h{g}", class: "plan-grp-head",
                                        span { class: "plan-grp-bar", style: "background:{gcolor};" }
                                        span { class: "plan-grp-name mono", "{g}" }
                                    }
                                    for (mname, mlabel, mcolor, mzero) in rows.iter() {
                                        div {
                                            key: "{mname}",
                                            class: if *mzero { "plan-mrow zero" } else { "plan-mrow" },
                                            span { class: "plan-mname mono", "{mname}" }
                                            span { class: "plan-mval mono", style: "color:{mcolor};", "{mlabel}" }
                                        }
                                    }
                                }
                                if zero_count > 0 {
                                    button {
                                        class: "plan-zeros mono",
                                        onclick: move |_| { let v = !show_zeros(); show_zeros.set(v); },
                                        "{zeros_label}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
