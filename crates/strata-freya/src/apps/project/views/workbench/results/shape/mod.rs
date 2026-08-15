//! The **Shape panel** (Chart 09): the chart-side aggregation UX, in a surface of its own.
//!
//! A modal working panel — `DBeaver`'s Grouping panel is the precedent — that composes
//! **visible SQL** over the settled run: group-by columns with `date_bin` strides,
//! per-measure aggregates, an explicit `ORDER BY` — and opens it **unrun** in a new tab the
//! user owns. It never replaces the current buffer, never runs anything itself, and keeps no
//! state of its own past the dialog: the SQL in the new tab is the only artifact.
//!
//! This is the placement `docs/CHART_SPEC.md` §8 invited: the cut *Aggregate in SQL* press
//! was sound mechanism on the wrong surface (the encoder strip). The refusal overlays keep
//! no control behind them; the trigger is the results toolbar's own action, on both bodies,
//! and when the press comes from the Chart view the form arrives seeded from the resolved
//! encoding — the cut press's "composed from the encoding" value, without its placement.
//!
//! The card is the panel's own, on the shared [`Modal`] base — a working panel is not a
//! confirm, so it does not wear the 420px confirm card; what it shares with one is how a
//! modal behaves (overlay, backdrop, Esc as a close request), which is exactly what the base
//! carries.

mod compose;

use std::collections::HashSet;

use freya::components::{MenuItem, ScrollView, Select, SelectThemePartial};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::{ChartRole, ColumnInfo, Origin, TabId};

use self::compose::{
    compose, GroupBy, GroupPick, MeasurePick, ShapeForm, ShapeOrder, SqlAgg, Stride,
};
use crate::apps::project::state::{Chan, SessionState};
use crate::components::dialog::DialogHeader;
use crate::components::divider::Divider;
use crate::components::form::{Form, Row};
use crate::components::icon::IconName;
use crate::components::metrics::{ACTION_HEIGHT, R_4, SP_1, SP_2, SP_3, SP_4, SP_5, SP_6};
use crate::components::modal::Modal;
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};
use crate::components::typography::{Caption, Control, Eyebrow, MonoValue, Title};
use crate::theme::{use_roles, Role};

/// The panel's own proportions: a working surface, not a confirm — wider than the confirm
/// card and tall enough that a real result's columns read as a list, capped to the window.
const CARD_WIDTH: f32 = 560.;
const CARD_HEIGHT: f32 = 540.;
/// A row's control box — the strip's `Select` recipe at the form's own width.
const CONTROL_WIDTH: f32 = 220.;

/// What the chart body seeds the form with when the press came from the Chart view: the
/// resolved encoding's category channels and its measures, by name.
#[derive(Clone, PartialEq, Debug)]
pub struct ShapeSeed {
    pub groups: Vec<String>,
    pub measures: Vec<String>,
}

/// What the Shape press is about: the settled run the panel composes over.
///
/// `sql` is the SQL that **produced the settled result** — the press's own `QuerySpec`,
/// never the live buffer (Chart 04 settled this for the cut press) — and a settled rows
/// result is one statement by construction (`sql::validate` refuses a multi-statement Run).
#[derive(Clone, PartialEq, Debug)]
pub struct ShapeTarget {
    pub tab: TabId,
    pub sql: String,
    /// The result's schema — what the form's rows are built from, roles and all.
    pub columns: Vec<ColumnInfo>,
    pub seed: Option<ShapeSeed>,
}

/// The Shape panel, mounted at the project root beside the other dialogs and watching the
/// same kind of slot: a press elsewhere fills it, either outcome clears it.
#[derive(PartialEq)]
pub struct ShapeDialog {
    pub target: State<Option<ShapeTarget>>,
}

impl Component for ShapeDialog {
    fn render(&self) -> impl IntoElement {
        let Some(target) = self.target.read().clone() else {
            return rect().into_element();
        };
        ShapeCard {
            target,
            slot: self.target,
        }
        .into_element()
    }
}

/// The card itself — its own component so the form state lives exactly as long as the panel
/// is open: closing clears the slot, which unmounts this card and drops its state, so every
/// open seeds a fresh form from the target. The target cannot change *under* a mounted card
/// (its only writer is the toolbar press, unreachable beneath the modal's backdrop), so no
/// key is needed to tell two targets apart.
#[derive(PartialEq)]
struct ShapeCard {
    target: ShapeTarget,
    slot: State<Option<ShapeTarget>>,
}

/// The form as the target seeds it: every groupable column (grouped where the seed names
/// it), every measure (summed where the seed names it), row count off, ordered by group.
///
/// Groupable is the **complement** — everything that is not a measure and not unchartable —
/// the same answer `Roles::categories` gives the chart strip, so a new role groups in both
/// places or neither. A duplicate result name (a join settling two `id` columns) gets one
/// row: composed SQL addresses columns by name, and the subquery alias resolves a repeated
/// name to its *first* column, so a second row would be a pick that silently reads the
/// wrong data.
fn seeded(target: &ShapeTarget) -> ShapeForm {
    let seed_groups: &[String] = target.seed.as_ref().map_or(&[], |s| &s.groups);
    let seed_measures: &[String] = target.seed.as_ref().map_or(&[], |s| &s.measures);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut groups = Vec::new();
    let mut measures = Vec::new();
    for column in &target.columns {
        if column.role == ChartRole::Other || !seen.insert(column.name.as_str()) {
            continue;
        }
        if column.role == ChartRole::Measure {
            measures.push(MeasurePick {
                column: column.name.clone(),
                agg: seed_measures.contains(&column.name).then_some(SqlAgg::Sum),
            });
        } else {
            groups.push(GroupPick {
                column: column.name.clone(),
                role: column.role,
                by: if seed_groups.contains(&column.name) {
                    GroupBy::Exact
                } else {
                    GroupBy::Off
                },
            });
        }
    }
    ShapeForm {
        groups,
        measures,
        count_rows: false,
        order: ShapeOrder::ByGroup,
    }
}

impl Component for ShapeCard {
    fn render(&self) -> impl IntoElement {
        let roles = use_roles();
        let mut session = use_radio::<SessionState, Chan>(Chan::Tabs);
        let mut form = use_state({
            let seed = seeded(&self.target);
            move || seed
        });

        let held = form.read().clone();
        let composed = compose(&held, &self.target.sql);
        let mut slot = self.slot;
        let tab_name = session.read().name(self.target.tab);

        let mut confirm = {
            let composed = composed.clone();
            let name = format!("{tab_name} · shaped");
            move |()| {
                let Some(sql) = composed.clone() else {
                    return;
                };
                session
                    .write_channel(Chan::Tabs)
                    .open_named(&name, sql, Origin::Scratch);
                slot.set(None);
            }
        };
        let mut confirm_press = confirm.clone();

        let mut group_rows = Form::new();
        for (i, group) in held.groups.iter().enumerate() {
            group_rows = group_rows.child(match group.role {
                ChartRole::Dimension => {
                    let on = group.by == GroupBy::Exact;
                    Row::new(group.column.clone())
                        .trailing()
                        .on_press(move |_| flip_group(form, i))
                        .child(Switch::new().toggled(on).on_toggle(move |()| {
                            flip_group(form, i);
                        }))
                }
                ChartRole::Instant | ChartRole::Clock => {
                    Row::new(group.column.clone()).child(group_select(form, i, group))
                }
                _ => unreachable!("seeded() only builds group rows for category roles"),
            });
        }

        let mut measure_rows = Form::new();
        for (i, measure) in held.measures.iter().enumerate() {
            measure_rows = measure_rows
                .child(Row::new(measure.column.clone()).child(measure_select(form, i, measure)));
        }
        let count_on = held.count_rows;
        measure_rows = measure_rows.child(
            Row::new("Row count")
                .hint("count(*) per group")
                .trailing()
                .on_press(move |_| flip_count(form))
                .child(
                    Switch::new()
                        .toggled(count_on)
                        .on_toggle(move |()| flip_count(form)),
                ),
        );

        let mut order_pill = SegmentedToggle::new();
        for (label, title, order) in [
            ("By group", "Group columns, ascending", ShapeOrder::ByGroup),
            (
                "By value",
                "First aggregate, descending",
                ShapeOrder::ByMeasureDesc,
            ),
        ] {
            order_pill = order_pill.child(
                ToggleSegment::text(label)
                    .title(title)
                    .selected(held.order == order)
                    .on_press(move |_| {
                        form.write().order = order;
                    }),
            );
        }
        let order_rows = Form::new().child(Row::new("Order").child(order_pill));

        let section = |label: &'static str, body: Form| {
            rect()
                .width(Size::fill())
                .vertical()
                .spacing(SP_3)
                .child(Eyebrow::new(label).color(roles.get(Role::TextLabel)))
                .child(body)
        };

        let header = rect()
            .width(Size::fill())
            .padding((SP_6, SP_6, SP_5, SP_6))
            .child(DialogHeader::new(
                IconName::Rows,
                roles.get(Role::Accent),
                rect()
                    .vertical()
                    .spacing(SP_1)
                    .child(Title::new("Shape result"))
                    .child(
                        Caption::new(format!(
                            "Compose a grouped query over '{tab_name}' and open it in a new tab."
                        ))
                        .color(roles.get(Role::TextMuted))
                        .wrap(),
                    ),
            ));

        let body = ScrollView::new().child(
            rect()
                .width(Size::fill())
                .vertical()
                .spacing(SP_5)
                .padding((SP_2, SP_6, SP_5, SP_6))
                .child(section("GROUP BY", group_rows))
                .child(section("MEASURES", measure_rows))
                .child(section("ORDER BY", order_rows)),
        );

        let strip = rect()
            .width(Size::fill())
            .horizontal()
            .main_align(Alignment::End)
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .padding((SP_4, SP_6))
            .background(roles.get(Role::SurfaceRaised))
            .child(
                Button::new()
                    .flat()
                    .height(Size::px(ACTION_HEIGHT))
                    .on_press(move |_| slot.set(None))
                    .child(Control::new("Cancel")),
            )
            .child(
                Button::new()
                    .filled()
                    .height(Size::px(ACTION_HEIGHT))
                    .enabled(composed.is_some())
                    .on_press(move |_| confirm_press(()))
                    .child(Control::new("Open in new tab")),
            );

        let card = rect()
            .width(Size::px(CARD_WIDTH))
            .height(Size::px(CARD_HEIGHT))
            .max_width(Size::window_percent(92.))
            .max_height(Size::window_percent(88.))
            .corner_radius(R_4)
            .background(roles.get(Role::ElevatedSurface))
            .border(Border::new().width(1.).fill(roles.get(Role::Border)))
            .shadow(
                Shadow::new()
                    .y(30.)
                    .blur(80.)
                    .color(roles.get(Role::Shadow)),
            )
            .overflow(Overflow::Clip)
            .a11y_role(AccessibilityRole::Dialog)
            .vertical()
            .content(Content::Flex)
            .child(header)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .child(body),
            )
            .child(Divider::horizontal().color(roles.get(Role::Border)))
            .child(strip)
            .child(
                rect().on_global_key_down(move |e: Event<KeyboardEventData>| {
                    if matches!(&e.key, Key::Named(NamedKey::Enter)) {
                        confirm(());
                        e.prevent_default();
                    }
                }),
            );

        Modal::new(card)
            .on_close_request(move |()| slot.set(None))
            .into_element()
    }
}

/// Toggle a dimension row between grouped and not.
fn flip_group(mut form: State<ShapeForm>, i: usize) {
    let mut held = form.write();
    if let Some(group) = held.groups.get_mut(i) {
        group.by = match group.by {
            GroupBy::Off => GroupBy::Exact,
            _ => GroupBy::Off,
        };
    }
}

/// Toggle the standalone `count(*)`.
fn flip_count(mut form: State<ShapeForm>) {
    let mut held = form.write();
    held.count_rows = !held.count_rows;
}

/// A time column's grouping menu: off, its exact value, or a `date_bin` stride — sub-day
/// strides only for a clock.
fn group_select(mut form: State<ShapeForm>, i: usize, group: &GroupPick) -> Select {
    let strides: &[Stride] = if group.role == ChartRole::Clock {
        &Stride::SUB_DAY
    } else {
        &Stride::ALL
    };
    let current = match group.by {
        GroupBy::Off => "Off".to_string(),
        GroupBy::Exact => "Exact value".to_string(),
        GroupBy::Binned(stride) => stride.label().to_string(),
    };
    let mut options: Vec<(String, GroupBy)> = vec![
        ("Off".into(), GroupBy::Off),
        ("Exact value".into(), GroupBy::Exact),
    ];
    options.extend(
        strides
            .iter()
            .map(|stride| (stride.label().to_string(), GroupBy::Binned(*stride))),
    );

    let by_now = group.by;
    Select::new()
        .theme(SelectThemePartial::new().width(Size::px(CONTROL_WIDTH)))
        .selected_item(MonoValue::new(current))
        .children(options.into_iter().map(move |(label, by)| {
            MenuItem::new()
                .selected(by == by_now)
                .on_press(move |_| {
                    let mut held = form.write();
                    if let Some(group) = held.groups.get_mut(i) {
                        group.by = by;
                    }
                })
                .child(MonoValue::new(label))
        }))
}

/// A measure row's aggregate menu: skip, or one of the SQL aggregates.
fn measure_select(mut form: State<ShapeForm>, i: usize, measure: &MeasurePick) -> Select {
    let current = measure
        .agg
        .map_or("Skip".to_string(), |agg| agg.func().to_string());
    let agg_now = measure.agg;
    let mut options: Vec<(String, Option<SqlAgg>)> = vec![("Skip".into(), None)];
    options.extend(
        SqlAgg::ALL
            .iter()
            .map(|agg| (agg.func().to_string(), Some(*agg))),
    );

    Select::new()
        .theme(SelectThemePartial::new().width(Size::px(CONTROL_WIDTH)))
        .selected_item(MonoValue::new(current))
        .children(options.into_iter().map(move |(label, agg)| {
            MenuItem::new()
                .selected(agg == agg_now)
                .on_press(move |_| {
                    let mut held = form.write();
                    if let Some(measure) = held.measures.get_mut(i) {
                        measure.agg = agg;
                    }
                })
                .child(MonoValue::new(label))
        }))
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
    use freya::radio::RadioStation;
    use freya_testing::TestingRunner;
    use futures::executor::block_on;
    use strata_core::theme::load;
    use strata_engine::{column_info, RunTag, WsId};

    use super::compose::{GroupBy, GroupPick, MeasurePick, ShapeForm, ShapeOrder, SqlAgg, Stride};
    use super::*;
    use crate::apps::project::contexts::EngineCtx;
    use crate::theme::strata_theme;

    /// A result with every awkward column the composer has to survive: a `Date64` instant
    /// (which `date_bin` does **not** coerce — the CAST is load-bearing), a clock, a
    /// NULL-holding group, a reserved word and an uppercase name.
    const FIXTURE: &str = "SELECT \
         arrow_cast(CAST(column1 AS DATE), 'Date64') AS \"day\", \
         CAST(column2 AS TIME) AS \"at\", \
         column3 AS \"Group\", \
         column4 AS \"select\", \
         column5 AS amount \
         FROM (VALUES \
             ('2024-01-01', '09:30:00', 'north', 1, 10.0), \
             ('2024-01-01', '10:30:00', NULL, 2, 20.0), \
             ('2024-02-03', '09:45:00', 'south', 3, 30.0)) AS t";

    /// **The composed SQL runs.** Golden strings prove the shape; only the engine proves the
    /// `Date64` cast, the clock stride, the NULL group and the quoted names survive planning.
    #[test]
    fn composed_sql_runs_against_the_engine() {
        let engine = EngineCtx::default();
        let form = ShapeForm {
            groups: vec![
                GroupPick {
                    column: "day".into(),
                    role: ChartRole::Instant,
                    by: GroupBy::Binned(Stride::Month),
                },
                GroupPick {
                    column: "at".into(),
                    role: ChartRole::Clock,
                    by: GroupBy::Binned(Stride::Hour),
                },
                GroupPick {
                    column: "Group".into(),
                    role: ChartRole::Dimension,
                    by: GroupBy::Exact,
                },
            ],
            measures: vec![
                MeasurePick {
                    column: "amount".into(),
                    agg: Some(SqlAgg::Sum),
                },
                MeasurePick {
                    column: "select".into(),
                    agg: Some(SqlAgg::Count),
                },
            ],
            count_rows: true,
            order: ShapeOrder::ByMeasureDesc,
        };
        let sql = compose(&form, FIXTURE).expect("has output");
        let (out, _) = block_on(engine.query(WsId(90), RunTag(1), sql.clone(), 10))
            .unwrap_or_else(|e| panic!("composed SQL failed to run: {e}\n{sql}"));
        let names: Vec<&str> = out.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["day", "at", "Group", "amount_sum", "select_count", "rows"]
        );
        assert_eq!(out.total, 3, "three distinct (month, hour, group) cells");
    }

    /// **The dialog composes and opens, unrun, in a tab the user owns.** Mounted over a real
    /// session store: the seeded form enables the confirm, and the press lands a new named
    /// scratch tab whose text is the composed query — the active buffer untouched.
    #[test]
    fn the_panel_opens_the_composed_query_in_a_new_tab() {
        let mut session = SessionState::default();
        let tab = session.open_named("orders", "SELECT * FROM orders".into(), Origin::Scratch);
        let target = ShapeTarget {
            tab,
            sql: "SELECT * FROM orders".into(),
            columns: [
                ("region", DataType::Utf8),
                ("day", DataType::Date32),
                ("at", DataType::Time64(TimeUnit::Nanosecond)),
                ("amount", DataType::Float64),
            ]
            .into_iter()
            .map(|(name, dtype)| column_info(&Field::new(name, dtype, true)))
            .collect(),
            seed: Some(ShapeSeed {
                groups: vec!["region".into()],
                measures: vec!["amount".into()],
            }),
        };
        let app = move || {
            use_init_theme(|| strata_theme(&load("midnight")));
            let slot = use_provide_context({
                let target = target.clone();
                move || State::create(Some(target))
            });
            rect().expanded().child(ShapeDialog { target: slot })
        };
        let (mut runner, station) = TestingRunner::new(
            app,
            (900., 700.).into(),
            move |r| r.provide_root_context(|| RadioStation::<SessionState, Chan>::create(session)),
            1.,
        );
        for _ in 0..4 {
            runner.sync_and_update();
        }

        let texts: Vec<String> =
            runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()));
        for expected in ["Shape result", "GROUP BY", "MEASURES", "ORDER BY", "region"] {
            assert!(
                texts.iter().any(|t| t == expected),
                "no {expected}: {texts:?}"
            );
        }
        assert!(texts.iter().any(|t| t == "at"), "{texts:?}");

        let area = runner
            .find(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == "Open in new tab")
                    .map(|_| node.layout().area)
            })
            .expect("the confirm button");
        let point = (
            f64::from(area.min_x() + area.width() / 2.),
            f64::from(area.min_y() + area.height() / 2.),
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        for _ in 0..4 {
            runner.sync_and_update();
        }

        let state = station.peek();
        let shaped = state
            .tabs
            .values()
            .find(|t| t.name == "orders · shaped")
            .expect("the composed tab opened");
        let sql = shaped.text();
        assert!(sql.contains("sum(\"amount\") AS \"amount_sum\""), "{sql}");
        assert!(sql.contains("GROUP BY 1"), "{sql}");
        assert!(
            sql.contains("FROM (\nSELECT * FROM orders\n) AS q"),
            "{sql}"
        );
        assert!(state.request(tab).is_none(), "nothing ran");
        assert_eq!(state.tabs[&tab].text(), "SELECT * FROM orders");
    }

    /// The panel to a PNG, for eyeballing — the chart preview harness's terms.
    #[test]
    #[ignore = "writes target/shape-panel.png for eyeballing; run explicitly"]
    fn shape_panel_preview() {
        let mut session = SessionState::default();
        let tab = session.open_named("orders", "SELECT * FROM orders".into(), Origin::Scratch);
        let target = ShapeTarget {
            tab,
            sql: "SELECT * FROM orders".into(),
            columns: [
                ("day", DataType::Date32),
                ("at", DataType::Time64(TimeUnit::Nanosecond)),
                ("region", DataType::Utf8),
                ("channel", DataType::Utf8),
                ("amount", DataType::Float64),
                ("qty", DataType::Int64),
            ]
            .into_iter()
            .map(|(name, dtype)| column_info(&Field::new(name, dtype, true)))
            .collect(),
            seed: Some(ShapeSeed {
                groups: vec!["region".into()],
                measures: vec!["amount".into()],
            }),
        };
        let app = move || {
            use_init_theme(|| strata_theme(&load("midnight")));
            let slot = use_provide_context({
                let target = target.clone();
                move || State::create(Some(target))
            });
            rect().expanded().child(ShapeDialog { target: slot })
        };
        let (mut runner, _) = TestingRunner::new(
            app,
            (1000., 700.).into(),
            move |r| r.provide_root_context(|| RadioStation::<SessionState, Chan>::create(session)),
            1.,
        );
        for _ in 0..4 {
            runner.sync_and_update();
        }
        runner.render_to_file(format!(
            "{}/../../target/shape-panel.png",
            env!("CARGO_MANIFEST_DIR")
        ));
    }
}
