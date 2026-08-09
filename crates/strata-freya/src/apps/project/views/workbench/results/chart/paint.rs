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

use super::marks;
use super::ChartTheme;

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

/// Where the **live** paint leaves what it drew, for the pointer to read back. A plain `RefCell`
/// and not a `State`: nothing renders from it, so a write must not wake anything.
///
/// The visible plot's alone. `marks::draw` answers with its regions rather than writing them
/// here, so the offscreen capture behind Copy Image cannot leave the pointer testing against a
/// chart nobody is looking at.
type Hits = Rc<RefCell<Vec<Hit>>>;

/// Which mark the pointer is over, and where on it to say so.
///
/// `at` is the **mark's** anchor rather than the pointer's position, so this value changes only
/// when the hovered mark changes — which is what lets the `!=` guard in the pointer handler
/// suppress the other ninety-nine samples of a slow drag.
#[derive(Clone, PartialEq)]
struct Hover {
    label: String,
    at: (f32, f32),
}

/// How far the readout sits from its anchor, so it never covers the mark it names.
const READOUT_OFFSET: f32 = 14.;

/// Squared distance from a mark's anchor to the pointer — the tie-break when hit regions
/// overlap. Squared because only the ordering is wanted, and `total_cmp` needs a total order.
fn reach((ax, ay): (f32, f32), x: f32, y: f32) -> f32 {
    (ax - x).powi(2) + (ay - y).powi(2)
}

/// The plot itself: a `canvas` the size of its pane, repainted on demand, with a readout that
/// names whatever mark the pointer is over.
#[derive(PartialEq)]
pub struct ChartCanvas {
    /// Shared with the toolbar's Copy Image, which captures the very frame this paints
    /// ([`super::capture`]) — one value, so the two cannot describe different charts.
    frame: Rc<Frame>,
}

impl ChartCanvas {
    pub fn new(frame: Rc<Frame>) -> Self {
        Self { frame }
    }
}

impl Component for ChartCanvas {
    fn render(&self) -> impl IntoElement {
        let platform = use_hook(Platform::get);
        // Seeded rather than left empty: an empty slot filled by the effect below would leave
        // the first paint with nothing to draw, and the paint is what records the hit regions
        // the pointer reads, so a chart would not answer a hover until something else asked it
        // to repaint. The seed is a handle, so it costs a refcount and not a copy of the read.
        let mut slot = use_state({
            let seed = Rc::clone(&self.frame);
            move || seed
        });

        let hits: Hits = use_hook(|| Rc::new(RefCell::new(Vec::new())));
        let mut hover = use_state(|| None::<Hover>);
        let mut size = use_state(Size2D::default);
        let mut readout_size = use_state(Size2D::default);

        // `use_reactive` under the hood, because a `use_side_effect` closure is built once and
        // would capture the *first* frame forever (AGENTS.md §3).
        use_side_effect_with_deps(&self.frame, move |frame| {
            slot.set(Rc::clone(frame));
            // The repaint this asks for rebuilds every hit region, so whatever the readout was
            // naming is gone: keeping it would leave a label pinned over a mark that has moved
            // or is no longer drawn.
            hover.set(None);
            platform.send(UserEvent::RequestRedraw);
        });

        let plot = canvas(RenderCallback::new({
            let hits = Rc::clone(&hits);
            move |context| {
                let frame = Rc::clone(&slot.peek());
                // Replaced wholesale, never appended: what the pointer can be over is exactly
                // what this paint drew.
                *hits.borrow_mut() = marks::draw(
                    context.canvas,
                    context.font_collection,
                    context.size,
                    &frame,
                );
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
            let hits = Rc::clone(&hits);
            move |e: Event<PointerEventData>| {
                let at = e.element_location();
                let (x, y) = (at.x as f32, at.y as f32);
                // The **nearest** mark whose region contains the pointer, not the first one
                // pushed: hit regions are a fixed reach around a point, so two series that
                // pass within that reach overlap, and taking the first would make the later
                // series unnameable anywhere in the band.
                let found = hits
                    .borrow()
                    .iter()
                    .filter(|hit| hit.contains(x, y))
                    .min_by(|a, b| reach(a.anchor(), x, y).total_cmp(&reach(b.anchor(), x, y)))
                    .map(|hit| Hover {
                        label: hit.label().to_string(),
                        at: hit.anchor(),
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
                // showing through only where one was semi-transparent. `Relative(1)` is the
                // whole fix; `Overlay` is a jump for things that must clear the window, and a
                // readout only has to clear its own sibling.
                .layer(Layer::Relative(1))
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
            .maybe_child(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
