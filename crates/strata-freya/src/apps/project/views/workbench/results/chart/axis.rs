//! The chart's **axes**: the category coordinate the marks sit on, and the two numbers a
//! value axis needs (a nice maximum and an abbreviated tick).
//!
//! [`Categories`] is a plotters [`Ranged`], not a hand-rolled tick stack (`docs/CHART_SPEC.md`
//! §9 forbids one): it hands plotters its own key points, so every gridline and every tick
//! label lands **on a category** and can be labelled with that category's own text. Two
//! constructions, and the difference is what the read carried:
//!
//! - [`Categories::indexed`] — one slot per row, equally spaced. The fallback, and the only
//!   thing a bar or a pie ever uses (a bar has a width, so it needs a slot to sit in).
//! - [`Categories::placed`] — the row's own X position ([`Axis::positions`], epoch
//!   milliseconds for an instant), so an irregular time series draws with its real gaps. Only
//!   taken when the positions are **strictly increasing**, which is the case where "result
//!   order" and "value order" are the same order — otherwise placing marks by value would
//!   quietly re-order the axis the spec (§1.6) says is the user's.
//!
//! [`ValueCoord`] is the second one, and it exists for the same reason: the value axis is
//! linear or logarithmic depending on a view preference, and a coordinate that can be either
//! keeps that choice out of every signature downstream. Without it each mark would have to be
//! generic over its Y axis and split into a build half and a draw half.
//!
//! [`Axis::positions`]: strata_model::Axis::positions

use std::ops::Range;

use freya::plot::plotters::coord::ranged1d::{DefaultFormatting, KeyPointHint, Ranged};
use freya::plot::plotters::coord::types::RangedCoordf64;
use freya::plot::plotters::prelude::{IntoLogRange, LogCoord};
use strata_core::util::fmt_int;

/// The category axis: where each category sits, how wide one category's slot is, and the
/// span the axis covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Categories {
    at: Vec<f64>,
    slot: f64,
    span: Range<f64>,
}

impl Categories {
    /// Equally spaced: category `i` sits at `i`, with half a slot of air at each end so a bar
    /// against either edge is drawn whole rather than clipped by the frame.
    pub fn indexed(n: usize) -> Self {
        Self {
            at: (0..n).map(|i| i as f64).collect(),
            slot: 1.,
            span: -0.5..(n.max(1) as f64 - 0.5),
        }
    }

    /// The rows' own positions, when there are as many as there are categories, all present,
    /// all finite and strictly increasing — see the module note for why the last condition is
    /// not negotiable. `None` when any of that fails, and the caller falls back to
    /// [`Self::indexed`].
    pub fn placed(positions: Option<&Vec<Option<f64>>>, n: usize) -> Option<Self> {
        let positions = positions?;
        if positions.len() != n || n < 2 {
            return None;
        }
        let at: Vec<f64> = positions.iter().map(|p| p.unwrap_or(f64::NAN)).collect();
        if !at.iter().all(|v| v.is_finite()) || !at.windows(2).all(|w| w[0] < w[1]) {
            return None;
        }
        // The tightest gap, so a mark drawn with a slot's width can never overlap its
        // neighbour.
        let slot = at
            .windows(2)
            .map(|w| w[1] - w[0])
            .fold(f64::INFINITY, f64::min);
        // The same half-slot of air `indexed` leaves, so a mark at either extreme is drawn
        // whole — and so switching a settled table between bar (always indexed) and line
        // (placed where the positions allow) does not visibly move the data.
        let span = (at[0] - slot / 2.)..(at[n - 1] + slot / 2.);
        Some(Self { at, slot, span })
    }

    /// Where category `i` sits on the axis.
    pub fn at(&self, i: usize) -> f64 {
        self.at.get(i).copied().unwrap_or(0.)
    }

    /// How wide one category is, in axis units — what a bar's group divides up.
    pub fn slot(&self) -> f64 {
        self.slot
    }

    /// Which category sits exactly at `v`, for the label formatter. Exact equality is sound
    /// because every tick plotters asks us to label came out of [`Self::key_points`], which
    /// only ever returns values from `at`.
    pub fn index_at(&self, v: f64) -> Option<usize> {
        self.at.iter().position(|p| *p == v)
    }
}

impl Ranged for Categories {
    type FormatOption = DefaultFormatting;
    type ValueType = f64;

    fn map(&self, v: &f64, limit: (i32, i32)) -> i32 {
        let (from, to) = limit;
        let width = self.span.end - self.span.start;
        if !(width.is_finite() && width.abs() > f64::EPSILON) {
            return from;
        }
        let t = (v - self.span.start) / width;
        from + (f64::from(to - from) * t).round() as i32
    }

    /// Every category, thinned to at most `hint` of them by taking every *n*th — so labels
    /// stay evenly spread and the first category always keeps one.
    fn key_points<Hint: KeyPointHint>(&self, hint: Hint) -> Vec<f64> {
        let max = hint.max_num_points();
        if max == 0 || self.at.is_empty() {
            return Vec::new();
        }
        let stride = self.at.len().div_ceil(max).max(1);
        self.at.iter().copied().step_by(stride).collect()
    }

    fn range(&self) -> Range<f64> {
        self.span.clone()
    }
}

// ---- the value axis ----

/// The value axis a cartesian mark is built on: linear, or logarithmic when the user asked for
/// one and the data can carry it.
///
/// One coordinate with two arms rather than making every mark generic over its Y: the marks
/// would each have to split into a build half and a draw half to name two different
/// `ChartContext` types, and `hit_box`, `mesh` and `zero_baseline` would grow a second type
/// parameter apiece. [`Categories`] is already precedent for writing the coordinate we want.
///
/// `FormatOption = DefaultFormatting` is safe despite the `Debug` blanket it opts into:
/// [`mesh`](super::marks::mesh) always sets an explicit `y_label_formatter`, so plotters never
/// reaches for the default.
pub enum ValueCoord {
    Linear(RangedCoordf64),
    Log(LogCoord<f64>),
}

impl ValueCoord {
    pub fn linear(range: Range<f64>) -> Self {
        ValueCoord::Linear(range.into())
    }

    /// A logarithmic axis over `range`, whose ends are already decades ([`log_span`]).
    pub fn log(range: Range<f64>) -> Self {
        ValueCoord::Log(range.log_scale().into())
    }

    /// How a tick on this axis is labelled.
    ///
    /// A **linear** axis picks one unit from its span ([`ticks`]) — every gridline is the same
    /// magnitude, so `2 000` reads `2k` or does not, once, for all of them. A **logarithmic**
    /// axis is the case that rule was written against: its whole point is that the gridlines
    /// are decades apart, so each is abbreviated on its own terms.
    pub fn tick_label(&self) -> Box<dyn Fn(&f64) -> String> {
        match self {
            ValueCoord::Linear(coord) => Box::new(ticks(&coord.range())),
            ValueCoord::Log(_) => Box::new(|v: &f64| decade(*v)),
        }
    }
}

impl Ranged for ValueCoord {
    type FormatOption = DefaultFormatting;
    type ValueType = f64;

    fn map(&self, v: &f64, limit: (i32, i32)) -> i32 {
        match self {
            ValueCoord::Linear(coord) => coord.map(v, limit),
            ValueCoord::Log(coord) => coord.map(v, limit),
        }
    }

    fn key_points<Hint: KeyPointHint>(&self, hint: Hint) -> Vec<f64> {
        match self {
            ValueCoord::Linear(coord) => coord.key_points(hint),
            ValueCoord::Log(coord) => coord.key_points(hint),
        }
    }

    fn range(&self) -> Range<f64> {
        match self {
            ValueCoord::Linear(coord) => coord.range(),
            ValueCoord::Log(coord) => coord.range(),
        }
    }
}

/// The decade span covering the **positive finite** values in `values`, or `None` when there
/// are none.
///
/// A log axis deliberately skips the linear axis's [`nice_max`] rounding and its proportional
/// edge padding: both are arithmetic on a linear span, and applied to a range that covers
/// several orders of magnitude they push the bottom of the axis through zero — where a
/// logarithm does not go. Rounding out to whole decades is the log axis's own version of the
/// same idea, and it is what puts the gridlines on 1, 10, 100.
///
/// It is also the log version of [`EDGE_AIR`](super::marks): an extreme value is drawn *inside*
/// the frame rather than on it, so a bound that already sits exactly on a decade takes the next
/// one out. Without that the commonest log histogram there is — a long tail whose smallest
/// count is 1 — draws every one of its 1s as a bar of no height at all, on an axis floored at
/// exactly 1.
pub fn log_span(values: impl Iterator<Item = f64>) -> Option<Range<f64>> {
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    for value in values.filter(|v| v.is_finite() && *v > 0.) {
        low = low.min(value);
        high = high.max(value);
    }
    if !(low.is_finite() && high.is_finite()) {
        return None;
    }
    let floor = 10f64.powf(low.log10().floor());
    let ceil = 10f64.powf(high.log10().ceil());
    let mut start = if floor < low { floor } else { floor / 10. };
    let mut end = if ceil > high { ceil } else { ceil * 10. };
    // The same guard the linear axis needs, for the same reason: plotters derives key points by
    // dividing a span down, and a non-finite or inverted one never terminates. A value near
    // `f64::MAX` is what rounds a decade up to infinity, so it keeps the data's own bound.
    if !(start.is_finite() && start > 0.) {
        start = low;
    }
    if !end.is_finite() {
        end = high;
    }
    if end <= start {
        // One decade at the very top of f64's range: there is no decade above the data to
        // round out to, so the axis takes the one below it instead.
        start = (end / 10.).max(f64::MIN_POSITIVE);
    }
    // **A log axis is bounded by its ratio, not its difference** — and an unbounded ratio is a
    // hang, not a bad-looking axis. `LogCoord::key_points` takes `((end/start).ln()/ln(base))`
    // as a bold-tick count and then loops `while max_points < count / cnt { cnt += 1 }`; a
    // column holding both 1e-300 and 1e300 overflows that division to infinity, `as usize`
    // saturates it to `usize::MAX`, and the loop runs ~3.7e18 times on the render thread. The
    // caller falls back to a linear axis, which is the same non-blocking degradation a
    // non-positive value takes.
    if !(end / start).is_finite() {
        return None;
    }
    (end > start).then_some(start..end)
}

/// A **log** axis's tick: abbreviated per value rather than per axis.
///
/// See [`ValueCoord::tick_label`] — a decade axis has no single magnitude to choose a unit
/// from, so `1`, `1k` and `1M` are three gridlines of the same axis and each says its own size.
pub fn decade(v: f64) -> String {
    for (unit, suffix) in [(1e9, "B"), (1e6, "M"), (1e3, "k")] {
        if v.abs() >= unit {
            let scaled = v / unit;
            let text = format!("{scaled:.1}");
            return format!("{}{suffix}", text.strip_suffix(".0").unwrap_or(&text));
        }
    }
    plain(v)
}

/// Round `v` up to a "nice" axis maximum — 1, 2, 5 or 10 times a power of ten — so the value
/// axis ends on a number a reader recognises rather than on the data's own maximum.
///
/// **Never answers a non-finite number.** Rounding *up* can overflow (`2 x 1e308` is `inf`),
/// and an infinite axis bound is not a cosmetic problem: plotters derives its key points by
/// dividing the span down until it has few enough of them, and against an infinite span that
/// loop never terminates — on the render thread, which is the whole window.
pub fn nice_max(v: f64) -> f64 {
    if !v.is_finite() || v <= 0. {
        return 1.;
    }
    let power = 10f64.powf(v.log10().floor());
    let scaled = v / power;
    let step = if scaled <= 1. {
        1.
    } else if scaled <= 2. {
        2.
    } else if scaled <= 5. {
        5.
    } else {
        10.
    };
    let rounded = step * power;
    if rounded.is_finite() {
        rounded
    } else {
        v
    }
}

/// How a value axis covering `range` labels its ticks.
///
/// **Abbreviation is a property of the axis, not of a value.** `2 000` is `2k` on an axis that
/// spans thousands, and a lie on one spanning 2 000 to 2 024 — where every one of five
/// gridlines abbreviates to the same `2k` and the axis says nothing at all. So the unit is
/// chosen once, from the span, and only when the span is at least one unit wide; below that the
/// ticks are written out in full. (A year column against a measure is exactly this case, and
/// it is what a scatter's X axis usually holds.)
pub fn ticks(range: &Range<f64>) -> impl Fn(&f64) -> String {
    let span = (range.end - range.start).abs();
    let unit = if span >= 1e9 {
        Some((1e9, "B"))
    } else if span >= 1e6 {
        Some((1e6, "M"))
    } else if span >= 1e3 {
        Some((1e3, "k"))
    } else {
        None
    };
    move |v| match unit {
        Some((unit, suffix)) => {
            let scaled = v / unit;
            // A tick smaller than a tenth of the unit has no abbreviated form: `20 / 1000`
            // rounds to `0.0`, which prints as `0k` — and `-20` as `-0k`, which is not a
            // number at all. Below the unit's own resolution, write the tick out.
            if scaled.abs() < 0.1 {
                return plain(*v);
            }
            let text = format!("{scaled:.1}");
            format!("{}{suffix}", text.strip_suffix(".0").unwrap_or(&text))
        }
        None => plain(*v),
    }
}

/// A value written out: a whole number with the app's own thousands separators, anything else
/// to two **significant** figures.
///
/// Significant rather than absolute places, because an axis is not always about the numbers
/// people usually chart. Two absolute decimals erase a rate column outright — a range of
/// 0..0.004 gives five gridlines all captioned `0`, and a tick at 0.025 captioned `0.03`, which
/// is a gridline labelled with a number it is not.
pub fn plain(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let magnitude = v.abs();
    if v.fract() == 0. && magnitude < 1e15 {
        let sign = if v.is_sign_negative() && v != 0. {
            "-"
        } else {
            ""
        };
        return format!("{sign}{}", fmt_int(magnitude as u64));
    }
    // Two places from the first significant digit down, so 1.239 is `1.24` and 0.0031 is
    // `0.0031` rather than `0`.
    let places = if magnitude >= 1. {
        2
    } else {
        // How many zeros sit between the point and the first significant digit.
        let leading_zeros = (-magnitude.log10().floor()) as usize;
        (leading_zeros + 1).min(MAX_TICK_PLACES)
    };
    let text = format!("{v:.places$}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    // A value too small for any fixed notation would round to a bare `0`, which is the very
    // mislabelling this arm exists to avoid — say it in exponent form instead.
    if trimmed.trim_start_matches('-') == "0" {
        return format!("{v:e}");
    }
    trimmed.to_string()
}

/// How far a fixed-notation tick will go before [`plain`] gives up and uses an exponent.
const MAX_TICK_PLACES: usize = 12;

/// A value as the **hover readout** says it: never abbreviated, whatever the axis chose.
///
/// The axis may abbreviate because a tick has a few characters of room and its job is the shape
/// of the scale; a readout is the one place the exact figure is being asked for, and answering
/// `1.2k` there when the value is 1 234 is the whole point of hovering, missed.
pub fn readout(v: f64) -> String {
    plain(v)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn an_indexed_axis_puts_a_category_on_every_integer_and_pads_for_the_end_bars() {
        let cats = Categories::indexed(4);
        assert_eq!(cats.at(0), 0.);
        assert_eq!(cats.at(3), 3.);
        assert_eq!(cats.range(), -0.5..3.5);
        assert_eq!(cats.slot(), 1.);
    }

    #[test]
    fn key_points_thin_by_stride_and_label_back_to_their_own_category() {
        let cats = Categories::indexed(10);
        let points = cats.key_points(4usize);
        assert_eq!(points, [0., 3., 6., 9.]);
        assert_eq!(cats.index_at(6.), Some(6));
        assert_eq!(cats.index_at(6.5), None);
    }

    /// True placement is taken only where the result is already in X order — otherwise the
    /// value axis would re-order the rows the user asked for (spec §1.6).
    #[test]
    fn placement_needs_present_finite_strictly_increasing_positions() {
        let ok = vec![Some(10.), Some(40.), Some(41.)];
        let placed = Categories::placed(Some(&ok), 3).expect("ordered positions place");
        assert_eq!(placed.slot(), 1., "the tightest gap bounds a mark's width");
        assert_eq!(
            placed.range(),
            9.5..41.5,
            "half a slot of air at each end, the same as `indexed` — otherwise switching a \
             settled table between bar and line visibly moves every mark"
        );

        assert!(Categories::placed(Some(&vec![Some(3.), Some(1.)]), 2).is_none());
        assert!(Categories::placed(Some(&vec![Some(1.), Some(1.)]), 2).is_none());
        assert!(Categories::placed(Some(&vec![Some(1.), None]), 2).is_none());
        assert!(Categories::placed(Some(&vec![Some(1.), Some(2.)]), 3).is_none());
        assert!(Categories::placed(None, 3).is_none());
    }

    /// **An axis abbreviates, a value does not.** Whether `2 000` reads `2k` depends on what
    /// the axis spans: over thousands it is the right label, and over 2 000..2 024 it is the
    /// same label on every gridline, which is an axis that says nothing.
    #[test]
    fn abbreviation_is_chosen_from_the_span_not_the_value() {
        let wide = ticks(&(0.0..4_000.0));
        assert_eq!(wide(&1_200.), "1.2k");
        assert_eq!(wide(&0.), "0");

        let narrow = ticks(&(2_000.0..2_024.0));
        let labels: Vec<String> = [2_000., 2_006., 2_012., 2_018., 2_024.]
            .iter()
            .map(&narrow)
            .collect();
        assert_eq!(labels, ["2,000", "2,006", "2,012", "2,018", "2,024"]);
        assert_eq!(
            labels.iter().collect::<HashSet<_>>().len(),
            labels.len(),
            "every gridline of an axis has to carry its own number"
        );

        // A tick below a tenth of the unit has no abbreviated form — `20 / 1000` rounds to
        // `0.0`, which printed as `0k`, and `-20` as `-0k`, which is not a number.
        let wide_span = ticks(&(-1_000.0..1_000.0));
        assert_eq!(wide_span(&20.), "20");
        assert_eq!(wide_span(&-20.), "-20");
        assert_eq!(wide_span(&0.), "0");

        assert_eq!(ticks(&(0.0..8e6))(&3_400_000.), "3.4M");
        assert_eq!(ticks(&(0.0..8e9))(&5e9), "5B");
    }

    /// The readout is where the exact figure is being asked for, so it never abbreviates —
    /// answering `1.2k` for 1 234 is the whole point of hovering, missed.
    #[test]
    fn a_readout_says_the_number_where_an_axis_would_abbreviate_it() {
        assert_eq!(ticks(&(0.0..4_000.0))(&1_234.), "1.2k");
        assert_eq!(readout(1_234.), "1,234");
        assert_eq!(readout(-1_234.), "-1,234");
        assert_eq!(readout(0.), "0");
        assert_eq!(readout(2_400_000.), "2,400,000");
        assert_eq!(readout(0.0031), "0.0031");
        assert_eq!(readout(f64::NAN), "");
    }

    /// A small value keeps two **significant** figures. Two absolute places erased a rate
    /// column outright — every gridline of a 0..0.004 axis captioned `0`, and a tick at 0.025
    /// captioned `0.03`, which is a gridline labelled with a number it is not.
    #[test]
    fn a_value_below_one_is_never_rounded_away_to_zero() {
        assert_eq!(plain(0.5), "0.5");
        assert_eq!(plain(0.025), "0.025");
        assert_eq!(plain(0.0031), "0.0031");
        assert_eq!(plain(-0.0031), "-0.0031");
        assert_eq!(plain(0.000_08), "0.00008");
        assert_eq!(plain(1.239), "1.24");
        // Past what fixed notation can hold, it says so rather than reading as zero.
        assert_eq!(plain(1e-20), "1e-20");
        for v in [0.5, 0.025, 0.0031, 0.000_08, 1e-20] {
            assert_ne!(plain(v), "0", "{v} must not caption a gridline as zero");
        }
    }

    /// A non-finite axis bound is a **hang**, not a cosmetic problem: plotters derives its key
    /// points by dividing the span down until there are few enough, and that loop never ends
    /// against an infinite span — on the render thread.
    #[test]
    fn a_nice_maximum_is_always_finite() {
        for v in [1.5e308, 9e307, f64::MAX, 1e300] {
            let max = nice_max(v);
            assert!(max.is_finite(), "nice_max({v}) = {max}");
            assert!(max >= v, "nice_max({v}) = {max} must not shrink the data");
        }
    }

    /// **A log axis rounds out to decades, and it never reaches for zero.** The linear axis's
    /// padding is proportional arithmetic on a span, and over several orders of magnitude that
    /// pushes the bottom of the axis through zero — where a logarithm does not go.
    #[test]
    fn a_log_span_covers_its_data_in_whole_decades() {
        assert_eq!(log_span([3., 40., 900.].into_iter()), Some(1.0..1000.0));
        assert_eq!(log_span([0.05, 0.4].into_iter()), Some(0.01..1.0));
        // One value still gets a decade to sit in rather than a span of nothing.
        let one = log_span([7.].into_iter()).expect("a single value has a decade");
        assert!(one.start < 7. && one.end > 7., "{one:?}");

        // Zero, the negatives and the non-finite have no logarithm and are simply not in it.
        assert_eq!(log_span([-4., 0., 25.].into_iter()), Some(10.0..100.0));
        assert_eq!(log_span([-4., 0.].into_iter()), None);
        assert_eq!(log_span([f64::NAN, f64::INFINITY].into_iter()), None);
        assert_eq!(log_span(std::iter::empty()), None);
    }

    /// **A value on a decade boundary is drawn inside the frame, not on it** — the log axis's
    /// version of the linear one's edge air. Without it the commonest log histogram there is
    /// (a long tail bottoming out at a count of 1) draws every 1 as a bar of no height.
    #[test]
    fn a_bound_already_on_a_decade_takes_the_next_one_out() {
        assert_eq!(log_span([1., 900.].into_iter()), Some(0.1..1000.0));
        assert_eq!(log_span([3., 1_000.].into_iter()), Some(1.0..10_000.0));
        for values in [vec![1., 900.], vec![3., 1_000.], vec![100., 100.]] {
            let span = log_span(values.iter().copied()).expect("positive values span");
            let (low, high) = (values[0], values[values.len() - 1]);
            assert!(
                span.start < low.min(high) && span.end > low.max(high),
                "log_span({values:?}) = {span:?} draws an extreme on the frame"
            );
        }
    }

    /// A non-finite bound is a hang, not a bad-looking axis (see [`nice_max`]) — and rounding
    /// a decade *up* is exactly where one comes from.
    #[test]
    fn a_log_span_is_always_finite_and_ordered() {
        for values in [vec![f64::MAX], vec![1e-30, 1e30], vec![f64::MIN_POSITIVE]] {
            let span = log_span(values.iter().copied()).expect("positive values span");
            assert!(
                span.start.is_finite() && span.end.is_finite() && span.end > span.start,
                "log_span({values:?}) = {span:?}"
            );
            assert!(span.start > 0., "a log axis never reaches zero: {span:?}");
        }
    }

    /// **A log axis is bounded by its bounds' *ratio*, not their difference — and an unbounded
    /// ratio is a hang.** `LogCoord::key_points` turns `ln(end/start)/ln(10)` into a bold-tick
    /// count; an overflowed ratio saturates that `as usize` to `usize::MAX`, and the loop that
    /// divides it down then runs ~3.7e18 times on the render thread. So the span refuses
    /// instead, and the caller draws a linear axis — the same non-blocking fallback a
    /// non-positive value takes.
    #[test]
    fn a_span_whose_ratio_overflows_is_no_log_axis_at_all() {
        assert_eq!(log_span([1e-300, 1e300].into_iter()), None);
        assert_eq!(log_span([f64::MIN_POSITIVE, f64::MAX].into_iter()), None);
        // Wide, but a ratio f64 can still hold — that one is drawn.
        assert!(log_span([1e-150, 1e150].into_iter()).is_some());
    }

    /// A decade axis has no single magnitude to choose one unit from, so each gridline is
    /// abbreviated on its own terms — the deliberate opposite of [`ticks`].
    #[test]
    fn a_log_tick_abbreviates_per_value_where_a_linear_one_abbreviates_per_axis() {
        assert_eq!(decade(1.), "1");
        assert_eq!(decade(100.), "100");
        assert_eq!(decade(1_000.), "1k");
        assert_eq!(decade(2_500_000.), "2.5M");
        assert_eq!(decade(1e9), "1B");
        assert_eq!(decade(0.01), "0.01");

        // The same axis carries all of them, which is what a per-axis unit could not do.
        let span = log_span([1., 1e9].into_iter()).expect("a nine-decade span");
        let label = ValueCoord::log(span).tick_label();
        assert_eq!(
            [1., 1e3, 1e6, 1e9].map(|v| label(&v)),
            ["1", "1k", "1M", "1B"]
        );
    }

    #[test]
    fn a_value_axis_ends_on_a_nice_number() {
        assert_eq!(nice_max(0.), 1.);
        assert_eq!(nice_max(7.), 10.);
        assert_eq!(nice_max(1_800.), 2_000.);
        assert_eq!(nice_max(f64::NAN), 1.);
    }
}
