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
use crate::components::metrics::SP_3;
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
        .child(super::canvas_pane(
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .vertical()
                .content(Content::Flex)
                .spacing(SP_3)
                .maybe_child(banner.map(super::Banner::new))
                .child(
                    rect()
                        .width(Size::fill())
                        .height(Size::flex(1.))
                        .child(ChartCanvas::new(Rc::new(Frame {
                            log_y: encoding.log_y && fallback.is_none(),
                            trend: None,
                            data,
                            mark,
                            dress,
                        }))),
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
    let (mut runner, ()) = TestingRunner::new(
        app,
        (1000., 620.).into(),
        |r| {
            r.provide_root_context(|| {
                RadioStation::<SessionState, Chan>::create(SessionState::default())
            });
        },
        1.,
    );
    for _ in 0..4 {
        runner.sync_and_update();
    }
    if let Some(at) = press {
        runner.move_cursor(at);
        runner.click_cursor(at);
        runner.poll(Duration::from_millis(1), Duration::from_millis(350));
    }
    if let Some(at) = hover {
        runner.render();
        runner.move_cursor(at);
        runner.sync_and_update();
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
    shoot("bar-hover", table(), ChartMark::Bar, Some((445., 400.)));
    shoot_at(
        "strip-open",
        table(),
        ChartMark::Bar,
        columns(),
        None,
        Some((116., 259.)),
    );
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

    shoot_as(
        "log-histogram",
        ChartData::Bins(
            (0..12)
                .map(|i| ChartBin {
                    lo: f64::from(i) * 5.,
                    hi: f64::from(i + 1) * 5.,
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

    shoot(
        "crosshair",
        bins(),
        ChartMark::Histogram,
        Some((495., 450.)),
    );

    shoot_as(
        "heatmap",
        matrix(),
        marked(ChartMark::Heatmap),
        matrix_columns(),
        None,
        None,
    );
    shoot_as(
        "heatmap-hover",
        matrix(),
        marked(ChartMark::Heatmap),
        matrix_columns(),
        Some((470., 500.)),
        None,
    );

    shoot_as(
        "band",
        banded(),
        ChartConfig {
            ys: Some(vec!["avg_ms".into()]),
            y_lo: Some("p05".into()),
            y_hi: Some("p95".into()),
            ..marked(ChartMark::Band)
        },
        stats_columns(),
        None,
        None,
    );

    shoot_as(
        "box",
        boxed(),
        ChartConfig {
            ys: Some(vec!["med".into()]),
            y_lo: Some("lo".into()),
            y_hi: Some("hi".into()),
            q1: Some("p25".into()),
            q3: Some("p75".into()),
            ..marked(ChartMark::Box)
        },
        stats_columns(),
        Some((400., 350.)),
        None,
    );
}

/// A 6x4 matrix with one empty cell — the heatmap fixture.
fn matrix() -> ChartData {
    let rows: [(&str, [Option<f64>; 6]); 4] = [
        (
            "north",
            [
                Some(12.),
                Some(48.),
                Some(31.),
                Some(90.),
                Some(22.),
                Some(65.),
            ],
        ),
        (
            "south",
            [Some(80.), Some(14.), None, Some(41.), Some(73.), Some(9.)],
        ),
        (
            "east",
            [
                Some(25.),
                Some(66.),
                Some(52.),
                Some(18.),
                Some(97.),
                Some(40.),
            ],
        ),
        (
            "west",
            [
                Some(5.),
                Some(33.),
                Some(78.),
                Some(59.),
                Some(11.),
                Some(84.),
            ],
        ),
    ];
    ChartData::Table {
        axis: Axis {
            labels: ["jan", "feb", "mar", "apr", "may", "jun"]
                .map(String::from)
                .to_vec(),
            positions: None,
        },
        series: rows
            .into_iter()
            .map(|(name, values)| ChartSeries {
                name: name.into(),
                values: values.to_vec(),
            })
            .collect(),
    }
}

/// The schema the matrix came from: two categories and a measure.
fn matrix_columns() -> Vec<ColumnInfo> {
    [
        ("month", DataType::Utf8),
        ("region", DataType::Utf8),
        ("orders", DataType::Int64),
    ]
    .into_iter()
    .map(|(name, dtype)| column_info(&Field::new(name, dtype, true)))
    .collect()
}

/// Centre and bounds over ten categories, with the bounds missing mid-run — the band fixture,
/// series in `encode`'s order (centre, lower, upper).
fn banded() -> ChartData {
    let centre: Vec<Option<f64>> = (0..10)
        .map(|i| Some(200. + f64::from(i % 5) * 40. + f64::from(i) * 10.))
        .collect();
    let lower: Vec<Option<f64>> = centre
        .iter()
        .enumerate()
        .map(|(i, v)| if i == 5 { None } else { v.map(|v| v - 60.) })
        .collect();
    let upper: Vec<Option<f64>> = centre
        .iter()
        .enumerate()
        .map(|(i, v)| if i == 5 { None } else { v.map(|v| v + 60.) })
        .collect();
    ChartData::Table {
        axis: Axis {
            labels: (0..10).map(|i| format!("t{i}")).collect(),
            positions: None,
        },
        series: vec![
            ChartSeries {
                name: "avg_ms".into(),
                values: centre,
            },
            ChartSeries {
                name: "p05".into(),
                values: lower,
            },
            ChartSeries {
                name: "p95".into(),
                values: upper,
            },
        ],
    }
}

/// Five categories of five measures — the box plot fixture, series in `encode`'s order
/// (median, low whisker, high whisker, q1, q3).
fn boxed() -> ChartData {
    let names = ["med", "lo", "hi", "p25", "p75"];
    let per_category: [[f64; 5]; 5] = [
        [50., 10., 95., 35., 70.],
        [42., 18., 66., 30., 55.],
        [78., 40., 120., 60., 96.],
        [22., 2., 58., 12., 39.],
        [64., 30., 88., 51., 75.],
    ];
    ChartData::Table {
        axis: Axis {
            labels: ["api", "web", "batch", "cron", "etl"]
                .map(String::from)
                .to_vec(),
            positions: None,
        },
        series: (0..5)
            .map(|role| ChartSeries {
                name: names[role].into(),
                values: per_category.iter().map(|c| Some(c[role])).collect(),
            })
            .collect(),
    }
}

/// The schema the band and box fixtures came from: a category, a time-ish label column and
/// the five computed measures.
fn stats_columns() -> Vec<ColumnInfo> {
    [
        ("t", DataType::Utf8),
        ("avg_ms", DataType::Float64),
        ("p05", DataType::Float64),
        ("p95", DataType::Float64),
        ("med", DataType::Float64),
        ("lo", DataType::Float64),
        ("hi", DataType::Float64),
        ("p25", DataType::Float64),
        ("p75", DataType::Float64),
    ]
    .into_iter()
    .map(|(name, dtype)| column_info(&Field::new(name, dtype, true)))
    .collect()
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

/// The scatter's trendline (Chart 11), posed directly on the canvas — the body only carries a
/// fit the engine settled, which this harness has no engine to ask. What to look for: the
/// dashed line through the cloud at the fixture's own slope, and the R² label inside the plot
/// near the line's end.
#[test]
#[ignore = "writes target/chart-*.png for eyeballing; run explicitly"]
fn trendline_preview() {
    use strata_model::Trend;

    let app = move || {
        use_init_theme(|| strata_theme(&load("midnight")));
        let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
        let scattered: Vec<ChartPoint> = (0..80)
            .map(|i| {
                let x = f64::from(i) * 0.5;
                ChartPoint {
                    x,
                    y: 2.0f64.mul_add(x, 5.) + f64::from((i * 37) % 23) - 11.,
                }
            })
            .collect();
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .background(theme.background)
            .child(ChartCanvas::new(Rc::new(Frame {
                data: ChartData::Points(scattered),
                mark: ChartMark::Scatter,
                log_y: false,
                trend: Some(Trend {
                    slope: 2.,
                    intercept: 5.,
                    r2: 0.87,
                    n: 80,
                }),
                dress: Dress::new(&theme, &scale()),
            })))
    };
    let (mut runner, ()) = TestingRunner::new(app, (1000., 620.).into(), |_| {}, 1.);
    runner.sync_and_update();
    runner.render_to_file(format!(
        "{}/../../target/chart-trendline.png",
        env!("CARGO_MANIFEST_DIR")
    ));
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
        let (mut runner, ()) = TestingRunner::new(
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
