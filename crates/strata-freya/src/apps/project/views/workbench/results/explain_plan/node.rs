//! One operator of the tree: the railed row ([`plan_row`]), the kind-coloured card with its
//! parsed-detail grid and — under `EXPLAIN ANALYZE` — the three-tier metrics block
//! (spec §6): headline (rows · self-time · bytes · time-share bar) → non-zero insight
//! callouts → the collapsed, grouped metrics box. Plus the small [`PlanLink`] text-button
//! the expanders share.

use freya::prelude::*;

use strata_core::engine::plan::{
    detail_parts, fmt_int, insights, metric_group, DetailPart, PlanKind, PlanNode,
    METRIC_GROUPS,
};

use super::palette::PlanPalette;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Caption, Eyebrow, Meta, MonoValue, Path};

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

/// One row of the tree: a 22px rail column per ancestor level (lit only where the tree
/// continues — `guide_rails`, so a single node shows no dangling connectors), then the card.
/// `Content::Fit` + `fill_minimum` stretch the rails to the card's height.
pub fn plan_row(
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
}
