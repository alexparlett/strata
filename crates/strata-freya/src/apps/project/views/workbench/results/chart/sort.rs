//! The strip's **sort**: a view transform over the settled data, not part of the read
//! (`docs/CHART_SPEC.md` §6).
//!
//! Result order is the chart's default and it is real (spec §1.6) — the order the user's own
//! query produced, carried by the snapshot ordinal. Re-ordering is therefore something the
//! *user* asks for, on data already in hand: flipping the toggle permutes the settled
//! [`ChartData`] and repaints, with no new engine read and no change to cache identity.
//!
//! Only [`ChartData::Table`] has an order to permute. A scatter's points are documented
//! unordered (a scatter draws marks, not a sequence) and a histogram's bins are ascending by
//! construction, so the strip does not offer the control for either mark
//! ([`sortable`](super::config::sortable)) and this pass leaves every other shape alone.
//!
//! **Every comparison here is total.** The withdrawn aggregation pipeline panicked in a
//! `sort_by` on a NaN weight; a chart's values are `f64`s straight out of the user's data, so
//! `total_cmp` and an explicit place for the missing ones are not defensive, they are the
//! only way this cannot panic on a column it has never seen.

use std::cmp::Ordering;

use strata_model::{Axis, ChartData, ChartSeries, ChartSort};

/// `data`, reordered as `sort` asks. Result order is the identity, which is also what every
/// non-table shape gets.
pub fn sorted(data: ChartData, sort: ChartSort) -> ChartData {
    let (axis, series) = match data {
        ChartData::Table { axis, series } => (axis, series),
        other => return other,
    };
    let Some(order) = order(&axis, &series, sort) else {
        return ChartData::Table { axis, series };
    };
    ChartData::Table {
        axis: Axis {
            labels: permute(&axis.labels, &order),
            positions: axis.positions.map(|p| permute(&p, &order)),
        },
        series: series
            .into_iter()
            .map(|one| ChartSeries {
                name: one.name,
                values: permute(&one.values, &order),
            })
            .collect(),
    }
}

/// The permutation `sort` asks for, or `None` when the data is already in it.
///
/// **Stable**, so equal keys keep result order — which makes the sort a refinement of the
/// order the user's query produced rather than a reshuffle of it.
fn order(axis: &Axis, series: &[ChartSeries], sort: ChartSort) -> Option<Vec<usize>> {
    let mut order: Vec<usize> = (0..axis.labels.len()).collect();
    match sort {
        ChartSort::ResultOrder => return None,
        // By X's true value where it has one (a numeric or temporal axis carries positions),
        // else by the label — which for a categorical axis is the only value there is.
        ChartSort::ByX => match &axis.positions {
            Some(positions) => order.sort_by(|a, b| {
                compare(
                    positions.get(*a).copied().flatten(),
                    positions.get(*b).copied().flatten(),
                    false,
                )
            }),
            None => order.sort_by(|a, b| {
                axis.labels
                    .get(*a)
                    .map(String::as_str)
                    .cmp(&axis.labels.get(*b).map(String::as_str))
            }),
        },
        // By the **first** series: with several plotted there is no one value to a category,
        // and the first is the one the legend leads with.
        ChartSort::ByYDesc => {
            let values = series.first().map(|one| &one.values)?;
            order.sort_by(|a, b| {
                compare(
                    values.get(*a).copied().flatten(),
                    values.get(*b).copied().flatten(),
                    true,
                )
            });
        }
    }
    Some(order)
}

/// A total order over values that may be missing or NaN.
///
/// `descending` flips **the values only**: a gap is not a small value, so it sorts last either
/// way rather than heading a descending chart. That is why the direction is a flag here and not
/// a `reverse()` at the call site — reversing the comparison reverses where the gaps go with
/// it, which is exactly the bug this signature exists to make unwritable.
fn compare(a: Option<f64>, b: Option<f64>, descending: bool) -> Ordering {
    match (a.filter(|v| !v.is_nan()), b.filter(|v| !v.is_nan())) {
        (Some(a), Some(b)) if descending => b.total_cmp(&a),
        (Some(a), Some(b)) => a.total_cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// `values`, in `order`. Every vector here is documented as long as the axis, but a series
/// shorter than one would panic on an index — and a chart is not the place to find out.
fn permute<T: Clone + Default>(values: &[T], order: &[usize]) -> Vec<T> {
    order
        .iter()
        .map(|i| values.get(*i).cloned().unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(
        labels: &[&str],
        positions: Option<&[Option<f64>]>,
        values: &[Option<f64>],
    ) -> ChartData {
        ChartData::Table {
            axis: Axis {
                labels: labels.iter().map(|l| (*l).to_string()).collect(),
                positions: positions.map(|p| p.to_vec()),
            },
            series: vec![ChartSeries {
                name: "amount".into(),
                values: values.to_vec(),
            }],
        }
    }

    fn parts(data: ChartData) -> (Vec<String>, Vec<Option<f64>>) {
        let ChartData::Table { axis, series } = data else {
            panic!("a table stays a table");
        };
        (axis.labels, series[0].values.clone())
    }

    /// Result order is the identity — the order the user's query produced, untouched.
    #[test]
    fn result_order_leaves_the_rows_exactly_as_the_query_produced_them() {
        let data = table(&["b", "a", "c"], None, &[Some(2.), Some(1.), Some(3.)]);
        assert_eq!(sorted(data.clone(), ChartSort::ResultOrder), data);
    }

    /// A categorical axis sorts by its labels; the series follows its own axis, which is the
    /// whole point — a permutation applied to one and not the other silently relabels the
    /// chart.
    #[test]
    fn by_x_sorts_a_categorical_axis_by_label_and_carries_the_values_with_it() {
        let (labels, values) = parts(sorted(
            table(&["b", "a", "c"], None, &[Some(2.), Some(1.), Some(3.)]),
            ChartSort::ByX,
        ));
        assert_eq!(labels, ["a", "b", "c"]);
        assert_eq!(values, [Some(1.), Some(2.), Some(3.)]);
    }

    /// Where X has a true position — a numeric or temporal axis — that is what "ascending"
    /// means, not the rendering. `"10"` sorts after `"9"` by value and before it by label.
    #[test]
    fn by_x_sorts_by_position_when_the_axis_has_one() {
        let (labels, _) = parts(sorted(
            table(
                &["10", "9", "100"],
                Some(&[Some(10.), Some(9.), Some(100.)]),
                &[Some(1.), Some(2.), Some(3.)],
            ),
            ChartSort::ByX,
        ));
        assert_eq!(labels, ["9", "10", "100"]);
    }

    /// The axis's positions are permuted with its labels — a mark placed by position after a
    /// sort that moved only the labels would land on the wrong category.
    #[test]
    fn by_x_permutes_the_positions_with_the_labels() {
        let ChartData::Table { axis, .. } = sorted(
            table(
                &["b", "a"],
                Some(&[Some(2.), Some(1.)]),
                &[Some(9.), Some(8.)],
            ),
            ChartSort::ByX,
        ) else {
            panic!("a table stays a table");
        };
        assert_eq!(axis.labels, ["a", "b"]);
        assert_eq!(axis.positions, Some(vec![Some(1.), Some(2.)]));
    }

    /// **A gap is not a value.** A NULL cell and a NaN both sort last under a descending
    /// value order rather than heading the chart — and neither panics the comparison, which
    /// is the lesson the withdrawn pipeline left behind.
    #[test]
    fn by_value_puts_the_biggest_first_and_the_missing_last() {
        let (labels, values) = parts(sorted(
            table(
                &["a", "b", "c", "d"],
                None,
                &[Some(2.), None, Some(f64::NAN), Some(7.)],
            ),
            ChartSort::ByYDesc,
        ));
        assert_eq!(labels[..2], ["d", "a"]);
        assert_eq!(values[..2], [Some(7.), Some(2.)]);
        assert!(
            values[2..]
                .iter()
                .copied()
                .all(|v| v.is_none_or(f64::is_nan)),
            "the gap and the NaN are what is left, at the end"
        );
    }

    /// Equal values keep result order, so the sort refines the query's own order rather than
    /// reshuffling the rows it has nothing to say about.
    #[test]
    fn equal_values_keep_the_order_the_query_produced() {
        let (labels, _) = parts(sorted(
            table(&["b", "a", "c"], None, &[Some(1.), Some(1.), Some(1.)]),
            ChartSort::ByYDesc,
        ));
        assert_eq!(labels, ["b", "a", "c"]);
    }

    /// Nothing else has an order to permute: points are documented unordered and bins are
    /// ascending by construction, so both come back as they went in.
    #[test]
    fn a_shape_with_no_order_of_its_own_is_left_alone() {
        use strata_model::{ChartBin, ChartPoint};

        let points = ChartData::Points(vec![
            ChartPoint { x: 2., y: 1. },
            ChartPoint { x: 1., y: 2. },
        ]);
        assert_eq!(sorted(points.clone(), ChartSort::ByX), points);

        let bins = ChartData::Bins(vec![ChartBin {
            lo: 0.,
            hi: 1.,
            count: 3,
        }]);
        assert_eq!(sorted(bins.clone(), ChartSort::ByYDesc), bins);

        // A refusal carries no data at all, so there is nothing to reorder either.
        let refused = ChartData::Duplicates {
            x: "month".into(),
            series: "region".into(),
        };
        assert_eq!(sorted(refused.clone(), ChartSort::ByX), refused);
    }
}
