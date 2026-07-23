//! The EXPLAIN plan view (P2-05, EXPLAIN_PLAN_SPEC v3 — the v19 design mock): a toolbar
//! (Physical/Logical text segments, ANALYZE badge, Raw/Tree toggle) over an indented tree
//! of operator cards — or the raw plan text. All values arrive pre-typed and pre-labelled
//! from the engine (`strata_core::engine::plan`) — the view does no unit math.
//!
//! Split like the datagrid: this file owns the `explain_plan` theme component and the
//! [`ExplainPlan`] shell (toolbar + tree/raw body); [`node`] renders one railed operator
//! card with its three-tier metrics block; [`palette`] maps kind / metric / group / tone
//! onto the theme's colour fields; [`preview`] is the headless render harness.

use freya::components::use_theme;
use freya::prelude::*;

use strata_core::engine::plan::{guide_rails, PlanTab, QueryPlan};

use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::toggle_button::{ChangeEventData, ToggleButton};
use crate::components::typography::{Eyebrow, Readout};

mod node;
mod palette;
#[cfg(test)]
mod preview;

use node::plan_row;
use palette::PlanPalette;

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

#[cfg(test)]
mod tests {
    use strata_core::engine::plan::{PlanKind, PlanNode};

    use super::*;

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
