//! The six marks, drawn through plotters' own `ChartBuilder` / series machinery
//! (`docs/CHART_SPEC.md` §5, §9) onto the Skia canvas via `PlotSkiaBackend`.
//!
//! Everything here is a **rendering** of an answer the engine already shaped: nothing is
//! aggregated, bucketed or re-ordered on the way to the screen, and a `None` value is a gap
//! that a line is cut at rather than interpolated across (spec §4). The dispatch keys on the
//! *data* shape first and uses the mark only to choose between the readings of a table, so a
//! frame whose mark and data have momentarily disagreed still paints something true.
//!
//! Coordinates are logical: `CanvasContext::size` is already divided by the scale factor and
//! the canvas is pre-scaled, so a `12` here is 12 logical pixels on any display.

use std::f64::consts::{FRAC_PI_2, TAU};
use std::mem::take;
use std::ops::Range;

use freya::plot::plotters::chart::{ChartBuilder, ChartContext};
use freya::plot::plotters::coord::cartesian::Cartesian2d;
use freya::plot::plotters::coord::ranged1d::ValueFormatter;
use freya::plot::plotters::coord::types::RangedCoordf64;
use freya::plot::plotters::coord::Shift;
use freya::plot::plotters::prelude::{
    AreaSeries, Circle, Color as PlotColor, DrawingArea, DrawingAreaErrorKind, IntoDrawingArea,
    IntoFont, LineSeries, Pie, RGBAColor, RGBColor, Ranged, Rectangle, TextStyle,
};
use freya::plot::PlotSkiaBackend;
use freya::plot::PlotSkiaBackendError;
use freya::prelude::{CanvasContext, Color};
use strata_model::{Axis, ChartBin, ChartData, ChartMark, ChartPoint, ChartSeries};

use strata_core::util::clip;

use super::axis::{nice_max, readout, ticks, Categories};
use super::paint::{Dress, Frame, Hit, Hits};

/// Anything drawn by this module.
type Plot = Result<(), DrawingAreaErrorKind<PlotSkiaBackendError>>;
/// The Skia-backed drawing area every mark is built on.
type Area<'a> = DrawingArea<PlotSkiaBackend<'a>, Shift>;

/// The plot's insets: room for the tick labels down the left and along the bottom, and air at
/// the other two edges (the canvas's `padL` / `padB` / `padT` / `padR`).
const Y_LABEL_AREA: i32 = 56;
const X_LABEL_AREA: i32 = 34;
const MARGIN_TOP: i32 = 12;
const MARGIN_RIGHT: i32 = 16;

/// Roughly how much width one X tick label needs before the next one starts crowding it —
/// what "thinned X labels" (spec §9) resolves to.
const X_LABEL_PITCH: f64 = 64.;
/// Horizontal gridlines, so a value axis reads at a glance without becoming a ruler.
const Y_LABELS: usize = 5;

/// Below this the plot has no room for its own furniture, let alone data.
const MIN_WIDTH: f32 = 120.;
const MIN_HEIGHT: f32 = 80.;

/// How much of a category's slot the bars in it leave empty, so neighbouring groups read as
/// groups.
const BAR_GAP: f64 = 0.22;
/// Up to this many categories, a line and an area also carry a dot per value — past it the
/// dots merge into a smear and only a value stranded between two gaps still gets one.
const POINT_MARKERS_MAX: usize = 60;
/// How much of a measurement axis's span is air, so a point at either extreme is drawn inside
/// the frame rather than on it.
const EDGE_AIR: f64 = 0.04;

/// How much of the pane's shorter side a pie's radius takes.
const PIE_RADIUS: f64 = 0.34;
/// How long an **axis tick's** label may be. The engine clips a label to `DISPLAY_CHARS`, which
/// is a *cell* budget (400) — a tick has one category's share of the axis, and the canvas
/// clips one to a dozen characters.
const AXIS_LABEL_CHARS: usize = 12;

/// Paint `frame` onto `context`'s canvas, recording into `hits` where each mark landed.
pub fn draw(context: &mut CanvasContext, frame: &Frame, hits: &Hits) {
    let mut marks = Vec::new();
    let size = context.size;
    if size.width < MIN_WIDTH || size.height < MIN_HEIGHT {
        // Too small to have drawn anything, so there is nothing to be over either.
        hits.borrow_mut().clear();
        return;
    }
    let area = PlotSkiaBackend::new(
        context.canvas,
        context.font_collection,
        (size.width as i32, size.height as i32),
    )
    .into_drawing_area();

    let dress = &frame.dress;
    let drawn = match &frame.data {
        ChartData::Table { axis, series } => match frame.mark {
            ChartMark::Pie => pie(&area, dress, axis, series, &mut marks),
            ChartMark::Line => lines(&area, dress, axis, series, false, &mut marks),
            ChartMark::Area => lines(&area, dress, axis, series, true, &mut marks),
            _ => bars(&area, dress, axis, series, &mut marks),
        },
        ChartData::Points(points) => scatter(&area, dress, points, &mut marks),
        ChartData::Bins(bins) => histogram(&area, dress, bins, &mut marks),
        // A refusal carries nothing to draw at all (spec §1.4) — the body renders the reason
        // in place of the canvas.
        ChartData::OverCap { .. } | ChartData::Duplicates { .. } => Ok(()),
    };
    if let Err(err) = drawn {
        // A plot that failed mid-draw leaves a part-drawn pane, which is the one thing the
        // surface's notice states cannot describe — so it is a warning, not a debug line. It
        // is not `error!` only because a resize drag would repeat it per frame.
        tracing::warn!("chart: {err}");
    }
    // Replaced wholesale, never appended: what the pointer can be over is exactly what this
    // paint drew.
    *hits.borrow_mut() = marks;
}

/// A mark's hit box from the two data coordinates that bound it, through plotters' own
/// mapping — never through a second copy of the layout arithmetic.
fn hit_box<X>(
    chart: &ChartContext<'_, PlotSkiaBackend<'_>, Cartesian2d<X, RangedCoordf64>>,
    a: (f64, f64),
    b: (f64, f64),
    label: String,
) -> Hit
where
    X: Ranged<ValueType = f64>,
{
    let area = chart.plotting_area();
    let (ax, ay) = area.map_coordinate(&a);
    let (bx, by) = area.map_coordinate(&b);
    Hit::Box {
        left: ax.min(bx) as f32,
        top: ay.min(by) as f32,
        right: ax.max(bx) as f32,
        bottom: ay.max(by) as f32,
        label,
    }
}

/// How far from a point the pointer still counts as on it. A line's vertex is a 2px dot, which
/// nobody can hit; this is the target around it.
const POINT_REACH: f32 = 7.;

/// A point's hit box: [`POINT_REACH`] in every direction from where it was drawn.
fn hit_point<X>(
    chart: &ChartContext<'_, PlotSkiaBackend<'_>, Cartesian2d<X, RangedCoordf64>>,
    at: (f64, f64),
    label: String,
) -> Hit
where
    X: Ranged<ValueType = f64>,
{
    let (x, y) = chart.plotting_area().map_coordinate(&at);
    Hit::Box {
        left: x as f32 - POINT_REACH,
        top: y as f32 - POINT_REACH,
        right: x as f32 + POINT_REACH,
        bottom: y as f32 + POINT_REACH,
        label,
    }
}

// ---- the cartesian marks ----

/// Grouped bars: one run per series, each category's slot split between them.
fn bars<'a>(
    area: &'a Area<'a>,
    dress: &Dress,
    axis: &Axis,
    series: &[ChartSeries],
    hits: &mut Vec<Hit>,
) -> Plot {
    let cats = Categories::indexed(axis.labels.len());
    let values = value_range(series.iter().flat_map(|s| s.values.iter().copied()));
    let mut chart = frame_on(area, cats.clone(), values.clone())?;
    mesh(
        &mut chart,
        dress,
        x_labels(area),
        &category_label(&cats, axis),
        &ticks(&values),
    )?;

    let base = 0f64.clamp(values.start, values.end);
    let lanes = series.len().max(1) as f64;
    let slot = cats.slot();
    let width = (slot * (1. - BAR_GAP)) / lanes;
    for (index, one) in series.iter().enumerate() {
        let color = rgba(dress.series(index));
        let left_of =
            |i: usize| cats.at(i) - slot / 2. + slot * BAR_GAP / 2. + index as f64 * width;
        let bars: Vec<(usize, f64, f64)> = one
            .values
            .iter()
            .enumerate()
            .filter_map(|(i, value)| Some((i, left_of(i), value.filter(|v| v.is_finite())?)))
            .collect();
        chart.draw_series(bars.iter().map(|(_, left, value)| {
            Rectangle::new([(*left, base), (left + width, *value)], color.filled())
        }))?;
        for (i, left, value) in &bars {
            hits.push(hit_box(
                &chart,
                (*left, base),
                (left + width, *value),
                point_label(axis, *i, one, *value),
            ));
        }
    }
    zero_baseline(&mut chart, dress, &cats, &values)
}

/// A line per series, optionally filled down to the baseline. A `None` cell ends the run it
/// was in: the next value starts a new one, so the gap stays a gap (spec §4).
fn lines<'a>(
    area: &'a Area<'a>,
    dress: &Dress,
    axis: &Axis,
    series: &[ChartSeries],
    filled: bool,
    hits: &mut Vec<Hit>,
) -> Plot {
    let count = axis.labels.len();
    // The rows' own X positions where the result is already in X order, so an irregular time
    // series draws with its real gaps; equally spaced otherwise (see `axis`).
    let cats = Categories::placed(axis.positions.as_ref(), count)
        .unwrap_or_else(|| Categories::indexed(count));
    let values = value_range(series.iter().flat_map(|s| s.values.iter().copied()));
    let mut chart = frame_on(area, cats.clone(), values.clone())?;
    mesh(
        &mut chart,
        dress,
        x_labels(area),
        &category_label(&cats, axis),
        &ticks(&values),
    )?;

    let base = 0f64.clamp(values.start, values.end);
    for (index, one) in series.iter().enumerate() {
        let color = rgba(dress.series(index));
        let runs = runs(&one.values, &cats);
        for run in &runs {
            if filled {
                chart.draw_series(
                    AreaSeries::new(run.iter().copied(), base, color.mix(0.14))
                        .border_style(color.stroke_width(2)),
                )?;
            } else {
                chart.draw_series(LineSeries::new(run.iter().copied(), color.stroke_width(2)))?;
            }
            // Dots while they still read as dots — and always for a value stranded between
            // two gaps, which no line segment would show at all.
            if count <= POINT_MARKERS_MAX || run.len() == 1 {
                chart.draw_series(
                    run.iter()
                        .map(|point| Circle::new(*point, 2, color.filled())),
                )?;
            }
        }
    }
    for one in series {
        for (i, value) in one.values.iter().enumerate() {
            let Some(value) = value.filter(|v| v.is_finite()) else {
                continue;
            };
            hits.push(hit_point(
                &chart,
                (cats.at(i), value),
                point_label(axis, i, one, value),
            ));
        }
    }
    zero_baseline(&mut chart, dress, &cats, &values)
}

/// Raw points over two measures — marks, not a sequence, so nothing is joined.
///
/// **Both** axes are measurement axes ([`data_range`]), not value axes: a scatter plots one
/// measure against another, and neither is a magnitude read against zero.
fn scatter<'a>(
    area: &'a Area<'a>,
    dress: &Dress,
    points: &[ChartPoint],
    hits: &mut Vec<Hit>,
) -> Plot {
    let xs = data_range(points.iter().map(|p| p.x));
    let ys = data_range(points.iter().map(|p| p.y));
    let mut chart = ChartBuilder::on(area)
        .margin_top(MARGIN_TOP)
        .margin_right(MARGIN_RIGHT)
        .x_label_area_size(X_LABEL_AREA)
        .y_label_area_size(Y_LABEL_AREA)
        .build_cartesian_2d(xs.clone(), ys.clone())?;
    mesh(&mut chart, dress, x_labels(area), &ticks(&xs), &ticks(&ys))?;

    let color = rgba(dress.series(0)).mix(0.55);
    chart.draw_series(
        points
            .iter()
            .map(|p| Circle::new((p.x, p.y), 3, color.filled())),
    )?;
    for p in points {
        hits.push(hit_point(
            &chart,
            (p.x, p.y),
            format!("{}, {}", readout(p.x), readout(p.y)),
        ));
    }
    Ok(())
}

/// The engine's bins, drawn at their real edges — the one mark whose X axis is a measurement
/// rather than a category.
fn histogram<'a>(
    area: &'a Area<'a>,
    dress: &Dress,
    bins: &[ChartBin],
    hits: &mut Vec<Hit>,
) -> Plot {
    let (Some(first), Some(last)) = (bins.first(), bins.last()) else {
        return Ok(());
    };
    let span = if last.hi > first.lo {
        first.lo..last.hi
    } else {
        first.lo..first.lo + 1.
    };
    let counts = value_range(bins.iter().map(|b| Some(b.count as f64)));
    let mut chart = ChartBuilder::on(area)
        .margin_top(MARGIN_TOP)
        .margin_right(MARGIN_RIGHT)
        .x_label_area_size(X_LABEL_AREA)
        .y_label_area_size(Y_LABEL_AREA)
        .build_cartesian_2d(span.clone(), counts.clone())?;
    mesh(
        &mut chart,
        dress,
        x_labels(area),
        &ticks(&span),
        &ticks(&counts),
    )?;

    let color = rgba(dress.series(0));
    chart
        .draw_series(bins.iter().map(|bin| {
            Rectangle::new([(bin.lo, 0.), (bin.hi, bin.count as f64)], color.filled())
        }))?;
    for bin in bins {
        hits.push(hit_box(
            &chart,
            (bin.lo, 0.),
            (bin.hi, bin.count as f64),
            format!(
                "{} to {}: {}",
                readout(bin.lo),
                readout(bin.hi),
                readout(bin.count as f64)
            ),
        ));
    }
    Ok(())
}

// ---- the one radial mark ----

/// One slice per category over a single measure.
///
/// A missing or zero value has no wedge to be — a zero-area slice is a slice of nothing, which
/// is arithmetic rather than a truncation. A **negative** value cannot be a wedge at all, and
/// dropping one would silently change the total every percentage is read against; that is
/// [`notice`](super::notice)'s refusal, checked before this is called, so by
/// the time a pie is drawn every value in it is one a wedge can represent.
fn pie(
    area: &Area,
    dress: &Dress,
    axis: &Axis,
    series: &[ChartSeries],
    hits: &mut Vec<Hit>,
) -> Plot {
    let Some(one) = series.first() else {
        return Ok(());
    };
    let drawn = pie_slices(one);
    if drawn.is_empty() {
        return Ok(());
    }
    let sizes: Vec<f64> = drawn.iter().map(|(_, value)| *value).collect();
    let colors: Vec<RGBColor> = (0..drawn.len()).map(|n| rgb(dress.slice(n))).collect();
    // No text around the wedges: what a colour means is the strip's legend, which is the one
    // place that both names every slice and has somewhere to scroll when there are 24 of them.
    let labels = vec![""; drawn.len()];

    let (width, height) = area.dim_in_pixel();
    let center = (width as i32 / 2, height as i32 / 2);
    let radius = f64::from(width.min(height)) * PIE_RADIUS;
    let mut wedges = Pie::new(&center, &radius, &sizes, &colors, &labels);
    // Start at twelve o'clock, the way every pie in the canvas does.
    wedges.start_angle(-90.);

    // The same sweep `Pie` walks, so a wedge and its hit region are the same wedge: it starts
    // at the same angle and advances by each slice's share of the total.
    let total: f64 = sizes.iter().sum();
    let mut angle = -FRAC_PI_2;
    for (i, value) in &drawn {
        let sweep = value / total * TAU;
        hits.push(Hit::Wedge {
            center: (center.0 as f32, center.1 as f32),
            radius: radius as f32,
            from: angle.rem_euclid(TAU),
            sweep,
            label: format!(
                "{}: {} ({:.0}%)",
                axis.labels.get(*i).map_or("", String::as_str),
                readout(*value),
                value / total * 100.
            ),
        });
        angle += sweep;
    }
    area.draw(&wedges)
}

/// What a mark says when the pointer is over it: which category it sits at, which series it
/// belongs to, and its value. The series is named because a grouped bar chart draws several
/// bars per category and nothing else on the plot says which is which.
fn point_label(axis: &Axis, i: usize, series: &ChartSeries, value: f64) -> String {
    let category = axis.labels.get(i).map_or("", String::as_str);
    let value = readout(value);
    match (category.is_empty(), series.name.is_empty()) {
        (true, true) => value,
        (true, false) => format!("{}: {value}", series.name),
        (false, true) => format!("{category} · {value}"),
        (false, false) => format!("{category} · {}: {value}", series.name),
    }
}

/// The slices a pie actually draws, in draw order: each one's category index and its value.
///
/// Shared with the strip's legend rather than repeated there, because the legend's whole job is
/// to say what a colour means — and two separate walks of the same values, filtering the same
/// way, is exactly how a legend comes to name the wrong wedge.
pub fn pie_slices(series: &ChartSeries) -> Vec<(usize, f64)> {
    series
        .values
        .iter()
        .enumerate()
        .filter_map(|(i, value)| Some((i, value.filter(|v| v.is_finite() && *v > 0.)?)))
        .collect()
}

// ---- shared furniture ----

/// The plot frame every cartesian mark is built in.
fn frame_on<'a>(
    area: &'a Area<'a>,
    x: Categories,
    y: Range<f64>,
) -> Result<
    ChartContext<'a, PlotSkiaBackend<'a>, Cartesian2d<Categories, RangedCoordf64>>,
    DrawingAreaErrorKind<PlotSkiaBackendError>,
> {
    ChartBuilder::on(area)
        .margin_top(MARGIN_TOP)
        .margin_right(MARGIN_RIGHT)
        .x_label_area_size(X_LABEL_AREA)
        .y_label_area_size(Y_LABEL_AREA)
        .build_cartesian_2d(x, y)
}

/// The gridlines, the axes and their labels. `light_line_style` is transparent because the
/// only horizontal rules we want are the ones a tick sits on.
fn mesh<'a, X>(
    chart: &mut ChartContext<'a, PlotSkiaBackend<'a>, Cartesian2d<X, RangedCoordf64>>,
    dress: &Dress,
    x_labels: usize,
    x_label: &dyn Fn(&f64) -> String,
    y_label: &dyn Fn(&f64) -> String,
) -> Plot
where
    X: Ranged<ValueType = f64> + ValueFormatter<f64>,
{
    chart
        .configure_mesh()
        .disable_x_mesh()
        .x_labels(x_labels)
        .y_labels(Y_LABELS)
        .bold_line_style(rgba(dress.grid))
        .light_line_style(rgba(Color::TRANSPARENT))
        .axis_style(rgba(dress.axis))
        .label_style(text(dress, &rgba(dress.tick)))
        .x_label_formatter(x_label)
        .y_label_formatter(y_label)
        .draw()
}

/// The rule at zero, drawn only when the data crosses it — otherwise the axis itself is the
/// baseline and a second line on top of it is noise.
fn zero_baseline<'a, X>(
    chart: &mut ChartContext<'a, PlotSkiaBackend<'a>, Cartesian2d<X, RangedCoordf64>>,
    dress: &Dress,
    cats: &Categories,
    values: &Range<f64>,
) -> Plot
where
    X: Ranged<ValueType = f64>,
{
    if values.start >= 0. {
        return Ok(());
    }
    let span = cats.range();
    chart.draw_series(LineSeries::new(
        [(span.start, 0.), (span.end, 0.)],
        rgba(dress.axis).stroke_width(1),
    ))?;
    Ok(())
}

/// Label a category tick with the category's own text (`Axis::labels`), which is exactly what
/// [`Categories`] hands plotters as key points — clipped to a tick's share of the axis, since
/// the engine's own clip is a cell's budget (400 characters) and not a tick's.
fn category_label<'a>(cats: &'a Categories, axis: &'a Axis) -> impl Fn(&f64) -> String + 'a {
    move |value| {
        cats.index_at(*value)
            .and_then(|i| axis.labels.get(i))
            .map(|label| clip(label, AXIS_LABEL_CHARS).into_owned())
            .unwrap_or_default()
    }
}

/// A **measurement** axis: the data's own span, with a little air at each end so a point at an
/// extreme is not drawn on the frame.
///
/// Deliberately not [`value_range`], and the difference is the whole point of having two. A
/// value axis is read against zero, because the length of a bar and the height of a line *are*
/// the magnitude. A scatter's X and Y are measurements — years, latitudes, prices — and
/// anchoring one at zero pushes every point into a corner of an axis that mostly covers values
/// the result never contained (`year` in 2000..2024 would draw on an axis of 0..5 000).
fn data_range(values: impl Iterator<Item = f64>) -> Range<f64> {
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    for value in values.filter(|v| v.is_finite()) {
        low = low.min(value);
        high = high.max(value);
    }
    if !(low.is_finite() && high.is_finite()) {
        // Nothing finite to span. The surface shows a notice instead of this plot, so the
        // range only has to be one an axis can be built on.
        return 0.0..1.0;
    }
    // A span of nothing is every point at one value: give it a window rather than a zero-width
    // axis plotters cannot map into.
    let pad = if high > low {
        (high - low) * EDGE_AIR
    } else {
        low.abs().max(1.) * 0.05
    };
    finite_span(low - pad, high + pad, low, high)
}

/// `start..end`, or the unpadded span it was derived from when padding pushed it out of range.
///
/// Both ends of a measurement axis are arithmetic on values that are only *individually* known
/// to be finite: `1e308 - -1e308` is already infinite before any padding, and an infinite or
/// NaN bound is not cosmetic — plotters derives key points by dividing the span down until
/// there are few enough of them, and against a non-finite span that loop never ends. It runs
/// on the render thread, so the window stops.
fn finite_span(start: f64, end: f64, fallback_start: f64, fallback_end: f64) -> Range<f64> {
    if start.is_finite() && end.is_finite() && end > start {
        return start..end;
    }
    if fallback_start.is_finite() && fallback_end.is_finite() && fallback_end > fallback_start {
        return fallback_start..fallback_end;
    }
    0.0..1.0
}

/// The value axis: zero (or a nice negative floor) up to a nice maximum, never a zero-height
/// span.
fn value_range(values: impl Iterator<Item = Option<f64>>) -> Range<f64> {
    let (mut low, mut high) = (0f64, 0f64);
    for value in values.flatten().filter(|v| v.is_finite()) {
        low = low.min(value);
        high = high.max(value);
    }
    let start = if low < 0. { -nice_max(-low) } else { 0. };
    let end = nice_max(high);
    // `nice_max` never answers non-finite, but the pair still has to be a usable span — see
    // `finite_span` for why an unusable one is a hang rather than a bad-looking axis.
    finite_span(start, end, start, start + 1.)
}

/// One series' values cut into runs of consecutive present values — the gaps are where the
/// runs end.
fn runs(values: &[Option<f64>], cats: &Categories) -> Vec<Vec<(f64, f64)>> {
    let mut runs = Vec::new();
    let mut run: Vec<(f64, f64)> = Vec::new();
    for (i, value) in values.iter().enumerate() {
        match value.filter(|v| v.is_finite()) {
            Some(value) => run.push((cats.at(i), value)),
            None if !run.is_empty() => runs.push(take(&mut run)),
            None => {}
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

/// How many X ticks the plot has room for, at [`X_LABEL_PITCH`] each.
fn x_labels(area: &Area) -> usize {
    let (width, _) = area.dim_in_pixel();
    let plot = f64::from(width) - f64::from(Y_LABEL_AREA + MARGIN_RIGHT);
    ((plot / X_LABEL_PITCH).floor() as usize).max(1)
}

/// The theme's small mono, in `color` — the one text style the plot draws in.
///
/// `color` is borrowed for the style's own lifetime (plotters keeps the reference), so the
/// caller holds the converted colour rather than passing a temporary.
fn text<'a>(dress: &'a Dress, color: &'a RGBAColor) -> TextStyle<'a> {
    TextStyle::from((dress.label.0.as_str(), dress.label.1).into_font()).color(color)
}

/// A themed colour as plotters sees it, alpha included.
fn rgba(color: Color) -> RGBAColor {
    RGBAColor(color.r(), color.g(), color.b(), f64::from(color.a()) / 255.)
}

/// A themed colour with its alpha dropped — what [`Pie`] takes, since a wedge is opaque.
fn rgb(color: Color) -> RGBColor {
    RGBColor(color.r(), color.g(), color.b())
}

#[cfg(test)]
mod tests {
    use std::iter::empty;

    use super::*;

    #[test]
    fn a_value_axis_covers_zero_and_never_collapses() {
        assert_eq!(value_range([Some(3.), Some(7.)].into_iter()), 0.0..10.0);
        assert_eq!(value_range([Some(-3.), Some(7.)].into_iter()), -5.0..10.0);
        assert_eq!(
            value_range([None, Some(f64::NAN)].into_iter()),
            0.0..1.0,
            "an empty or non-finite series still has an axis"
        );
    }

    /// A missing value ends the run it was in, so a line is cut rather than interpolated
    /// across it (CHART_SPEC §4).
    /// A measurement axis is arithmetic on values that are only *individually* finite, and a
    /// non-finite bound hangs the render thread inside plotters' key-point loop.
    #[test]
    fn a_measurement_axis_is_always_finite_and_ordered() {
        for values in [
            vec![-1e308, 1e308],
            vec![f64::MAX, f64::MIN],
            vec![1e308, 1e308],
            vec![0., 1.],
        ] {
            let span = data_range(values.iter().copied());
            assert!(
                span.start.is_finite() && span.end.is_finite() && span.end > span.start,
                "data_range({values:?}) = {span:?}"
            );
        }
        let span = value_range([Some(f64::MAX), Some(-f64::MAX)].into_iter());
        assert!(
            span.start.is_finite() && span.end.is_finite() && span.end > span.start,
            "value_range over the f64 extremes = {span:?}"
        );
    }

    #[test]
    fn a_gap_cuts_the_run_it_falls_in() {
        let cats = Categories::indexed(5);
        let values = [Some(1.), None, Some(3.), Some(4.), None];
        assert_eq!(
            runs(&values, &cats),
            vec![vec![(0., 1.)], vec![(2., 3.), (3., 4.)]]
        );
        assert!(runs(&[None, None], &cats).is_empty());
    }

    /// A tick has one category's share of the axis. The engine's own clip is a *cell*'s 400
    /// characters, which under a tick is a wall of text. The budget is the crate's one
    /// clipping path, so it keeps `max` characters and adds the ellipsis after them.
    #[test]
    fn a_long_axis_label_is_clipped_to_the_space_a_tick_has() {
        assert_eq!(clip("short", AXIS_LABEL_CHARS), "short");
        assert_eq!(
            clip("a-very-long-category-name", AXIS_LABEL_CHARS),
            "a-very-long-…"
        );
    }

    /// A **measurement** axis spans the data, not zero-to-data. Anchoring a scatter's X at zero
    /// drew `year` in 2000..2024 on an axis of 0..5 000, with every point in one pixel column.
    #[test]
    fn a_measurement_axis_spans_the_data_rather_than_reaching_back_to_zero() {
        let years = data_range([2000., 2024.].into_iter());
        assert!(
            years.start > 1990. && years.end < 2035.,
            "a measurement axis stays around its data, got {years:?}"
        );
        assert!(
            years.start < 2000. && years.end > 2024.,
            "with air at both ends so an extreme point is inside the frame, got {years:?}"
        );

        // One value repeated is a span of nothing, which no axis can be built on.
        let flat = data_range([51.5, 51.5].into_iter());
        assert!(flat.start < 51.5 && flat.end > 51.5, "{flat:?}");

        // Nothing finite is still an axis — the surface shows a notice in place of the plot.
        assert_eq!(data_range([f64::NAN, f64::INFINITY].into_iter()), 0.0..1.0);
        assert_eq!(data_range(empty()), 0.0..1.0);
    }
}
