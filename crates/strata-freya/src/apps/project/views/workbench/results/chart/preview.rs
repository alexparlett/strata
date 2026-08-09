//! A headless render of the chart body to a PNG, for eyeballing — the same harness the plan
//! view uses (`explain_plan::preview`).
//!
//! `#[ignore]`d, because it writes files rather than asserting anything. It exists because a
//! chart is the one surface in this app whose correctness is *visual*: the unit tests can pin
//! that a gap cuts a run and that an axis spans its data, and none of them can see that a
//! legend has run off the pane or that a wedge is aliased.

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

/// The whole body: strip on the left (mark picker, encoders, bins, sort, scale, legend), plot
/// on the right. `config` is the tab's stored intent, so a fixture can pose the surface in a
/// state a press would have put it in.
fn body(data: ChartData, config: ChartConfig, schema: Vec<ColumnInfo>) -> impl IntoElement {
    let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
    let typography = scale();
    let dress = Dress::new(&theme, &typography);
    let roles = Roles::of(&schema);
    let encoding = resolve(&config, &roles);
    let mark = encoding.mark;
    let data = super::hide::applied(data, &encoding.hidden);
    let key = legend(&data, mark, &dress, &encoding.hidden);
    // The same two questions the body asks, in the same order — a fixture that skipped them
    // would be a picture of a surface the app does not have.
    let fallback = encoding.log_y.then(|| super::log_fallback(&data)).flatten();
    let banner = fallback
        .map(str::to_string)
        .or_else(|| super::crowded(&data));
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .horizontal()
        .content(Content::Flex)
        .background(theme.background)
        .child(ControlStrip::new(TabId::new(), config, encoding.clone(), roles).legend(key))
        // The body's own pane, floor and all — not a second copy of it (see `canvas_pane`).
        .child(super::canvas_pane(
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .vertical()
                .content(Content::Flex)
                .spacing(8.)
                .maybe_child(banner.map(super::Banner::new))
                .child(
                    rect()
                        .width(Size::fill())
                        .height(Size::flex(1.))
                        .child(ChartCanvas::new(Frame {
                            log_y: encoding.log_y && fallback.is_none(),
                            data,
                            mark,
                            dress,
                        })),
                ),
        ))
}

/// Render one mark to `target/chart-<name>.png`.
///
/// `hover` moves the pointer before the shot; `press` clicks first, which is how the strip's
/// own popups get into a picture — an encoder's open list is the one part of this surface
/// whose layout a static render cannot show.
fn shoot(name: &str, data: ChartData, mark: ChartMark, hover: Option<(f64, f64)>) {
    shoot_as(name, data, marked(mark), columns(), hover, None);
}

/// An otherwise untouched config on one mark — what most fixtures pose.
fn marked(mark: ChartMark) -> ChartConfig {
    ChartConfig {
        mark: Some(mark),
        ..ChartConfig::default()
    }
}

fn shoot_at(
    name: &str,
    data: ChartData,
    mark: ChartMark,
    schema: Vec<ColumnInfo>,
    hover: Option<(f64, f64)>,
    press: Option<(f64, f64)>,
) {
    shoot_as(name, data, marked(mark), schema, hover, press);
}

fn shoot_as(
    name: &str,
    data: ChartData,
    config: ChartConfig,
    schema: Vec<ColumnInfo>,
    hover: Option<(f64, f64)>,
    press: Option<(f64, f64)>,
) {
    let app = move || {
        use_init_theme(|| strata_theme(&load("midnight")));
        body(data.clone(), config.clone(), schema.clone())
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
    // **Settled, not merely synced once.** The hit regions below are recorded by the *paint*,
    // at whatever layout is current when it runs — so a tree still one pass short of its final
    // layout records them at coordinates the finished picture does not use, and a hover lands
    // on nothing while the shot looks perfectly right. Measured: the histogram fixture missed
    // its own bar that way.
    for _ in 0..4 {
        runner.sync_and_update();
    }
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
    shoot("histogram", bins(), ChartMark::Histogram, None);

    // ---- Chart 06 ----

    // A hidden series: the middle line gone from the plot, its legend row dim, and every other
    // series still in the colour it had.
    shoot_as(
        "hidden-series",
        table(),
        ChartConfig {
            hidden: vec!["amount".into()],
            ..marked(ChartMark::Line)
        },
        columns(),
        None,
        None,
    );

    // A log count axis over a long tail — decade gridlines, bars standing on the axis floor.
    shoot_as(
        "log-histogram",
        ChartData::Bins(
            (0..12)
                .map(|i| ChartBin {
                    lo: f64::from(i) * 5.,
                    hi: f64::from(i + 1) * 5.,
                    // Two and a half decades of counts, with an empty bin in the tail — which
                    // must not cost the axis its log scale.
                    count: [900, 400, 180, 70, 30, 12, 6, 3, 2, 1, 0, 1][i as usize],
                })
                .collect(),
        ),
        ChartConfig {
            log_y: true,
            ..marked(ChartMark::Histogram)
        },
        columns(),
        None,
        None,
    );

    // The same preference over a series that dips to zero: linear, under the banner.
    shoot_as(
        "log-refused",
        ChartData::Table {
            axis: Axis {
                labels: (0..6).map(|i| format!("t{i}")).collect(),
                positions: None,
            },
            series: vec![ChartSeries {
                name: "amount".into(),
                values: vec![Some(10.), Some(0.), Some(400.), Some(90.), None, Some(7.)],
            }],
        },
        ChartConfig {
            log_y: true,
            ..marked(ChartMark::Line)
        },
        columns(),
        None,
        None,
    );

    // The crosshair: two hairlines across the plot frame through the hovered bin's own top
    // edge, its value at the axis, and the ordinary hover readout beside it. The pointer has
    // to be **on** a mark — the crosshair rides on the hover, which is what makes it free.
    shoot(
        "crosshair",
        bins(),
        ChartMark::Histogram,
        Some((495., 450.)),
    );
}

/// A dozen bins with a readable spread — the plain histogram fixture.
fn bins() -> ChartData {
    ChartData::Bins(
        (0..12)
            .map(|i| ChartBin {
                lo: f64::from(i) * 5.,
                hi: f64::from(i + 1) * 5.,
                count: 4 + (i as u64 * 7) % 23,
            })
            .collect(),
    )
}

/// A guardrail notice in a **collapsed** pane — the state the min-width fix got wrong. What to
/// look for: the copy cut off by the pane edge, not reflowed into a column of letters, and
/// nothing painted over the strip.
#[test]
#[ignore = "writes target/chart-*.png for eyeballing; run explicitly"]
fn narrow_notice_preview() {
    for (name, width) in [("notice-narrow", 300.), ("notice-wide", 900.)] {
        let app = move || {
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
