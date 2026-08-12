//! The Shape panel's **composer** (Chart 09): a [`ShapeForm`] plus the settled run's SQL
//! become one readable `SELECT … FROM (…) AS q GROUP BY … ORDER BY …` string.
//!
//! **The aggregate vocabulary is UI-local and renders to SQL text** — it must not enter
//! strata-model, `ChartQuery`, or any engine type, because an engine-side aggregation
//! pipeline was built, adversarially reviewed and withdrawn (`docs/reference/INVARIANTS.md`,
//! the chart entry). What this module produces is a *query the user owns*: opened unrun in a
//! new tab, editable, and never executed on the user's behalf.
//!
//! Pure functions over strings and picks — no Freya types, so the golden tests read as SQL.

use strata_core::engine::export::quote_col;
use strata_model::ChartRole;

/// The aggregate a measure row offers — rendered to DataFusion's own function names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SqlAgg {
    Sum,
    Avg,
    Min,
    Max,
    Count,
    Median,
}

impl SqlAgg {
    /// Every aggregate, in the order the row's picker offers them.
    pub const ALL: [SqlAgg; 6] = [
        SqlAgg::Sum,
        SqlAgg::Avg,
        SqlAgg::Min,
        SqlAgg::Max,
        SqlAgg::Count,
        SqlAgg::Median,
    ];

    /// The SQL function this renders to — also how the pick reads in the menu.
    pub fn func(self) -> &'static str {
        match self {
            SqlAgg::Sum => "sum",
            SqlAgg::Avg => "avg",
            SqlAgg::Min => "min",
            SqlAgg::Max => "max",
            SqlAgg::Count => "count",
            SqlAgg::Median => "median",
        }
    }
}

/// A time column's bucket width — rendered to a `date_bin` interval.
///
/// Month and year are real strides: DataFusion 54's `date_bin` takes a months-typed interval
/// through its own calendar arm (verified against the pinned sources, `date_bin.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stride {
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl Stride {
    /// Every stride, widest-offer order for an [`Instant`](ChartRole::Instant) column.
    pub const ALL: [Stride; 6] = [
        Stride::Minute,
        Stride::Hour,
        Stride::Day,
        Stride::Week,
        Stride::Month,
        Stride::Year,
    ];

    /// The strides a [`Clock`](ChartRole::Clock) column may take: sub-day only — DataFusion
    /// refuses a day-or-wider `date_bin` over a time of day outright (the fact the
    /// `Instant`/`Clock` split was kept for, finally read).
    pub const SUB_DAY: [Stride; 2] = [Stride::Minute, Stride::Hour];

    /// The `INTERVAL` literal this stride renders to.
    pub fn interval(self) -> &'static str {
        match self {
            Stride::Minute => "1 minute",
            Stride::Hour => "1 hour",
            Stride::Day => "1 day",
            Stride::Week => "1 week",
            Stride::Month => "1 month",
            Stride::Year => "1 year",
        }
    }

    /// How the stride reads in the picker.
    pub fn label(self) -> &'static str {
        match self {
            Stride::Minute => "By minute",
            Stride::Hour => "By hour",
            Stride::Day => "By day",
            Stride::Week => "By week",
            Stride::Month => "By month",
            Stride::Year => "By year",
        }
    }
}

/// How one category column groups: not at all, by its exact value, or binned to a stride
/// (time columns only — the form never offers a stride to a dimension).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GroupBy {
    Off,
    Exact,
    Binned(Stride),
}

/// One groupable column of the settled result and how the user left it.
#[derive(Clone, PartialEq, Debug)]
pub struct GroupPick {
    pub column: String,
    /// The column's chart role, carried so the composer knows whether a stride needs the
    /// timestamp cast (`Instant`) or must not take one (`Clock`).
    pub role: ChartRole,
    pub by: GroupBy,
}

/// One measure column of the settled result and the aggregate it takes — `None` skips it.
#[derive(Clone, PartialEq, Debug)]
pub struct MeasurePick {
    pub column: String,
    pub agg: Option<SqlAgg>,
}

/// The composed query's own `ORDER BY` — always emitted, because a `GROUP BY` has no output
/// order (the workstream's standing lesson).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShapeOrder {
    /// By the group columns, ascending — the calendar reading.
    ByGroup,
    /// By the first aggregated measure, descending — the ranking reading.
    ByMeasureDesc,
}

/// What the panel holds: every groupable column, every measure, the row-count toggle and
/// the order.
#[derive(Clone, PartialEq, Debug)]
pub struct ShapeForm {
    pub groups: Vec<GroupPick>,
    pub measures: Vec<MeasurePick>,
    /// The standalone `count(*)` toggle — a fact about rows, not about any one column.
    pub count_rows: bool,
    pub order: ShapeOrder,
}

impl ShapeForm {
    /// Whether the form composes anything at all — the confirm's enabled state. A form with
    /// no group, no aggregate and no count would render `SELECT FROM`, which is not a query.
    pub fn has_output(&self) -> bool {
        self.groups.iter().any(|g| g.by != GroupBy::Off)
            || self.measures.iter().any(|m| m.agg.is_some())
            || self.count_rows
    }
}

/// Compose the form over the run's SQL, or `None` for a form with nothing picked.
///
/// **Subquery form, not a CTE** — `FROM (…) AS q` — so a run whose SQL already opens with
/// `WITH` nests instead of colliding. The inner SQL sits on its own lines, which is also
/// what keeps a trailing line comment from swallowing the closing paren. A settled rows
/// result is one statement by construction (`sql::validate` refuses a multi-statement Run
/// outright), so the only terminator to shed is a trailing semicolon.
///
/// **Ordinal `GROUP BY`**, so a `date_bin` expression is stated once; idents through
/// [`quote_col`], which quotes a result column exactly as the user's query produced it.
pub fn compose(form: &ShapeForm, sql: &str) -> Option<String> {
    if !form.has_output() {
        return None;
    }
    let inner = sql.trim();
    let inner = inner.strip_suffix(';').map_or(inner, str::trim_end);

    let mut select: Vec<String> = Vec::new();
    let mut group_count = 0usize;
    for group in &form.groups {
        let col = quote_col(&group.column);
        match group.by {
            GroupBy::Off => {}
            GroupBy::Exact => {
                select.push(col);
                group_count += 1;
            }
            GroupBy::Binned(stride) => {
                // `date_bin` takes a timestamp: `Date32` coerces but `Date64` does not
                // (measured, Chart 04), so an instant is cast — a no-op where it already is
                // one. A clock is handed over as itself: casting a time of day to a
                // timestamp is the operation DataFusion refuses, and its sub-day strides
                // are valid directly.
                let value = if group.role == ChartRole::Clock {
                    col.clone()
                } else {
                    format!("CAST({col} AS TIMESTAMP)")
                };
                select.push(format!(
                    "date_bin(INTERVAL '{}', {value}) AS {col}",
                    stride.interval()
                ));
                group_count += 1;
            }
        }
    }

    let mut measure_count = 0usize;
    for measure in &form.measures {
        if let Some(agg) = measure.agg {
            select.push(format!(
                "{}({}) AS {}",
                agg.func(),
                quote_col(&measure.column),
                quote_col(format!("{}_{}", measure.column, agg.func()))
            ));
            measure_count += 1;
        }
    }
    if form.count_rows {
        select.push(format!("count(*) AS {}", quote_col("rows")));
        measure_count += 1;
    }

    let list = select.join(",\n    ");
    let mut out = format!("SELECT\n    {list}\nFROM (\n{inner}\n) AS q");
    if group_count > 0 {
        out.push_str(&format!("\nGROUP BY {}", ordinals(1, group_count)));
    }
    // Always emitted: a `GROUP BY` has no output order. Each arm falls back to the columns
    // the form actually produced, so the clause always names a real ordinal.
    let order = match form.order {
        ShapeOrder::ByGroup if group_count > 0 => ordinals(1, group_count),
        ShapeOrder::ByMeasureDesc if measure_count > 0 => format!("{} DESC", group_count + 1),
        _ => "1".to_string(),
    };
    out.push_str(&format!("\nORDER BY {order}"));
    Some(out)
}

/// `from..from + n` as the comma list an ordinal clause takes.
fn ordinals(from: usize, n: usize) -> String {
    (from..from + n)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(column: &str, role: ChartRole, by: GroupBy) -> GroupPick {
        GroupPick {
            column: column.into(),
            role,
            by,
        }
    }

    fn measure(column: &str, agg: Option<SqlAgg>) -> MeasurePick {
        MeasurePick {
            column: column.into(),
            agg,
        }
    }

    /// The whole shape, golden: quoted idents, the stride's cast, measure aliases, ordinal
    /// GROUP BY, and the always-emitted ORDER BY.
    #[test]
    fn a_full_form_composes_the_golden_query() {
        let form = ShapeForm {
            groups: vec![
                group("day", ChartRole::Instant, GroupBy::Binned(Stride::Month)),
                group("Region", ChartRole::Dimension, GroupBy::Exact),
                group("ignored", ChartRole::Dimension, GroupBy::Off),
            ],
            measures: vec![measure("revenue", Some(SqlAgg::Sum)), measure("cost", None)],
            count_rows: true,
            order: ShapeOrder::ByGroup,
        };
        let sql = compose(&form, "SELECT * FROM sales;").expect("has output");
        assert_eq!(
            sql,
            "SELECT\n    \
                 date_bin(INTERVAL '1 month', CAST(\"day\" AS TIMESTAMP)) AS \"day\",\n    \
                 \"Region\",\n    \
                 sum(\"revenue\") AS \"revenue_sum\",\n    \
                 count(*) AS \"rows\"\n\
             FROM (\n\
             SELECT * FROM sales\n\
             ) AS q\n\
             GROUP BY 1, 2\n\
             ORDER BY 1, 2"
        );
    }

    /// By-measure order names the first measure's ordinal, descending — after the groups.
    #[test]
    fn by_measure_orders_the_first_aggregate_descending() {
        let form = ShapeForm {
            groups: vec![group("region", ChartRole::Dimension, GroupBy::Exact)],
            measures: vec![measure("revenue", Some(SqlAgg::Avg))],
            count_rows: false,
            order: ShapeOrder::ByMeasureDesc,
        };
        let sql = compose(&form, "SELECT 1").expect("has output");
        assert!(sql.ends_with("GROUP BY 1\nORDER BY 2 DESC"), "{sql}");
        assert!(sql.contains("avg(\"revenue\") AS \"revenue_avg\""), "{sql}");
    }

    /// A clock column takes its stride raw — casting a time of day to a timestamp is the
    /// operation DataFusion refuses — and an order with nothing to point at falls back to
    /// the first column rather than emitting no ORDER BY at all.
    #[test]
    fn a_clock_stride_takes_no_cast_and_the_order_is_always_emitted() {
        let form = ShapeForm {
            groups: vec![group("at", ChartRole::Clock, GroupBy::Binned(Stride::Hour))],
            measures: vec![],
            count_rows: false,
            order: ShapeOrder::ByMeasureDesc,
        };
        let sql = compose(&form, "SELECT 1").expect("has output");
        assert!(
            sql.contains("date_bin(INTERVAL '1 hour', \"at\") AS \"at\""),
            "{sql}"
        );
        assert!(sql.ends_with("ORDER BY 1"), "{sql}");
    }

    /// Aggregates with no groups are a whole-result summary: no GROUP BY, still ordered.
    #[test]
    fn measures_alone_summarize_without_a_group_by() {
        let form = ShapeForm {
            groups: vec![group("region", ChartRole::Dimension, GroupBy::Off)],
            measures: vec![measure("v", Some(SqlAgg::Max))],
            count_rows: false,
            order: ShapeOrder::ByGroup,
        };
        let sql = compose(&form, "SELECT 1").expect("has output");
        assert!(!sql.contains("GROUP BY"), "{sql}");
        assert!(sql.ends_with("ORDER BY 1"), "{sql}");
    }

    /// The terminator is shed and the inner SQL keeps its own lines, so a trailing line
    /// comment cannot swallow the closing paren.
    #[test]
    fn the_inner_sql_sheds_its_terminator_and_keeps_its_own_lines() {
        let form = ShapeForm {
            groups: vec![],
            measures: vec![],
            count_rows: true,
            order: ShapeOrder::ByGroup,
        };
        let sql = compose(&form, "SELECT 1 AS n -- trailing note\n;").expect("has output");
        assert!(
            sql.contains("FROM (\nSELECT 1 AS n -- trailing note\n) AS q"),
            "{sql}"
        );

        // A quoted name with an embedded quote survives the round trip doubled.
        let quoted = ShapeForm {
            groups: vec![group("a\"b", ChartRole::Dimension, GroupBy::Exact)],
            measures: vec![],
            count_rows: false,
            order: ShapeOrder::ByGroup,
        };
        let sql = compose(&quoted, "SELECT 1").expect("has output");
        assert!(sql.contains("\"a\"\"b\""), "{sql}");
    }

    /// Nothing picked is no query at all — the confirm's enabled state, not a `SELECT FROM`.
    #[test]
    fn a_form_with_nothing_picked_composes_nothing() {
        let form = ShapeForm {
            groups: vec![group("region", ChartRole::Dimension, GroupBy::Off)],
            measures: vec![measure("v", None)],
            count_rows: false,
            order: ShapeOrder::ByGroup,
        };
        assert!(!form.has_output());
        assert_eq!(compose(&form, "SELECT 1"), None);
    }
}
