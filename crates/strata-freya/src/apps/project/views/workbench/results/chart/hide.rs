//! The legend's **hidden series**: a view transform over the settled data, in
//! [`sort`](super::sort)'s class — a repaint, never a re-read, and never part of cache identity.
//!
//! **A hidden series keeps its slot.** Its values are blanked to all-`None` rather than the
//! series being dropped, and that is the whole design: a series' colour comes from its
//! *position* ([`Dress::series`](super::paint::Dress::series)), so removing one would recolour
//! every series after it — press a legend row and the chart changes colour under you. Blanking
//! costs nothing downstream either: a `None` cell is already "no value in this cell" and every
//! mark draws it as a gap, and hit regions are built per finite value, so
//! [`marks`](super::marks) needs no idea that any of this happened. A grouped bar chart keeps
//! an empty lane where the hidden series was, which is the accepted v1 cost of stable colour.
//!
//! **By name, not by position.** [`ChartSeries::name`] is documented a label rather than a key,
//! so a NULL-valued series and a literal `"(null)"` one toggle together. That is accepted
//! coarseness: the name is what the user pressed, and a legend keyed by position would forget
//! the choice the moment a column is added to the SELECT list.
//!
//! Only a [`ChartData::Table`] has named series. A scatter and a histogram draw in one colour
//! by construction and key no legend, so nothing here can reach them.
//!
//! A **pie** is the one shape that would be reached and must not be: it is a `Table`, and its Y
//! is an ordinary measure that a bar may well have hidden earlier — so applying the set there
//! would empty the pie with no control on screen to bring it back, and hiding a slice would
//! silently recompute every remaining percentage anyway. The caller gates on
//! [`hideable`](super::config::hideable) rather than this module second-guessing the mark.

use strata_model::{ChartData, ChartSeries};

/// `data` with every series named in `hidden` blanked out. Nothing is removed, so positions —
/// and therefore colours — are exactly what they were.
pub fn applied(data: ChartData, hidden: &[String]) -> ChartData {
    if hidden.is_empty() {
        return data;
    }
    let (axis, series) = match data {
        ChartData::Table { axis, series } => (axis, series),
        other => return other,
    };
    ChartData::Table {
        axis,
        series: series
            .into_iter()
            .map(|one| {
                if hidden.contains(&one.name) {
                    ChartSeries {
                        values: vec![None; one.values.len()],
                        name: one.name,
                    }
                } else {
                    one
                }
            })
            .collect(),
    }
}

/// Whether every series this table draws is hidden — the state the canvas has to say something
/// about, because the alternative is an empty frame that looks exactly like a broken chart.
///
/// Asked of the *names*, not of the blanked values, so a result whose every value is genuinely
/// NULL is not mistaken for one the user pressed out.
pub fn all_hidden(data: &ChartData, hidden: &[String]) -> bool {
    let ChartData::Table { series, .. } = data else {
        return false;
    };
    !series.is_empty() && series.iter().all(|one| hidden.contains(&one.name))
}

#[cfg(test)]
mod tests {
    use strata_model::{Axis, ChartBin};

    use super::*;

    fn table(names: &[&str]) -> ChartData {
        ChartData::Table {
            axis: Axis {
                labels: vec!["a".into(), "b".into()],
                positions: None,
            },
            series: names
                .iter()
                .enumerate()
                .map(|(i, name)| ChartSeries {
                    name: (*name).to_string(),
                    values: vec![Some(i as f64), Some(i as f64 + 1.)],
                })
                .collect(),
        }
    }

    fn series_of(data: &ChartData) -> Vec<(String, Vec<Option<f64>>)> {
        let ChartData::Table { series, .. } = data else {
            panic!("a table stays a table");
        };
        series
            .iter()
            .map(|one| (one.name.clone(), one.values.clone()))
            .collect()
    }

    /// **A hidden series keeps its slot.** Dropping it instead would shift every later series
    /// down the colour ramp, so pressing a legend row would recolour the chart under the user.
    #[test]
    fn hiding_blanks_a_series_in_place_rather_than_removing_it() {
        let drawn = applied(table(&["revenue", "cost", "margin"]), &["cost".to_string()]);
        assert_eq!(
            series_of(&drawn),
            [
                ("revenue".to_string(), vec![Some(0.), Some(1.)]),
                ("cost".to_string(), vec![None, None]),
                ("margin".to_string(), vec![Some(2.), Some(3.)]),
            ]
        );
    }

    /// A name this result has no series for matches nothing — which is what lets the choice be
    /// kept across a result that dropped the column and brought it back.
    #[test]
    fn a_name_the_result_has_no_series_for_changes_nothing() {
        let data = table(&["revenue"]);
        assert_eq!(applied(data.clone(), &["margin".to_string()]), data);
        assert_eq!(applied(data.clone(), &[]), data);
        assert!(!all_hidden(&data, &["margin".to_string()]));
    }

    /// Nothing else carries named series, so nothing else is touched.
    #[test]
    fn a_shape_with_no_named_series_is_left_alone() {
        let bins = ChartData::Bins(vec![ChartBin {
            lo: 0.,
            hi: 1.,
            count: 3,
        }]);
        assert_eq!(applied(bins.clone(), &["n".to_string()]), bins);
        assert!(!all_hidden(&bins, &["n".to_string()]));
    }

    /// Every series hidden is a state the canvas has to name — and it is asked of the names,
    /// so a result that is genuinely all NULL is not mistaken for one the user pressed out.
    #[test]
    fn all_hidden_reads_the_names_and_not_the_values() {
        let data = table(&["revenue", "cost"]);
        let hidden = ["revenue".to_string(), "cost".to_string()];
        assert!(all_hidden(&data, &hidden));
        assert!(!all_hidden(&data, &hidden[..1]));

        let all_null = ChartData::Table {
            axis: Axis {
                labels: vec!["a".into()],
                positions: None,
            },
            series: vec![ChartSeries {
                name: "revenue".into(),
                values: vec![None],
            }],
        };
        assert!(!all_hidden(&all_null, &[]));
    }
}
