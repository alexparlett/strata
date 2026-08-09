//! A headless render of the chart body to a PNG, for eyeballing — the same harness the plan
//! view uses (`explain_plan::preview`).
//!
//! `#[ignore]`d, because it writes files rather than asserting anything. It exists because a
//! chart is the one surface in this app whose correctness is *visual*: the unit tests can pin
//! that a gap cuts a run and that an axis spans its data, and none of them can see that a
//! legend has run off the pane or that a wedge is aliased.

use std::rc::Rc;
use std::time::Duration;

use datafusion::arrow::datatypes::{DataType, Field};
use freya::prelude::*;
use freya::radio::RadioStation;
use freya_testing::TestingRunner;
use strata_core::engine::column_info;
use strata_core::theme::load;
use strata_model::{
    Axis, ChartBin, ChartConfig, ChartData, ChartMark, ChartPoint, ChartSeries, ColumnInfo, TabId,
};

use super::config::{resolve, Roles};
use super::paint::{ChartCanvas, Dress, Frame};
use super::strip::ControlStrip;
use super::{legend, ChartThemePartial, ChartThemePreference};
use crate::apps::project::state::{Chan, SessionState};
use crate::components::typography::scale;
use crate::theme::strata_theme;
use freya::components::get_theme;

/// Two named series over six categories, one carrying a gap.
fn table() -> ChartData {
    ChartData::Table {
        axis: Axis {
            labels: ["login", "purchase", "refund", "signup", "logout", "view"]
                .map(String::from)
                .to_vec(),
            positions: None,
        },
        series: vec![
            ChartSeries {
                name: "amount".into(),
                values: vec![
                    Some(1_240.),
                    Some(2_480.),
                    Some(410.),
                    None,
                    Some(1_900.),
                    Some(3_100.),
                ],
            },
            ChartSeries {
                name: "user_id".into(),
                values: vec![
                    Some(820.),
                    Some(1_450.),
                    Some(2_010.),
                    Some(760.),
                    Some(1_120.),
                    Some(2_400.),
                ],
            },
        ],
    }
}

/// A pie past the ten-colour ramp, so the wrap shading is visible.
fn many_slices() -> ChartData {
    ChartData::Table {
        axis: Axis {
            labels: (1..=13).map(|n| format!("category-{n}")).collect(),
            positions: None,
        },
        series: vec![ChartSeries {
            name: "n".into(),
            values: (1..=13).map(|n| Some(f64::from(n) * 10.)).collect(),
        }],
    }
}

/// Two measures nowhere near the origin — the case a zero-anchored axis squashed.
fn points() -> ChartData {
    ChartData::Points(
        (0..120)
            .map(|i| ChartPoint {
                x: 2000. + f64::from(i) * 0.2,
                y: 51.5 + f64::from(i % 17) * 0.01,
            })
            .collect(),
    )
}

/// The schema the fixtures came from — real Arrow fields through the engine's own
/// `column_info`, so the strip's menus are the ones this result would really offer, and the
/// names in them are the ones on the plot beside it.
fn columns() -> Vec<ColumnInfo> {
    [
        ("event", DataType::Utf8),
        ("amount", DataType::Float64),
        ("user_id", DataType::Int64),
    ]
    .into_iter()
    .map(|(name, dtype)| column_info(&Field::new(name, dtype, true)))
    .collect()
}

/// A **wide** result — the `SELECT *` over a real parquet that the encoder menus have to stay
/// usable over. 40 columns is an ordinary table, and every one of them is offered on X.
fn wide_columns() -> Vec<ColumnInfo> {
    (0..40)
        .map(|i| {
            column_info(&Field::new(
                format!("column_{i:02}"),
                if i % 3 == 0 {
                    DataType::Float64
                } else {
                    DataType::Utf8
                },
                true,
            ))
        })
        .collect()
}

/// The whole body: strip on the left (mark picker, encoders, sort, legend), plot on the right.
fn body(data: ChartData, mark: ChartMark, schema: Vec<ColumnInfo>) -> impl IntoElement {
    let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
    let typography = scale();
    let dress = Dress::new(&theme, &typography);
    let config = ChartConfig {
        mark: Some(mark),
        ..ChartConfig::default()
    };
    let roles = Roles::of(&schema);
    let encoding = resolve(&config, &roles);
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .horizontal()
        .content(Content::Flex)
        .background(theme.background)
        .child(
            ControlStrip::new(TabId::new(), config, encoding, roles)
                .legend(legend(&data, mark, &dress)),
        )
        // The body's own pane, floor and all — not a second copy of it (see `canvas_pane`).
        .child(super::canvas_pane(ChartCanvas::new(Rc::new(Frame {
            data,
            mark,
            dress,
        }))))
}

/// Render one mark to `target/chart-<name>.png`.
///
/// `hover` moves the pointer before the shot; `press` clicks first, which is how the strip's
/// own popups get into a picture — an encoder's open list is the one part of this surface
/// whose layout a static render cannot show.
fn shoot(name: &str, data: ChartData, mark: ChartMark, hover: Option<(f64, f64)>) {
    shoot_at(name, data, mark, columns(), hover, None);
}

fn shoot_at(
    name: &str,
    data: ChartData,
    mark: ChartMark,
    schema: Vec<ColumnInfo>,
    hover: Option<(f64, f64)>,
    press: Option<(f64, f64)>,
) {
    let app = move || {
        use_init_theme(|| strata_theme(&load("midnight")));
        body(data.clone(), mark, schema.clone())
    };
    // The strip's controls write the tab's encoding, so they need the session store the
    // window provides — nothing presses here, but the handles have to resolve.
    let (mut runner, _) = TestingRunner::new(
        app,
        (1000., 620.).into(),
        |r| {
            r.provide_root_context(|| {
                RadioStation::<SessionState, Chan>::create(SessionState::default())
            });
        },
        1.,
    );
    runner.sync_and_update();
    if let Some(at) = press {
        runner.move_cursor(at);
        runner.click_cursor(at);
        // A `Select`'s list fades and slides in, and it is *transparent* until that animation
        // has run — so a shot that only settled the tree would show an open list as nothing at
        // all. Polling past the 125ms open is what puts it in the picture.
        runner.poll(Duration::from_millis(1), Duration::from_millis(350));
    }
    if let Some(at) = hover {
        // A paint first: the hit regions are recorded *by* the paint that draws them, and
        // headless only paints on demand — so without this the pointer lands on an empty map.
        // (In the app a frame has always been drawn before a pointer can be moved over it.)
        runner.render();
        runner.move_cursor(at);
        runner.sync_and_update();
        // The readout is placed from its own measured size, so it settles a frame later.
        runner.sync_and_update();
    }
    runner.render_to_file(format!(
        "{}/../../target/chart-{name}.png",
        env!("CARGO_MANIFEST_DIR")
    ));
}

#[test]
#[ignore = "writes target/chart-*.png for eyeballing; run explicitly"]
fn chart_preview() {
    shoot("bar", table(), ChartMark::Bar, None);
    // Inside the second category's first bar.
    shoot("bar-hover", table(), ChartMark::Bar, Some((445., 400.)));
    // The Y encoder open, on its trigger — where a multi-pick list lands, and how far it
    // runs before the strip's own scroll clips it.
    shoot_at(
        "strip-open",
        table(),
        ChartMark::Bar,
        columns(),
        None,
        Some((116., 259.)),
    );
    // The same list over a 40-column `SELECT *`: it must cap and scroll rather than run off
    // the bottom of the window, where its tail would be unreachable.
    shoot_at(
        "strip-open-wide",
        table(),
        ChartMark::Bar,
        wide_columns(),
        None,
        Some((116., 192.)),
    );
    shoot("line", table(), ChartMark::Line, None);
    shoot("area", table(), ChartMark::Area, None);
    shoot("pie", many_slices(), ChartMark::Pie, None);
    shoot("scatter", points(), ChartMark::Scatter, None);
    shoot(
        "histogram",
        ChartData::Bins(
            (0..12)
                .map(|i| ChartBin {
                    lo: f64::from(i) * 5.,
                    hi: f64::from(i + 1) * 5.,
                    count: 4 + (i as u64 * 7) % 23,
                })
                .collect(),
        ),
        ChartMark::Histogram,
        None,
    );
}

/// A guardrail notice in a **collapsed** pane — the state the min-width fix got wrong. What to
/// look for: the copy cut off by the pane edge, not reflowed into a column of letters, and
/// nothing painted over the strip.
#[test]
#[ignore = "writes target/chart-*.png for eyeballing; run explicitly"]
fn narrow_notice_preview() {
    for (name, width) in [("notice-narrow", 300.), ("notice-wide", 900.)] {
        let app = || {
            use_init_theme(|| strata_theme(&load("midnight")));
            let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
            let config = ChartConfig::default();
            let roles = Roles::of(&columns());
            let encoding = resolve(&config, &roles);
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .background(theme.background)
                .child(ControlStrip::new(TabId::new(), config, encoding, roles))
                .child(super::canvas_pane(super::Notice::new(
                    "Too much data to chart honestly",
                    "This result has more than 24 rows. Aggregate it in SQL so the chart draws \
                     a compact result."
                        .to_string(),
                    theme.note_color,
                )))
        };
        let (mut runner, _) = TestingRunner::new(
            app,
            (width, 420.).into(),
            |r| {
                r.provide_root_context(|| {
                    RadioStation::<SessionState, Chan>::create(SessionState::default())
                });
            },
            1.,
        );
        runner.sync_and_update();
        runner.render_to_file(format!(
            "{}/../../target/chart-{name}.png",
            env!("CARGO_MANIFEST_DIR")
        ));
    }
}
