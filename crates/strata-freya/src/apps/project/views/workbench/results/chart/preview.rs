//! A headless render of the chart body to a PNG, for eyeballing — the same harness the plan
//! view uses (`explain_plan::preview`).
//!
//! `#[ignore]`d, because it writes files rather than asserting anything. It exists because a
//! chart is the one surface in this app whose correctness is *visual*: the unit tests can pin
//! that a gap cuts a run and that an axis spans its data, and none of them can see that a
//! legend has run off the pane or that a wedge is aliased.

use freya::prelude::*;
use freya_testing::TestingRunner;
use strata_core::theme::load;
use strata_model::{Axis, ChartBin, ChartData, ChartMark, ChartPoint, ChartSeries};

use super::paint::{ChartCanvas, Dress, Frame};
use super::strip::ControlStrip;
use super::{legend, ChartThemePartial, ChartThemePreference};
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

/// The whole body: strip on the left (mark picker + legend), plot on the right.
fn body(data: ChartData, mark: ChartMark) -> impl IntoElement {
    let theme = get_theme!(&None::<ChartThemePartial>, ChartThemePreference, "chart");
    let typography = scale();
    let dress = Dress::new(&theme, &typography);
    let mark_state = use_state(move || mark);
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .horizontal()
        .content(Content::Flex)
        .background(theme.background)
        .child(ControlStrip::new(mark_state).legend(legend(&data, mark, &dress)))
        .child(
            rect()
                .width(Size::flex(1.))
                .height(Size::fill())
                .padding((8., 12.))
                .child(ChartCanvas::new(Frame { data, mark, dress })),
        )
}

/// Render one mark to `target/chart-<name>.png`.
fn shoot(name: &str, data: ChartData, mark: ChartMark, hover: Option<(f64, f64)>) {
    let app = move || {
        use_init_theme(|| strata_theme(&load("midnight")));
        body(data.clone(), mark)
    };
    let (mut runner, _) = TestingRunner::new(app, (1000., 620.).into(), |_| {}, 1.);
    runner.sync_and_update();
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
