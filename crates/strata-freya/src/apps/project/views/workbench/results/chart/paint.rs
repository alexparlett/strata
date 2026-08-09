//! What the chart paints, and when it repaints.
//!
//! A [`Frame`] is everything the painter needs, as plain values: the settled data, the mark
//! chosen for it, and the [`Dress`] it wears. The component publishes it into a slot the
//! render callback **peeks**, which is the only shape that works here: `RenderCallback`'s
//! `PartialEq` is always-true, so Freya never treats a new closure as a change and the
//! callback stored in the tree stays the one from the first render. A callback that captured
//! the frame by value would therefore paint the first frame forever (the same staleness
//! `VirtualScrollView`'s builder has); a callback that reads a slot paints whatever is in the
//! slot when the paint happens.
//!
//! Nothing about that *schedules* a paint, so the same side effect that fills the slot asks
//! the platform for a redraw — the `feature_plot_3d` idiom. The four things that change a
//! chart are all covered by it: a new mark and newly settled data both change the frame; a
//! theme change changes the dress, because the dress is read in `render`; and a resize
//! repaints the tree anyway, with the slot's contents unchanged.

use std::cell::RefCell;
use std::f64::consts::TAU;
use std::rc::Rc;

use freya::prelude::*;
use strata_core::theme::Typography;
use strata_model::{ChartData, ChartMark};

use freya::components::Tooltip;

use super::axis::readout;
use super::marks;
use super::ChartTheme;
use crate::components::typography::Meta;

/// Every colour and font the painter needs, lifted out of the theme in `render` so the paint
/// callback itself touches no hooks.
#[derive(Clone, PartialEq)]
pub struct Dress {
    /// The pane the plot sits on — also what a pie's later wraps are blended toward.
    pub background: Color,
    pub grid: Color,
    pub axis: Color,
    pub tick: Color,
    /// The categorical ramp, in order. A series past the tenth wraps around.
    pub series: [Color; 10],
    /// The type scale's `meta` role — the small mono the canvas labels its axes in.
    pub label: (String, f64),
}

impl Dress {
    pub fn new(theme: &ChartTheme, typography: &Typography) -> Self {
        Self {
            background: theme.background,
            grid: theme.grid_fill,
            axis: theme.axis_fill,
            tick: theme.tick_color,
            series: [
                theme.series_1,
                theme.series_2,
                theme.series_3,
                theme.series_4,
                theme.series_5,
                theme.series_6,
                theme.series_7,
                theme.series_8,
                theme.series_9,
                theme.series_10,
            ],
            label: (
                typography.meta.family.clone(),
                f64::from(typography.meta.size),
            ),
        }
    }

    /// The ramp colour for series `i`, wrapping past the tenth.
    pub fn series(&self, i: usize) -> Color {
        self.series[i % self.series.len()]
    }

    /// The ramp colour for **pie slice** `i` — the ramp, blended a step toward the pane on each
    /// wrap past its tenth colour.
    ///
    /// A pie's last slice touches its first, which no other mark's last series does, so a bare
    /// modulo puts two identically-filled wedges edge to edge at exactly eleven slices (and
    /// again at twenty-one, both under the read's own 24-slice cap) and the two read as one.
    /// `Pie` fills its wedges with no separating stroke, so the colour has to carry the
    /// boundary.
    pub fn slice(&self, i: usize) -> Color {
        let wraps = i / self.series.len();
        let base = self.series(i);
        if wraps == 0 {
            return base;
        }
        blend(base, self.background, (wraps as f32 * 0.28).min(0.7))
    }
}

/// `a` moved `t` of the way toward `b` (0 = `a`, 1 = `b`), per channel.
fn blend(a: Color, b: Color, t: f32) -> Color {
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

/// One paintable state of the chart.
#[derive(Clone, PartialEq)]
pub struct Frame {
    pub data: ChartData,
    pub mark: ChartMark,
    /// Whether the value axis is drawn logarithmically — already resolved against the mark
    /// (`config::log_axis`) and against the data (`mod.rs::log_fallback`), so the painter only
    /// has to obey it.
    pub log_y: bool,
    pub dress: Dress,
}

/// One drawn mark's hit region and what it says, in **logical canvas coordinates**.
///
/// Recorded by the paint that drew it rather than recomputed for the pointer, because the paint
/// is the only place the true geometry exists: plotters owns the mapping from a value to a
/// pixel, and a second copy of that arithmetic out here would be a second answer to *where is
/// this bar* — the kind that drifts silently the first time a margin changes.
pub enum Hit {
    /// A bar, a bin, or the square around a point.
    Box {
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
        /// Where the mark itself sits and what it is worth — a bar's top edge at its centre, a
        /// point's own coordinates, and the value the axis puts there.
        ///
        /// Distinct from the box, which is a *reach* around the mark: a point's box is
        /// [`POINT_REACH`](super::marks) larger in every direction, so its top edge is not
        /// where the value is. And the value is **carried, not inverted** — mapping it back
        /// out of the pixel row is a round trip through integer pixels, which put `11.01`
        /// under a tooltip reading `11`.
        cross: Cross,
        label: String,
    },
    /// A pie's wedge: where its arc starts, and how far it sweeps.
    ///
    /// A **start and a length**, not two bounds, because a wedge is an arc on a circle and two
    /// independently wrapped bounds cannot describe one: the wedge containing three o'clock
    /// gets `to < from`, and every ordinary `from <= a < to` test then answers false for the
    /// whole of it. Every pie has exactly one such wedge, and a single-slice pie is all of it.
    Wedge {
        center: (f32, f32),
        radius: f32,
        /// Normalised into `0..TAU`.
        from: f64,
        /// The arc's length, always positive; `TAU` for a lone slice.
        sweep: f64,
        label: String,
    },
}

impl Hit {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        match self {
            Hit::Box {
                left,
                top,
                right,
                bottom,
                ..
            } => x >= *left && x <= *right && y >= *top && y <= *bottom,
            Hit::Wedge {
                center,
                radius,
                from,
                sweep,
                ..
            } => {
                let (dx, dy) = (f64::from(x - center.0), f64::from(y - center.1));
                if dx.hypot(dy) > f64::from(*radius) {
                    return false;
                }
                // Measure the pointer *from the wedge's own start* and wrap once, so an arc
                // that crosses zero is the same test as one that does not.
                let angle = dy.atan2(dx).rem_euclid(TAU);
                (angle - *from).rem_euclid(TAU) < *sweep
            }
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Hit::Box { label, .. } | Hit::Wedge { label, .. } => label,
        }
    }

    /// Where the crosshair rules through this mark and what it reads there — `None` for a
    /// wedge, which sits on no axis and has no row or column to rule.
    pub fn cross(&self) -> Option<Cross> {
        match self {
            Hit::Box { cross, .. } => Some(*cross),
            Hit::Wedge { .. } => None,
        }
    }

    /// Where the readout for this mark hangs — a point on the **mark**, not under the pointer.
    ///
    /// Anchoring to the mark is what makes the hover state settle: a readout that follows the
    /// pointer changes on every pixel of movement, so the "has the hover changed" guard never
    /// holds and each mouse sample re-renders the canvas and repaints the whole plot.
    pub fn anchor(&self) -> (f32, f32) {
        match self {
            Hit::Box {
                left, top, right, ..
            } => ((left + right) / 2., *top),
            Hit::Wedge {
                center,
                radius,
                from,
                sweep,
                ..
            } => {
                let mid = from + sweep / 2.;
                (
                    center.0 + (f64::from(*radius) * 0.6 * mid.cos()) as f32,
                    center.1 + (f64::from(*radius) * 0.6 * mid.sin()) as f32,
                )
            }
        }
    }
}

/// Where the crosshair rules through one mark, and the value the axis puts there.
#[derive(Clone, Copy, PartialEq)]
pub struct Cross {
    pub at: (f32, f32),
    pub value: f64,
}

/// Where a cartesian mark's plot frame landed — read off plotters' own geometry by the paint
/// that built it, for the same reason a [`Hit`] is.
///
/// It is what the crosshair's rules are clipped to, so they span the plot rather than the pane
/// whatever the axes' insets happen to be. `None` for a pie, which has no frame.
#[derive(Clone, Copy, PartialEq)]
pub struct PlotArea {
    /// The plotting area, in logical canvas coordinates.
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Where a paint leaves what it drew, for the pointer to read back: every mark's hit region,
/// and the plot frame they sit in. A plain `RefCell` and not a `State`: nothing renders from
/// it, so a write must not wake anything.
pub type Plotted = Rc<RefCell<(Vec<Hit>, Option<PlotArea>)>>;

/// Which mark the pointer is over, and where on it to say so.
///
/// `at` is the **mark's** anchor rather than the pointer's position, so this value changes only
/// when the hovered mark changes — which is what lets the `!=` guard in the pointer handler
/// suppress the other ninety-nine samples of a slow drag.
#[derive(Clone, PartialEq)]
struct Hover {
    label: String,
    at: (f32, f32),
    /// Where the crosshair rules through the mark, and what it reads there. Carried rather
    /// than looked up again, so the rules and the readout name the same mark this state
    /// settled on. `None` for a pie's wedge, which has no axis to rule.
    cross: Option<Cross>,
}

/// How far the readout sits from its anchor, so it never covers the mark it names.
const READOUT_OFFSET: f32 = 14.;

/// The crosshair's own furniture: a hairline, and the inset its readout keeps from the plot's
/// left edge.
const HAIRLINE: f32 = 1.;
const CROSS_READOUT_GAP: f32 = 4.;

/// Squared distance from a mark's anchor to the pointer — the tie-break when hit regions
/// overlap. Squared because only the ordering is wanted, and `total_cmp` needs a total order.
fn reach((ax, ay): (f32, f32), x: f32, y: f32) -> f32 {
    (ax - x).powi(2) + (ay - y).powi(2)
}

/// The plot itself: a `canvas` the size of its pane, repainted on demand, with a readout that
/// names whatever mark the pointer is over.
#[derive(PartialEq)]
pub struct ChartCanvas {
    frame: Frame,
}

impl ChartCanvas {
    pub fn new(frame: Frame) -> Self {
        Self { frame }
    }
}

impl Component for ChartCanvas {
    fn render(&self) -> impl IntoElement {
        let platform = use_hook(Platform::get);
        // Seeded, and cloned **once**. A `use_state` initialiser only ever runs on the first
        // render, but the expression building its seed is evaluated on every one — so this is
        // one deep copy of the read per render, and it used to be two. It is not zero on
        // purpose: an empty slot filled by the effect below would leave the first paint with
        // nothing to draw, and the paint is what records the hit regions the pointer reads, so
        // a chart would not answer a hover until something else asked it to repaint.
        let mut slot = use_state({
            let seed = Rc::new(self.frame.clone());
            move || seed
        });

        let plotted: Plotted = use_hook(|| Rc::new(RefCell::new((Vec::new(), None))));
        let mut hover = use_state(|| None::<Hover>);
        let mut size = use_state(Size2D::default);
        let mut readout_size = use_state(Size2D::default);

        // `use_reactive` under the hood, because a `use_side_effect` closure is built once and
        // would capture the *first* frame forever (AGENTS.md §3).
        use_side_effect_with_deps(&self.frame, move |frame| {
            slot.set(Rc::new(frame.clone()));
            // The repaint this asks for rebuilds every hit region and re-lays the plot frame,
            // so whatever the readout was naming is gone: keeping it would leave a label
            // pinned over a mark that has moved or is no longer drawn, and a crosshair
            // ruling through an axis that has changed underneath it.
            hover.set(None);
            platform.send(UserEvent::RequestRedraw);
        });

        let plot = canvas(RenderCallback::new({
            let plotted = Rc::clone(&plotted);
            move |context| {
                let frame = Rc::clone(&slot.peek());
                marks::draw(context, &frame, &plotted);
            }
        }))
        .width(Size::fill())
        .height(Size::fill())
        .on_sized(move |e: Event<SizedEventData>| {
            // A resize re-lays the plot and so rebuilds every hit region — same staleness as a
            // new frame. `set_if_modified` because a re-measure that found the same size must
            // not wake a render of its own.
            if *size.peek() != e.area.size {
                hover.set(None);
            }
            size.set_if_modified(e.area.size);
        })
        .on_pointer_move({
            let plotted = Rc::clone(&plotted);
            move |e: Event<PointerEventData>| {
                let at = e.element_location();
                let (x, y) = (at.x as f32, at.y as f32);
                // The **nearest** mark whose region contains the pointer, not the first one
                // pushed: hit regions are a fixed reach around a point, so two series that
                // pass within that reach overlap, and taking the first would make the later
                // series unnameable anywhere in the band.
                let found = plotted
                    .borrow()
                    .0
                    .iter()
                    .filter(|hit| hit.contains(x, y))
                    .min_by(|a, b| reach(a.anchor(), x, y).total_cmp(&reach(b.anchor(), x, y)))
                    .map(|hit| Hover {
                        label: hit.label().to_string(),
                        at: hit.anchor(),
                        cross: hit.cross(),
                    });
                if *hover.peek() != found {
                    hover.set(found);
                }
            }
        })
        .on_pointer_leave(move |_| {
            if hover.peek().is_some() {
                hover.set(None);
            }
        });

        // The crosshair: two hairlines across the plot frame through the **hovered mark**, and
        // the value its row sits at, read back through the axis's own mapping. Placed from the
        // recorded frame rather than the pane, so the rules line up with the gridlines whatever
        // the plot's insets are.
        //
        // **Through the mark, not under the pointer** — and that is the whole cost model, not a
        // simplification. Freya has no incremental rendering (`render_pipeline.rs`: every node
        // repaints every frame) and `CanvasElement::render` calls its `on_render` each pass, so
        // *any* reactive write here re-runs `marks::draw` — a full plotters replot plus a
        // rebuild of every hit region, on the render thread. A crosshair that followed the
        // pointer would do that on every mouse sample. Riding on `hover` instead costs nothing
        // beyond what the readout already costs, for the same reason `Hit::anchor` exists: the
        // state changes when the hovered *mark* changes, so a slow drag across one bar is
        // zero renders. The price is that the value axis can only be read at a mark, which is
        // where the numbers are.
        let label_height = self.frame.dress.label.1 as f32;
        // **Absolute siblings of the plot, not children of a wrapper.** An absolutely
        // positioned node resolves its offsets against its parent's area — and a wrapper here
        // would be a *stacked* sibling of a fill-height plot, so its own area starts below the
        // canvas and every hairline inside it lands off screen. Measured: the horizontal rule
        // came out 600px (one pane) below where the pointer was.
        let crosshair: Vec<Element> = hover
            .read()
            .as_ref()
            // A wedge answers no cross and a pie records no plot frame — either way there is
            // no axis here to rule through.
            .and_then(|hover| hover.cross)
            .zip(plotted.borrow().1)
            .map(|(Cross { at: (x, y), value }, frame)| {
                let dress = self.frame.dress.clone();
                // In front of the plot for the same reason the readout is: two siblings on one
                // layer have no paint order (AGENTS.md §3). Never a pointer target either — a
                // hit-testable hairline under the pointer would take the pointer off the
                // canvas and unmount the crosshair that drew it.
                let hair = |el: Rect| el.layer(Layer::Relative(1)).interactive(false);
                vec![
                    hair(rect())
                        .position(Position::new_absolute().left(x).top(frame.top))
                        .width(Size::px(HAIRLINE))
                        .height(Size::px(frame.bottom - frame.top))
                        .background(dress.tick)
                        .into_element(),
                    hair(rect())
                        .position(Position::new_absolute().left(frame.left).top(y))
                        .width(Size::px(frame.right - frame.left))
                        .height(Size::px(HAIRLINE))
                        .background(dress.tick)
                        .into_element(),
                    hair(rect())
                        .position(
                            Position::new_absolute()
                                .left(frame.left + CROSS_READOUT_GAP)
                                // Above the rule, so the label names the row it sits on rather
                                // than covering it.
                                .top(y - CROSS_READOUT_GAP - label_height),
                        )
                        .background(dress.background)
                        .child(Meta::new(readout(value)).color(dress.tick))
                        .into_element(),
                ]
            })
            .unwrap_or_default();

        let readout = hover.read().clone().map(|hover| {
            let (pane, card) = (*size.read(), *readout_size.read());
            // Flip to the other side of the anchor rather than running off the pane — the
            // failure the plot's own overlay legend had. `card` is last frame's measurement,
            // which is a frame behind on the very first hover; `Tooltip` keeps itself
            // transparent until it has measured its own text, so that frame is not one
            // anybody sees.
            let flip = |at: f32, card: f32, pane: f32| {
                if at + READOUT_OFFSET + card > pane {
                    (at - READOUT_OFFSET - card).max(0.)
                } else {
                    at + READOUT_OFFSET
                }
            };
            rect()
                // **In front of the plot, explicitly.** Freya paints by layer and holds the
                // nodes of one layer in a hash set, so two siblings on the same layer paint in
                // whatever order that set iterates — the readout came out *behind* the marks,
                // showing through only where one was semi-transparent. A relative layer is the
                // whole fix; `Overlay` is a jump for things that must clear the window, and a
                // readout only has to clear its own siblings. `2` rather than `1` because the
                // crosshair is one of those siblings and the card names a mark, so it wins.
                .layer(Layer::Relative(2))
                .position(
                    Position::new_absolute()
                        .top(flip(hover.at.1, card.height, pane.height))
                        .left(flip(hover.at.0, card.width, pane.width)),
                )
                .on_sized(move |e: Event<SizedEventData>| readout_size.set_if_modified(e.area.size))
                // Never a pointer target. The clamp below can put the card under the pointer on
                // a narrow pane, and a hit-testable card there would take the pointer off the
                // canvas, fire `pointer_leave`, clear the hover and unmount itself — a readout
                // that vanishes the instant it appears.
                .interactive(false)
                // The card itself is the standard `Tooltip` — it carries the app's tooltip
                // dress and wraps at its own width cap, so this wrapper does nothing but place
                // it.
                .child(Tooltip::new_text(hover.label))
        });

        rect()
            .width(Size::fill())
            .height(Size::fill())
            .child(plot)
            .children(crosshair)
            .maybe_child(readout)
    }
}

#[cfg(test)]
mod tests {
    use freya_testing::TestingRunner;
    use strata_core::theme::load;
    use strata_model::{ChartBin, ChartData};

    use super::*;
    use crate::components::typography::scale;
    use crate::theme::strata_theme;

    /// **The crosshair rules through the hovered mark, from the frame the paint recorded, over
    /// a real layout.**
    ///
    /// Two things only a mounted layout can prove. First, that the hairlines reach the screen
    /// at all: their geometry comes out of plotters rather than out of the tree, and a unit
    /// test on [`Mapping`] alone did not catch them landing a whole pane below the pointer.
    /// Second, that they sit on the **mark** — which is what makes the crosshair free. Freya
    /// repaints every node every frame and `CanvasElement::render` re-runs `on_render` each
    /// pass, so a crosshair that followed the pointer would replot the whole chart on every
    /// mouse sample; riding on the hover means the state changes only when the hovered mark
    /// does.
    #[test]
    fn a_crosshair_rules_through_the_hovered_mark_and_reads_its_row_off_the_axis() {
        let app = || {
            use_init_theme(|| strata_theme(&load("midnight")));
            let theme = get_theme!(
                &None::<super::super::ChartThemePartial>,
                super::super::ChartThemePreference,
                "chart"
            );
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .child(ChartCanvas::new(Frame {
                    data: ChartData::Bins(
                        (0..4)
                            .map(|i| ChartBin {
                                lo: f64::from(i),
                                hi: f64::from(i + 1),
                                count: 10 + i as u64,
                            })
                            .collect(),
                    ),
                    mark: ChartMark::Histogram,
                    log_y: false,
                    dress: Dress::new(&theme, &scale()),
                }))
        };
        let (mut runner, _) = TestingRunner::new(app, (800., 600.).into(), |_| {}, 1.);
        runner.sync_and_update();
        // The hit regions and the plot frame are recorded *by* the paint, and headless only
        // paints on demand.
        runner.render();
        // Inside the second bin, low enough to be over the bar rather than above it.
        runner.move_cursor((400., 500.));
        runner.sync_and_update();
        runner.sync_and_update();

        // The two hairlines: one column the height of the plot, one row its width. Found by
        // their geometry, which is the thing under test — they are placed from the frame the
        // paint recorded, not from the pane.
        let hairlines: Vec<Area> = runner.find_many(|node, _| {
            let area = node.layout().area;
            (area.width() == HAIRLINE || area.height() == HAIRLINE).then_some(area)
        });
        let column = *hairlines
            .iter()
            .find(|area| area.width() == HAIRLINE)
            .expect("no vertical hairline");
        let row = *hairlines
            .iter()
            .find(|area| area.height() == HAIRLINE)
            .expect("no horizontal hairline");
        assert!(
            column.height() > 300. && row.width() > 400.,
            "the hairlines are not the size of the plot frame: {column:?} {row:?}"
        );

        let labels: Vec<String> =
            runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()));
        // The readout is the *mark's* value — bin 2 of 4, counted 11 — because that is where
        // the row is ruled. A crosshair that followed the pointer would say something else.
        assert!(
            labels.iter().any(|text| text == "11"),
            "the readout does not name the hovered mark's value: {labels:?}"
        );

        // **The rules track the mark, not the pointer.** Moving within the same bar changes
        // neither — which is the property that makes this free, so it is asserted rather than
        // described.
        runner.move_cursor((410., 470.));
        runner.sync_and_update();
        let settled: Vec<Area> = runner.find_many(|node, _| {
            let area = node.layout().area;
            (area.width() == HAIRLINE || area.height() == HAIRLINE).then_some(area)
        });
        assert!(
            settled.contains(&column) && settled.contains(&row),
            "the crosshair moved with the pointer inside one mark: {settled:?}"
        );

        // Off every mark: no crosshair at all.
        runner.move_cursor((5., 5.));
        runner.sync_and_update();
        let after: Vec<Area> = runner.find_many(|node, _| {
            let area = node.layout().area;
            (area.width() == HAIRLINE || area.height() == HAIRLINE).then_some(area)
        });
        assert!(
            after.is_empty(),
            "the crosshair survived the pointer leaving the plot: {after:?}"
        );
    }

    /// A wedge is an arc, and every pie has one that crosses zero. Describing it as two wrapped
    /// bounds gave that one `to < from`, so an ordinary `from <= a < to` answered false for the
    /// whole of it: one dead wedge per pie, and a single-slice pie dead everywhere.
    #[test]
    fn every_wedge_of_a_pie_is_hittable_including_the_one_crossing_zero() {
        /// The wedges `marks::pie` would record for these shares, same sweep, same start.
        fn wedges(shares: &[f64]) -> Vec<Hit> {
            let total: f64 = shares.iter().sum();
            let mut angle = -std::f64::consts::FRAC_PI_2;
            shares
                .iter()
                .enumerate()
                .map(|(i, share)| {
                    let sweep = share / total * TAU;
                    let hit = Hit::Wedge {
                        center: (100., 100.),
                        radius: 50.,
                        from: angle.rem_euclid(TAU),
                        sweep,
                        label: format!("slice-{i}"),
                    };
                    angle += sweep;
                    hit
                })
                .collect()
        }

        for shares in [
            vec![1.],
            vec![0.5, 0.5],
            vec![0.6, 0.4],
            vec![0.1, 0.9],
            vec![0.25, 0.25, 0.25, 0.25],
            (1..=13).map(f64::from).collect(),
        ] {
            let hits = wedges(&shares);
            // Walk the rim at a radius inside every wedge and collect which one answers.
            let mut named = std::collections::HashSet::new();
            for step in 0..720 {
                let theta = f64::from(step) / 720. * TAU;
                let (x, y) = (
                    100. + (30. * theta.cos()) as f32,
                    100. + (30. * theta.sin()) as f32,
                );
                if let Some(hit) = hits.iter().find(|hit| hit.contains(x, y)) {
                    named.insert(hit.label().to_string());
                }
            }
            assert_eq!(
                named.len(),
                shares.len(),
                "every wedge of {shares:?} must answer somewhere; named {named:?}"
            );
        }
    }

    /// Outside the circle is nobody's wedge, however the arc is described.
    #[test]
    fn a_point_beyond_the_radius_is_not_in_any_wedge() {
        let hit = Hit::Wedge {
            center: (100., 100.),
            radius: 50.,
            from: 0.,
            sweep: TAU,
            label: "all".into(),
        };
        assert!(hit.contains(120., 100.));
        assert!(!hit.contains(160., 100.));
    }
}
