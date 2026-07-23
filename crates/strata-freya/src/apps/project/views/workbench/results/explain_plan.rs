//! The EXPLAIN plan view (P2-05, EXPLAIN_PLAN_SPEC v3 — the v19 design mock): a toolbar
//! (Physical/Logical text segments, ANALYZE badge, icon-only Raw/Tree toggle) over an
//! indented tree of operator cards — or the raw plan text. Each card parses the operator's
//! one-line `detail` into a key→value grid, and under `EXPLAIN ANALYZE` carries the
//! three-tier metrics block (spec §6): headline (rows · self-time · bytes · time-share bar)
//! → non-zero insight callouts → a collapsed, grouped metrics box. All values arrive
//! pre-typed and pre-labelled from the engine (`strata_core::engine::plan`) — the view does
//! no unit math.
//!
//! Themed by the `explain_plan` component: the sunken body + card surfaces, the hairline,
//! the text ramp, and the categorical operator palette (the same `type_*_color` fields the
//! datagrid carries). [`PlanPalette`] maps kind / metric / group / tone onto those fields,
//! mirroring the core's CSS-var palette (`PlanKind::color` & friends, which the Dioxus app
//! consumes directly) — the palette *values* live in the theme file, the mapping here.

use freya::components::use_theme;
use freya::prelude::*;

use strata_core::engine::plan::{
    detail_parts, fmt_int, guide_rails, insights, metric_group, DetailPart, MetricKind,
    PlanKind, PlanNode, PlanTab, QueryPlan, METRIC_GROUPS,
};

use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::toggle_button::{ChangeEventData, ToggleButton};
use crate::components::typography::{Caption, Eyebrow, Meta, MonoValue, Path, Readout};

define_theme!(
    %[component]
    pub ExplainPlan {
        %[fields]
        background: Color,
        card_background: Color,
        border_fill: Color,
        group_background: Color,
        insight_background: Color,
        color: Color,
        value_color: Color,
        key_color: Color,
        muted_color: Color,
        raw_color: Color,
        hot_color: Color,
        warm_color: Color,
        type_str_color: Color,
        type_num_color: Color,
        type_bool_color: Color,
        type_ts_color: Color,
        type_struct_color: Color,
        type_list_color: Color,
        type_map_color: Color,
    }
);

/// The resolved plan dress every card reads: the `explain_plan` component theme plus the
/// semantic sheet slots the palette borrows (accent · error · secondary text). The mapping
/// fns mirror the core's CSS-var palette exactly (`PlanKind::color`, `MetricKind::color`,
/// `group_color`, `InsightTone::color`) — one mapping source, two frontends.
#[derive(Clone, PartialEq)]
struct PlanPalette {
    theme: ExplainPlanTheme,
    accent: Color,
    error: Color,
    count_color: Color,
}

impl PlanPalette {
    /// A node's accent colour (core: `PlanKind::color`).
    fn kind(&self, kind: PlanKind) -> Color {
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
    fn metric(&self, kind: MetricKind) -> Color {
        match kind {
            MetricKind::Time => self.theme.warm_color,
            MetricKind::Bytes | MetricKind::Memory => self.theme.type_list_color,
            MetricKind::Count => self.count_color,
            MetricKind::Ratio => self.theme.type_str_color,
        }
    }

    /// A tier-3 group-header bar colour (core: `group_color`).
    fn group(&self, group: &str) -> Color {
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
    fn tone(&self, tone: strata_core::engine::plan::InsightTone) -> Color {
        use strata_core::engine::plan::InsightTone;
        match tone {
            InsightTone::Err => self.error,
            InsightTone::Warn => self.theme.warm_color,
            InsightTone::Ok => self.theme.type_str_color,
            InsightTone::Info => self.theme.type_list_color,
        }
    }
}

/// The tree the view actually shows for a selection: the selected tab, falling back to
/// whichever tree is present. Also drives the status bar's active-tab summary (`mod.rs`).
pub fn effective_tab(plan: &QueryPlan, tab: PlanTab) -> PlanTab {
    if plan.physical.is_empty() {
        PlanTab::Logical
    } else if plan.logical.is_empty() {
        PlanTab::Physical
    } else {
        tab
    }
}

/// The time-share bar's fill percentage: self-time over the tree max, floored at 3% so a
/// non-zero bar always reads (the design rule).
fn bar_pct(self_ms: f64, max_ms: f64) -> f32 {
    if max_ms <= 0.0 {
        return 0.0;
    }
    ((self_ms / max_ms) * 100.0).round().clamp(3.0, 100.0) as f32
}

/// Mono char-width estimate for the detail grid's key column (`Path` role, 11px JetBrains
/// Mono) — the Skia-side stand-in for CSS `max-content` (the datagrid's autofit precedent).
const DETAIL_CHAR_W: f32 = 6.6;

/// A 1px top-edge-only border — the metrics box's row rule.
fn top_border() -> BorderWidth {
    BorderWidth { top: 1., right: 0., bottom: 0., left: 0. }
}

/// The detail grid's key-column width: the widest key at the mono estimate (0 when no row
/// has a key — every part then spans the full width).
fn key_col_width(parts: &[DetailPart]) -> f32 {
    parts
        .iter()
        .filter(|p| p.has_key)
        .map(|p| p.key.chars().count())
        .max()
        .map(|n| n as f32 * DETAIL_CHAR_W)
        .unwrap_or(0.)
}

/// The plan body for one settled EXPLAIN: toolbar over the tree (or raw text). The tab slot
/// lives on the results pane (per-press, like the page number) so the status bar's summary
/// can read the same selection; the Raw flag is the [`ToggleButton`]'s own, mirrored here.
#[derive(PartialEq)]
pub struct ExplainPlan {
    plan: QueryPlan,
    tab: State<PlanTab>,
    theme: Option<ExplainPlanThemePartial>,
}

impl ExplainPlan {
    pub fn new(plan: QueryPlan, tab: State<PlanTab>) -> Self {
        Self { plan, tab, theme: None }
    }
}

impl Component for ExplainPlan {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, ExplainPlanThemePreference, "explain_plan");
        let app_theme = use_theme();
        let (toolbar_bg, accent, error, count_color) = {
            let c = &app_theme.read().colors;
            (c.background, c.primary, c.error, c.text_secondary)
        };
        let palette =
            PlanPalette { theme: theme.clone(), accent, error, count_color };

        let mut tab = self.tab;
        // The Raw/Tree flag: the ToggleButton owns the flip; this per-press mirror (the
        // results body is keyed on the press's nonce) picks which body renders.
        let mut raw = use_state(|| false);
        let raw_on = *raw.read();
        let eff = effective_tab(&self.plan, *tab.read());
        let physical = eff == PlanTab::Physical;
        let show_tabs = !self.plan.physical.is_empty() && !self.plan.logical.is_empty();
        let (nodes, raw_text) = match eff {
            PlanTab::Physical => (&self.plan.physical, &self.plan.physical_text),
            PlanTab::Logical => (&self.plan.logical, &self.plan.logical_text),
        };
        let max_ms = self.plan.max_ms();

        // ── toolbar (38px, aligned with the results toolbar) ──────────────────────────────
        let tabs = show_tabs.then(|| {
            SegmentedToggle::new()
                .child(
                    ToggleSegment::text("Physical")
                        .selected(physical)
                        .on_press(move |_| tab.set(PlanTab::Physical)),
                )
                .child(
                    ToggleSegment::text("Logical")
                        .selected(!physical)
                        .on_press(move |_| tab.set(PlanTab::Logical)),
                )
        });
        // Amber ANALYZE badge (physical tab only — the metrics live there): the design's
        // status-pill recipe, a 15% tint of its own colour.
        let badge = (self.plan.analyze && physical).then(|| {
            rect()
                .height(Size::px(22.))
                .padding((0., 12.))
                .corner_radius(6.)
                .background(theme.type_map_color.with_a(38))
                .center()
                .child(Eyebrow::new("ANALYZE").color(theme.type_map_color))
        });
        let raw_title = if raw_on { "Show the plan tree" } else { "Show the raw plan text" };
        // The standard toggle button (`toggle_button` theme): it flips, we echo the value
        // back through `toggle` (the Button-`enabled` recipe) — never computing the flip.
        let raw_toggle = ToggleButton::new()
            .toggle(raw_on)
            .title(raw_title)
            .on_change(move |e: Event<ChangeEventData>| raw.set(e.value))
            .child(Icon::new(IconName::Lines).size(15.));
        let toolbar = rect()
            .width(Size::fill())
            .height(Size::px(38.))
            .min_height(Size::px(38.))
            .content(Content::Flex)
            .background(toolbar_bg)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .padding((0., 8.))
                    .spacing(8.)
                    .content(Content::Flex)
                    .maybe_child(tabs)
                    .maybe_child(badge)
                    .child(rect().width(Size::flex(1.)))
                    .child(raw_toggle),
            )
            .child(Divider::horizontal().color(theme.border_fill));

        // ── body: the indented card tree, or the raw indent text ──────────────────────────
        // Keyed by the shown tree so a tab switch remounts the cards (their expand state
        // belongs to *that* tree's nodes, not to list positions).
        let body: Element = if raw_on {
            ScrollView::new()
                .child(
                    rect()
                        .padding(16.)
                        .child(Readout::new(raw_text.clone()).color(theme.raw_color).wrap()),
                )
                .into()
        } else {
            let rows = nodes.iter().enumerate().map(|(i, node)| {
                plan_row(node, &guide_rails(nodes, i), max_ms, &palette, i)
            });
            ScrollView::new()
                .child(
                    rect()
                        .key(if physical { "physical" } else { "logical" })
                        .width(Size::fill())
                        .vertical()
                        .padding(16.)
                        .spacing(8.)
                        .children(rows.collect::<Vec<_>>()),
                )
                .into()
        };

        rect()
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .background(theme.background)
            .child(toolbar)
            .child(rect().width(Size::fill()).height(Size::flex(1.)).child(body))
    }
}

/// One row of the tree: a 22px rail column per ancestor level (lit only where the tree
/// continues — `guide_rails`, so a single node shows no dangling connectors), then the card.
/// `Content::Fit` + `fill_minimum` stretch the rails to the card's height.
fn plan_row(
    node: &PlanNode,
    rails: &[bool],
    max_ms: f64,
    palette: &PlanPalette,
    index: usize,
) -> Element {
    let line = palette.theme.border_fill;
    let mut row = rect().width(Size::fill()).horizontal().content(Content::Fit);
    for on in rails {
        row = row.child(
            rect()
                .width(Size::px(22.))
                .height(Size::fill_minimum())
                .maybe(*on, |el| {
                    el.padding(Gaps::new(0., 11., 0., 10.)).child(
                        rect().width(Size::fill()).height(Size::fill()).background(line),
                    )
                }),
        );
    }
    row.child(
        PlanNodeCard {
            node: node.clone(),
            palette: palette.clone(),
            max_ms,
            key: DiffKey::None,
        }
        .key(&index),
    )
    .into()
}

/// One operator card: kind-coloured accent strip + head, the parsed-detail grid, and (under
/// ANALYZE) the three-tier metrics block. A component so each card owns its detail-expand /
/// metrics-collapse / show-zeros state.
#[derive(PartialEq)]
struct PlanNodeCard {
    node: PlanNode,
    palette: PlanPalette,
    max_ms: f64,
    key: DiffKey,
}

impl KeyExt for PlanNodeCard {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for PlanNodeCard {
    fn render(&self) -> impl IntoElement {
        let mut detail_open = use_state(|| false);
        let mut metrics_open = use_state(|| false);
        let mut show_zeros = use_state(|| false);

        let t = &self.palette.theme;
        let kind = self.palette.kind(self.node.kind);
        let show_metrics = !self.node.metrics.is_empty();
        let self_ms = self.node.self_ms.unwrap_or(0.0);
        let hot = show_metrics && self_ms >= self.max_ms * 0.6;
        let rows_label = self.node.rows.map(fmt_int);
        // Bytes headline is a source concept only.
        let bytes_label = (self.node.kind == PlanKind::Source)
            .then(|| {
                self.node
                    .metrics
                    .iter()
                    .find(|m| m.name == "bytes_scanned")
                    .map(|m| m.label.clone())
            })
            .flatten();
        let ins = insights(&self.node.metrics);
        let metric_total = self.node.metrics.len();
        let zero_count = self.node.metrics.iter().filter(|m| m.zero).count();

        // Parsed detail; collapsed shows the first two parts (the design rule).
        let all_parts = detail_parts(&self.node.detail);
        let detail_long = all_parts.len() > 2 || self.node.detail.chars().count() > 110;
        let shown: &[DetailPart] = if detail_long && !*detail_open.read() {
            &all_parts[..2.min(all_parts.len())]
        } else {
            &all_parts
        };
        let key_w = key_col_width(&all_parts);

        // ── head: kind square · mono name · HOTSPOT ───────────────────────────────────────
        let head = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.)
            .child(
                rect()
                    .width(Size::px(6.))
                    .height(Size::px(6.))
                    .corner_radius(1.5)
                    .background(kind),
            )
            .child(MonoValue::new(self.node.name.clone()).color(kind))
            .maybe(hot, |el| {
                el.child(
                    rect()
                        .padding((2., 4.))
                        .corner_radius(4.)
                        .background(t.hot_color.with_a(41))
                        .child(Eyebrow::new("HOTSPOT").color(t.hot_color)),
                )
            });

        // ── parsed detail: a key/value grid; bare fragments span both columns ─────────────
        let detail_grid = (!shown.is_empty()).then(|| {
            let rows = shown.iter().map(|part| {
                let value = Path::new(part.val.clone()).color(t.color).wrap();
                if part.has_key {
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .spacing(12.)
                        .child(Path::new(part.key.clone()).color(t.key_color).width(Size::px(key_w)))
                        .child(value.width(Size::fill()))
                        .into_element()
                } else {
                    value.width(Size::fill()).into_element()
                }
            });
            rect()
                .width(Size::fill())
                .margin(Gaps::new(4., 0., 0., 0.))
                .vertical()
                .spacing(2.)
                .children(rows.collect::<Vec<_>>())
        });
        let detail_toggle = detail_long.then(|| {
            let open = *detail_open.read();
            PlanLink {
                text: format!("{} Detail", if open { "▾" } else { "▸" }),
                color: t.muted_color,
                hover_color: self.palette.accent,
                on_press: (move |()| {
                    let v = !*detail_open.peek();
                    detail_open.set(v);
                })
                    .into(),
            }
        });

        // ── tier 1: rows · self-time · bytes · the time-share bar ─────────────────────────
        let tier1 = show_metrics.then(|| {
            let pct = bar_pct(self_ms, self.max_ms);
            let stats = rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Center)
                .content(Content::wrap_spacing(8.))
                .spacing(12.)
                .maybe(rows_label.is_some(), |el| {
                    el.child(
                        rect()
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(3.)
                            .child(Meta::new(rows_label.clone().unwrap_or_default()).color(t.value_color))
                            .child(Meta::new("rows").color(t.color)),
                    )
                })
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(4.)
                        .child(Icon::new(IconName::Clock).color(t.warm_color).size(12.))
                        .child(Meta::new(self.node.self_label.clone()).color(t.warm_color)),
                )
                .maybe_child(bytes_label.map(|b| Meta::new(b).color(t.muted_color)));
            let bar = rect()
                .width(Size::fill())
                .height(Size::px(4.))
                .corner_radius(2.)
                .background(t.border_fill)
                .overflow(Overflow::Clip)
                .child(rect().width(Size::percent(pct)).height(Size::fill()).background(kind));
            rect()
                .width(Size::fill())
                .margin(Gaps::new(8., 0., 0., 0.))
                .vertical()
                .spacing(8.)
                .child(stats)
                .child(bar)
        });

        // ── tier 2: non-zero insight callouts, tone-coloured pills ────────────────────────
        let tier2 = (show_metrics && !ins.is_empty()).then(|| {
            let pills = ins.iter().map(|i| {
                rect()
                    .padding((2., 8.))
                    .corner_radius(4.)
                    .background(t.insight_background)
                    .child(Meta::new(i.text.clone()).color(self.palette.tone(i.tone)))
                    .into_element()
            });
            rect()
                .width(Size::fill())
                .margin(Gaps::new(8., 0., 0., 0.))
                .horizontal()
                .content(Content::wrap_spacing(8.))
                .spacing(8.)
                .children(pills.collect::<Vec<_>>())
        });

        // ── tier 3: the full metrics box — grouped, collapsed, zeros hidden ───────────────
        let tier3 = show_metrics.then(|| {
            let open = *metrics_open.read();
            let toggle = PlanLink {
                text: format!("{} Metrics ({metric_total})", if open { "▾" } else { "▸" }),
                color: t.muted_color,
                hover_color: self.palette.accent,
                on_press: (move |()| {
                    let v = !*metrics_open.peek();
                    metrics_open.set(v);
                })
                    .into(),
            };
            let boxed = open.then(|| {
                let show_all = *show_zeros.read();
                let mut grid = rect()
                    .width(Size::fill())
                    .margin(Gaps::new(8., 0., 0., 0.))
                    .vertical()
                    .corner_radius(6.)
                    .border(Border::new().width(1.).fill(t.border_fill))
                    .overflow(Overflow::Clip);
                for group in METRIC_GROUPS {
                    let rows: Vec<_> = self
                        .node
                        .metrics
                        .iter()
                        .filter(|m| (show_all || !m.zero) && metric_group(&m.name) == group)
                        .collect();
                    if rows.is_empty() {
                        continue;
                    }
                    grid = grid.child(
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .padding((8., 12.))
                            .background(t.group_background)
                            .child(
                                rect()
                                    .width(Size::px(2.))
                                    .height(Size::px(10.))
                                    .corner_radius(1.)
                                    .background(self.palette.group(group)),
                            )
                            .child(Eyebrow::new(group.to_uppercase()).color(t.muted_color)),
                    );
                    for m in rows {
                        grid = grid.child(
                            rect()
                                .width(Size::fill())
                                .horizontal()
                                .cross_align(Alignment::Center)
                                .content(Content::Flex)
                                .spacing(16.)
                                .padding((4., 12.))
                                .border(Border::new().width(top_border()).fill(t.border_fill))
                                .maybe(m.zero, |el| el.opacity(0.55))
                                .child(Meta::new(m.name.clone()).color(t.color).width(Size::flex(1.)))
                                .child(Meta::new(m.label.clone()).color(self.palette.metric(m.kind))),
                        );
                    }
                }
                if zero_count > 0 {
                    let label = if show_all {
                        "hide zeros".to_string()
                    } else {
                        format!("show zeros ({zero_count})")
                    };
                    grid = grid.child(
                        rect()
                            .width(Size::fill())
                            .padding((8., 12.))
                            .background(t.group_background)
                            .border(Border::new().width(top_border()).fill(t.border_fill))
                            .child(PlanLink {
                                text: label,
                                color: t.muted_color,
                                hover_color: self.palette.accent,
                                on_press: (move |()| {
                                    let v = !*show_zeros.peek();
                                    show_zeros.set(v);
                                })
                                    .into(),
                            }),
                    );
                }
                grid
            });
            rect()
                .width(Size::fill())
                .margin(Gaps::new(8., 0., 0., 0.))
                .vertical()
                .child(toggle)
                .maybe_child(boxed)
        });

        // The card: 1px hairline + clipped kind-coloured 3px accent strip (Freya's `Border`
        // is all-sides — the strip child is the border-left idiom), then the content column.
        rect()
            .width(Size::fill())
            .corner_radius(8.)
            .border(Border::new().width(1.).fill(t.border_fill))
            .background(t.card_background)
            .overflow(Overflow::Clip)
            .horizontal()
            .content(Content::Fit)
            .child(rect().width(Size::px(3.)).height(Size::fill_minimum()).background(kind))
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .padding((8., 12.))
                    .child(head)
                    .maybe_child(detail_grid)
                    .maybe_child(detail_toggle)
                    .maybe_child(tier1)
                    .maybe_child(tier2)
                    .maybe_child(tier3),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// A small inline text-button ("Detail", "Metrics (24) ▸", "show zeros") — muted rest,
/// accent hover, no chrome.
#[derive(PartialEq)]
struct PlanLink {
    text: String,
    color: Color,
    hover_color: Color,
    on_press: EventHandler<()>,
}

impl Component for PlanLink {
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let on_press = self.on_press.clone();
        let color = if *hovered.read() { self.hover_color } else { self.color };
        rect()
            .margin(Gaps::new(8., 0., 0., 0.))
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |_| on_press.call(()))
            .child(Caption::new(self.text.clone()).color(color))
    }
}

/// Headless preview harness: renders this surface — real component, real `midnight` theme —
/// to `target/plan-preview.png` for eyeballing against the design canvas. Ignored by default
/// (it writes a file, asserts nothing):
/// `cargo test -p strata-freya plan_preview -- --ignored`.
#[cfg(test)]
mod preview {
    use freya_testing::TestingRunner;
    use strata_core::engine::plan::{fmt_ms, self_time_ms, Metric, MetricKind};

    use super::*;

    fn m(name: &str, value: u64, kind: MetricKind) -> Metric {
        Metric {
            name: name.to_string(),
            value,
            kind,
            label: kind.format(value),
            zero: value == 0,
        }
    }

    fn node(
        name: &str,
        detail: &str,
        kind: PlanKind,
        depth: usize,
        rows: Option<u64>,
        metrics: Vec<Metric>,
    ) -> PlanNode {
        let self_ms = self_time_ms(kind, &metrics);
        PlanNode {
            name: name.to_string(),
            detail: detail.to_string(),
            kind,
            depth,
            rows,
            self_ms,
            self_label: self_ms.map(fmt_ms).unwrap_or_default(),
            metrics,
        }
    }

    /// A compact cut of the spec §3 reference ANALYZE plan: sort → agg → exchange → join →
    /// two sibling scans, exercising rails, hotspots, insights, zeros, and the long detail.
    fn fixture() -> QueryPlan {
        use MetricKind::*;
        QueryPlan {
            physical: vec![
                node("SortExec", "TopK(fetch=20), expr=[cnt@2 DESC]", PlanKind::Sort, 0,
                    Some(4),
                    vec![m("output_rows", 4, Count), m("elapsed_compute", 156_000, Time),
                        m("row_replacements", 4, Count)]),
                node("AggregateExec", "mode=FinalPartitioned, gby=[country@0, action@1], aggr=[count(1)]",
                    PlanKind::Agg, 1, Some(4),
                    vec![m("output_rows", 4, Count), m("elapsed_compute", 4_790_000, Time),
                        m("peak_mem_used", 3481, Memory), m("spill_count", 0, Count)]),
                node("RepartitionExec", "partitioning=Hash([user_id@0], 10), input_partitions=10",
                    PlanKind::Exchange, 2, None,
                    vec![m("repartition_time", 4_300_000, Time), m("send_time", 19_000, Time),
                        m("fetch_time", 256_000_000, Time)]),
                node("HashJoinExec", "mode=Partitioned, join_type=Inner, on=[(user_id@0, user_id@0)]",
                    PlanKind::Join, 3, Some(4),
                    vec![m("output_rows", 4, Count), m("build_time", 216_000, Time),
                        m("join_time", 146_000, Time), m("build_mem_used", 2148, Memory),
                        m("build_input_rows", 4, Count), m("input_rows", 5, Count),
                        m("output_batches", 4, Count)]),
                node("ParquetExec",
                    "file_groups={1 group: [[…/events/year=2024/month=01/data.parquet, …/events/year=2024/month=02/data.parquet]]}, projection=[user_id, action, amount], predicate=amount@3 IS NOT NULL",
                    PlanKind::Source, 4, Some(7),
                    vec![m("output_rows", 7, Count), m("time_elapsed_processing", 15_594_334, Time),
                        m("time_elapsed_scanning_total", 17_147_249, Time),
                        m("metadata_load_time", 22_353_002, Time), m("bytes_scanned", 605, Bytes),
                        m("row_groups_matched_statistics", 2, Count),
                        m("row_groups_pruned_statistics", 0, Count),
                        m("pushdown_rows_matched", 0, Count), m("pushdown_rows_pruned", 0, Count),
                        m("file_open_errors", 0, Count), m("file_scan_errors", 0, Count)]),
                node("ParquetExec", "file_groups={1 group: [[…/users/users.parquet]]}, projection=[user_id, country]",
                    PlanKind::Source, 4, Some(5),
                    vec![m("output_rows", 5, Count), m("time_elapsed_processing", 578_000, Time),
                        m("metadata_load_time", 3_200_000, Time), m("bytes_scanned", 210, Bytes),
                        m("file_open_errors", 0, Count)]),
            ],
            logical: vec![
                node("Sort", "cnt DESC NULLS FIRST, fetch=20", PlanKind::Sort, 0, None, vec![]),
                node("Aggregate", "groupBy=[[country, action]], aggr=[[count(1)]]", PlanKind::Agg, 1, None, vec![]),
                node("TableScan", "events projection=[user_id, action, amount]", PlanKind::Source, 2, None, vec![]),
            ],
            physical_text: "SortExec: TopK(fetch=20)\n  AggregateExec: mode=FinalPartitioned\n    RepartitionExec: partitioning=Hash([user_id@0], 10)\n      HashJoinExec: mode=Partitioned, join_type=Inner\n        ParquetExec: file_groups={1 group}\n        ParquetExec: file_groups={1 group}".into(),
            logical_text: "Sort: cnt DESC NULLS FIRST, fetch=20\n  Aggregate: groupBy=[[country, action]]\n    TableScan: events".into(),
            analyze: true,
        }
    }

    fn app() -> impl IntoElement {
        use_init_theme(|| crate::theme::strata_theme(&strata_core::theme::load("midnight")));
        let tab = use_state(PlanTab::default);
        ExplainPlan::new(fixture(), tab)
    }

    #[test]
    #[ignore = "writes target/plan-preview.png for eyeballing; run explicitly"]
    fn plan_preview() {
        let (mut runner, _) = TestingRunner::new(app, (960., 900.).into(), |_| {}, 1.);
        runner.sync_and_update();
        runner.render_to_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/plan-preview.png"
        ));
        // Expanded states: SortExec's Metrics box open + AggregateExec's Detail expanded
        // (the second coordinate accounts for the first box's ~160px reflow).
        runner.click_cursor((63., 169.));
        runner.click_cursor((70., 424.));
        runner.sync_and_update();
        runner.render_to_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/plan-preview-open.png"
        ));
        // The Logical tab.
        runner.click_cursor((114., 18.));
        runner.sync_and_update();
        runner.render_to_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/plan-preview-logical.png"
        ));
        // The Raw text view (back on Physical).
        runner.click_cursor((44., 18.));
        runner.click_cursor((937., 18.));
        runner.sync_and_update();
        runner.render_to_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/plan-preview-raw.png"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(key: &str, val: &str, has_key: bool) -> DetailPart {
        DetailPart { key: key.into(), val: val.into(), has_key }
    }

    #[test]
    fn bar_floors_nonzero_and_clamps() {
        assert_eq!(bar_pct(15.6, 15.6), 100.0);
        assert_eq!(bar_pct(0.001, 15.6), 3.0);
        assert_eq!(bar_pct(7.8, 15.6), 50.0);
        assert_eq!(bar_pct(1.0, 0.0), 0.0);
    }

    #[test]
    fn key_column_fits_the_widest_key() {
        let parts = vec![
            part("mode", "Partitioned", true),
            part("join_type", "Inner", true),
            part("", "TableScan: t", false),
        ];
        assert_eq!(key_col_width(&parts), 9.0 * DETAIL_CHAR_W);
        // No keyed part → no key column.
        assert_eq!(key_col_width(&[part("", "bare", false)]), 0.0);
    }

    #[test]
    fn effective_tab_falls_back_to_the_present_tree() {
        let node = PlanNode {
            name: "X".into(),
            detail: String::new(),
            kind: PlanKind::Util,
            depth: 0,
            rows: None,
            self_ms: None,
            self_label: String::new(),
            metrics: Vec::new(),
        };
        let both = QueryPlan {
            logical: vec![node.clone()],
            physical: vec![node.clone()],
            ..Default::default()
        };
        assert_eq!(effective_tab(&both, PlanTab::Logical), PlanTab::Logical);
        let physical_only = QueryPlan { physical: vec![node.clone()], ..Default::default() };
        assert_eq!(effective_tab(&physical_only, PlanTab::Logical), PlanTab::Physical);
        let logical_only = QueryPlan { logical: vec![node], ..Default::default() };
        assert_eq!(effective_tab(&logical_only, PlanTab::Physical), PlanTab::Logical);
    }
}
