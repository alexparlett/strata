//! Chart vocabulary: the **request** a chart makes of a snapshot ([`ChartQuery`]) and the
//! chart-ready **answer** it gets back ([`ChartData`]). Produced by
//! `strata_core::engine::Engine::chart`, consumed by the results Chart surface
//! (`docs/CHART_SPEC.md` §5).
//!
//! This is data, not six code paths: a chart type is a **preset over a query algebra**
//! (`docs/CHART_FUNCTIONS.md` §2), so the request carries a group slot and a *list* of
//! measures rather than one `y`. Every core chart sends exactly one measure, and that is the
//! point — a box plot or a candlestick is extra measures on the same group, so widening the
//! list is additive where widening a single-`y` field would be a rewrite. The algebra's other
//! two slots (window ops, whole-set derived stats) are deliberately **not** scaffolded here;
//! they extend [`ChartQuery`] additively when the task that owns them picks them up
//! (AGENTS.md §5).
//!
//! [`ChartQuery`] is freya-query **cache identity**, which is why every field of it is
//! hashable and comparable — including [`Width`], whose whole reason for existing is that a
//! bin width is a float and cache identity may not be approximate.
//!
//! [`ChartData`] deliberately carries no "was it capped" flag beside a half-filled payload:
//! a refusal is [`ChartData::OverCap`], with no series to draw, because "honest boundaries"
//! (spec §1.4) means there is no such thing as a truncated chart to render.

use std::fmt;

/// Which aggregate a chart applies to one measure column.
///
/// All seven are DataFusion built-ins, bound at compile time through
/// `functions_aggregate::expr_fn` rather than looked up in the session's registry — so this
/// **is** the list of aggregates a chart can draw, and widening it is an arm here plus an arm
/// in the engine's `measure`. The reducers left out are left out on purpose: the two-column
/// family (`corr`, `regr_*`) and the ordered family (`first_value`, `nth_value`) need the
/// algebra's other slots, and a parameterised one (`percentile_cont(p)`) needs a payload
/// variant. `docs/CHART_FUNCTIONS.md` §1.1 maps the whole registry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AggFn {
    Sum,
    Avg,
    Min,
    Max,
    Count,
    Median,
    CountDistinct,
}

impl AggFn {
    /// How this aggregate of `y` reads in a legend, a tooltip or an axis title —
    /// `sum(amount)`, `count(distinct user)`.
    ///
    /// A measure with **no Y column** is a row count whichever function is selected, so it
    /// always reads `count(*)`: there is no other aggregate a missing Y could carry, and
    /// labelling it `sum(*)` would describe a query nobody ran.
    pub fn label(self, y: Option<&str>) -> String {
        match y {
            None => "count(*)".into(),
            Some(y) if self == AggFn::CountDistinct => format!("count(distinct {y})"),
            Some(y) => format!("{}({y})", self.name()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            AggFn::Sum => "sum",
            AggFn::Avg => "avg",
            AggFn::Min => "min",
            AggFn::Max => "max",
            AggFn::Count => "count",
            AggFn::Median => "median",
            AggFn::CountDistinct => "count distinct",
        }
    }
}

/// One value drawn per group: an aggregate, and the column it reduces (`None` = a row count).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Measure {
    pub y: Option<String>,
    pub agg_fn: AggFn,
}

impl Measure {
    /// How this measure reads in a legend.
    pub fn label(&self) -> String {
        self.agg_fn.label(self.y.as_deref())
    }
}

/// The width a temporal X axis is bucketed at — one rung of a ladder the engine walks when
/// it resolves a bucket automatically, and the whole set a stride control offers.
///
/// Every rung is expressible as an interval with a **single** non-zero component, which is
/// what keeps [`parts`](Stride::parts) unambiguous: DataFusion's `date_bin` treats an
/// interval carrying months as calendar-stepped and one carrying only days/nanos as a fixed
/// width, and it **rejects** one carrying both.
///
/// Deliberately no *week*: `date_bin`'s default origin is the Unix epoch, which was a
/// Thursday, so week bins would run Thursday-to-Wednesday — a wrong-looking axis that can
/// only be fixed by passing an origin, and an origin has to match the column's timezone.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stride {
    Minute,
    FiveMinutes,
    Hour,
    Day,
    Month,
    Quarter,
    Year,
}

impl Stride {
    /// This stride as an interval: `(months, days, nanos)` — exactly the three components
    /// of an Arrow `IntervalMonthDayNano`, which is what the engine bins with.
    pub fn parts(self) -> (i32, i32, i64) {
        match self {
            Stride::Minute => (0, 0, 60_000_000_000),
            Stride::FiveMinutes => (0, 0, 300_000_000_000),
            Stride::Hour => (0, 0, 3_600_000_000_000),
            Stride::Day => (0, 1, 0),
            Stride::Month => (1, 0, 0),
            Stride::Quarter => (3, 0, 0),
            Stride::Year => (12, 0, 0),
        }
    }

    /// The next rung up, or `None` at the top — the ladder both auto-resolution and a
    /// stride control walk.
    ///
    /// A `wider` chain rather than an `ALL` array because it is exhaustive by the
    /// compiler: a stride added without a place in the ladder fails to build, where a
    /// hand-written array would just quietly omit it.
    pub fn wider(self) -> Option<Stride> {
        match self {
            Stride::Minute => Some(Stride::FiveMinutes),
            Stride::FiveMinutes => Some(Stride::Hour),
            Stride::Hour => Some(Stride::Day),
            Stride::Day => Some(Stride::Month),
            Stride::Month => Some(Stride::Quarter),
            Stride::Quarter => Some(Stride::Year),
            Stride::Year => None,
        }
    }
}

impl fmt::Display for Stride {
    /// The body of the SQL interval literal this bins at (`interval '5 minutes'`), which is
    /// also how the stride reads in a control. One spelling, so a chart and the `GROUP BY`
    /// query scaffolded from it cannot describe different buckets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stride::Minute => "1 minute",
            Stride::FiveMinutes => "5 minutes",
            Stride::Hour => "1 hour",
            Stride::Day => "1 day",
            Stride::Month => "1 month",
            Stride::Quarter => "3 months",
            Stride::Year => "1 year",
        })
    }
}

/// A uniform bin width for a numeric X.
///
/// A width is a float and [`ChartQuery`] is a cache key, so this holds the `f64`'s **own
/// bits**: identity is then exact rather than approximate, which is the property a cache
/// needs and the reason the spec says the request carries no floats. Construction is the
/// gate — a width of zero, a negative one, a NaN or an infinity cannot bin anything, so
/// there is no [`Width`] that names one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Width(u64);

impl Width {
    pub fn new(width: f64) -> Option<Width> {
        (width.is_finite() && width > 0.0).then(|| Width(width.to_bits()))
    }

    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

impl fmt::Display for Width {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
    }
}

/// How an X axis is bucketed — the one control slot, whichever kind of column X holds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Bucket {
    /// A calendar/clock stride, for a temporal X.
    Time(Stride),
    /// A uniform width, for a numeric X.
    Width(Width),
}

/// One read of a snapshot, shaped for a chart. Resolved from the chart config + the result
/// schema, and carrying no UI types — this is what the engine answers and what the
/// freya-query entry is keyed by.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ChartQuery {
    /// The grouped read behind bar / line / area / pie — and, with more measures, behind
    /// every preset built on one `GROUP BY` (`docs/CHART_FUNCTIONS.md` §2).
    Aggregate {
        /// The category axis. `None` charts one category — the aggregate over everything,
        /// split only by `series` and by the measure list.
        x: Option<String>,
        /// Splits each category into one value per distinct value of this column.
        series: Option<String>,
        /// One drawn value per group, each its own series. Must not be empty.
        measures: Vec<Measure>,
        /// How X is bucketed. `None` leaves a temporal X to the engine's own resolution
        /// (reported back in the answer) and a numeric X grouped by raw value. A bucket of
        /// the wrong kind for the column is refused, never ignored.
        bucket: Option<Bucket>,
        /// How many aggregate rows — categories × series — the chart will draw before it
        /// refuses. Not a truncation point: over it, nothing is drawn (spec §7).
        group_cap: usize,
    },
    /// Raw points (scatter): no aggregation at all.
    Raw { x: String, y: String, cap: usize },
    /// Uniform-width bins over one numeric column, counted engine-side. `bins` of `None`
    /// lets the engine pick from the row count.
    Histogram { col: String, bins: Option<usize> },
}

/// One drawn series: a legend entry and its value at every category.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartSeries {
    /// How this series reads in a legend: the measure's own label, the distinct `series`
    /// value, or `value: measure` when a chart has both.
    ///
    /// A **label, not a key**. Two series can carry the same name for the same reason two
    /// categories can — a NULL and a literal `(null)` render alike — so a consumer addresses
    /// a series by its position, which is the documented contract on
    /// [`ChartData::Grouped`](ChartData), never by this string.
    pub name: String,
    /// One value per entry of [`ChartData::Grouped::categories`](ChartData), in that order.
    /// `None` is **no row for that cell** — an empty temporal bucket or numeric bin, or a
    /// (category, series) pair the data never contained. A renderer draws it as a gap and
    /// never interpolates across it.
    pub values: Vec<Option<f64>>,
}

/// One raw point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartPoint {
    pub x: f64,
    pub y: f64,
}

/// One histogram bin: the half-open interval `[lo, hi)` and how many rows fell in it. The
/// last bin of a set is closed at `hi`, so the maximum value has somewhere to land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartBin {
    pub lo: f64,
    pub hi: f64,
    pub count: u64,
}

/// What a refusal counted (spec §7) — the noun its message names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CapUnit {
    /// Aggregate rows: categories × series.
    Groups,
    /// Raw points.
    Points,
}

/// A chart-ready read of one snapshot.
#[derive(Clone, Debug, PartialEq)]
pub enum ChartData {
    /// The aggregated chart. `categories` is the X axis in draw order; every
    /// [`ChartSeries::values`] is exactly as long as it.
    ///
    /// `series` runs **measure by measure, in request order**, and within each measure by
    /// distinct `series` value ordered by what it measures. That order is the contract a
    /// multi-measure preset reads its parts back by — a candlestick's open/high/low/close
    /// are the four measures it asked for, in the order it asked.
    Grouped {
        categories: Vec<String>,
        series: Vec<ChartSeries>,
        /// How X was bucketed — `None` when it wasn't (a categorical X, or a numeric one
        /// grouped by raw value). Reported rather than assumed, because
        /// [`ChartQuery::Aggregate::bucket`](ChartQuery) may have been `None` and the
        /// engine chose.
        bucket: Option<Bucket>,
    },
    /// Raw points, **in no particular order**. The read applies no `ORDER BY`, and a snapshot
    /// scan is range-split above 10 MB, so the order rows arrive in is the scan's — the same
    /// reason a categorical axis does not order by it. A scatter draws marks, not a sequence,
    /// so nothing here needs one; anything that does must sort for itself.
    Points(Vec<ChartPoint>),
    /// Histogram bins, ascending and contiguous.
    Bins(Vec<ChartBin>),
    /// Refused: the read would have exceeded `cap` of `unit`. Carries no data at all —
    /// the chart is not drawn (spec §1.4, §7), and the surface offers the `GROUP BY`
    /// scaffold instead.
    ///
    /// `bucket` is the width that was in effect when it refused, so the guardrail can name
    /// the one thing that would make the chart fit (spec §7: "a temporal/numeric X also
    /// nudges to a wider bucket"). `None` when X was categorical or absent, where there is
    /// no bucket to widen.
    OverCap {
        unit: CapUnit,
        cap: usize,
        bucket: Option<Bucket>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A stride states its width once.** The interval the engine bins with and the
    /// literal the SQL scaffold writes are two renderings of one fact, and a chart whose
    /// scaffolded query buckets differently is worse than no scaffold at all.
    #[test]
    fn every_stride_reads_as_the_interval_it_bins_at() {
        let cases = [
            (Stride::Minute, (0, 0, 60_000_000_000i64), "1 minute"),
            (Stride::FiveMinutes, (0, 0, 300_000_000_000), "5 minutes"),
            (Stride::Hour, (0, 0, 3_600_000_000_000), "1 hour"),
            (Stride::Day, (0, 1, 0), "1 day"),
            (Stride::Month, (1, 0, 0), "1 month"),
            (Stride::Quarter, (3, 0, 0), "3 months"),
            (Stride::Year, (12, 0, 0), "1 year"),
        ];
        for (stride, parts, text) in cases {
            assert_eq!(stride.parts(), parts, "{stride:?}");
            assert_eq!(stride.to_string(), text, "{stride:?}");
        }
    }

    /// **Exactly one interval component is non-zero.** `date_bin` reads an interval
    /// carrying months as calendar-stepped, and **rejects** one that also carries days or
    /// nanoseconds ("does not support combination of month, day and nanosecond intervals").
    #[test]
    fn a_strides_interval_has_one_non_zero_component() {
        let mut stride = Some(Stride::Minute);
        while let Some(s) = stride {
            let (months, days, nanos) = s.parts();
            let set = [months != 0, days != 0, nanos != 0]
                .iter()
                .filter(|b| **b)
                .count();
            assert_eq!(set, 1, "{s:?} must set exactly one interval component");
            stride = s.wider();
        }
    }

    /// The ladder climbs and terminates — the auto-resolution loop walks it to widen a
    /// bucket until the chart fits, and a cycle or a gap would hang or strand it.
    #[test]
    fn the_stride_ladder_climbs_from_the_narrowest_to_the_top() {
        let mut seen = vec![Stride::Minute];
        while let Some(next) = seen.last().unwrap().wider() {
            assert!(!seen.contains(&next), "{next:?} repeats: the ladder cycles");
            seen.push(next);
        }
        assert_eq!(seen.last(), Some(&Stride::Year));
        assert_eq!(seen.len(), 7, "every stride is on the ladder");
    }

    /// **A width that cannot bin anything cannot be named.** Zero would divide by itself, a
    /// negative one walks backwards, and NaN breaks the `Eq`/`Hash` a cache key needs.
    #[test]
    fn a_width_exists_only_where_it_can_bin() {
        assert_eq!(Width::new(0.5).map(Width::get), Some(0.5));
        assert_eq!(Width::new(0.0), None);
        assert_eq!(Width::new(-1.0), None);
        assert_eq!(Width::new(f64::NAN), None);
        assert_eq!(Width::new(f64::INFINITY), None);
    }

    /// A width is cache identity, so equal widths must hash equally and unequal ones must
    /// not — the property a raw `f64` field cannot offer at all.
    #[test]
    fn equal_widths_are_the_same_cache_key() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        seen.insert(Width::new(0.25).unwrap());
        assert!(!seen.insert(Width::new(0.25).unwrap()), "one key");
        assert!(seen.insert(Width::new(0.5).unwrap()), "a different width");
    }

    /// A missing Y is a row count whatever the selected function says.
    #[test]
    fn a_measure_label_names_the_query_that_ran() {
        let label = |y: Option<&str>, agg_fn| {
            Measure {
                y: y.map(String::from),
                agg_fn,
            }
            .label()
        };
        assert_eq!(label(Some("amount"), AggFn::Sum), "sum(amount)");
        assert_eq!(
            label(Some("user"), AggFn::CountDistinct),
            "count(distinct user)"
        );
        assert_eq!(label(None, AggFn::Sum), "count(*)");
        assert_eq!(label(None, AggFn::Median), "count(*)");
    }
}
