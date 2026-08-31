//! The Shape panel's **composer** (Chart 09): a [`ShapeForm`] plus the settled run's SQL
//! become one readable `SELECT … FROM (…) AS q GROUP BY … ORDER BY …` string.
//!
//! **The aggregate vocabulary is UI-local and renders to SQL text** — it must not enter
//! strata-model, `ChartQuery`, or any engine type, because an engine-side aggregation
//! pipeline was built, adversarially reviewed and withdrawn. What this module
//! produces is a *query the user owns*: opened unrun in a new tab, editable, and never
//! executed on the user's behalf.
//!
//! Pure functions over strings and picks — no Freya types, so the golden tests read as SQL.

use strata_engine::sql::ResultColumn;
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
/// result is one statement by construction (`Workspace::run` refuses a multi-statement Run
/// outright), so the only terminator to shed is a trailing semicolon.
///
/// **Ordinal `GROUP BY`**, so a `date_bin` expression is stated once; idents through
/// [`ResultColumn`], which quotes a result column exactly as the user's query produced it.
pub fn compose(form: &ShapeForm, sql: &str) -> Option<String> {
    if !form.has_output() {
        return None;
    }
    let inner = statement_text(sql);

    let mut select: Vec<String> = Vec::new();
    let mut named: Vec<String> = Vec::new();
    let mut group_count = 0usize;
    for group in &form.groups {
        let col = ResultColumn::of(&group.column).to_string();
        match group.by {
            GroupBy::Off => {}
            GroupBy::Exact => {
                select.push(col);
                named.push(group.column.clone());
                group_count += 1;
            }
            GroupBy::Binned(stride) => {
                let value = if group.role == ChartRole::Clock {
                    col.clone()
                } else {
                    format!("CAST({col} AS TIMESTAMP)")
                };
                select.push(format!(
                    "date_bin(INTERVAL '{}', {value}) AS {col}",
                    stride.interval()
                ));
                named.push(group.column.clone());
                group_count += 1;
            }
        }
    }

    let mut measure_count = 0usize;
    for measure in &form.measures {
        if let Some(agg) = measure.agg {
            let alias = unique(format!("{}_{}", measure.column, agg.func()), &mut named);
            select.push(format!(
                "{}({}) AS {}",
                agg.func(),
                ResultColumn::of(&measure.column),
                ResultColumn::of(alias)
            ));
            measure_count += 1;
        }
    }
    if form.count_rows {
        let alias = unique("rows".to_string(), &mut named);
        select.push(format!("count(*) AS {}", ResultColumn::of(alias)));
        measure_count += 1;
    }

    let list = select.join(",\n    ");
    let mut out = format!("SELECT\n    {list}\nFROM (\n{inner}\n) AS q");
    if group_count > 0 {
        out.push_str(&format!("\nGROUP BY {}", ordinals(group_count)));
    }
    let order = match form.order {
        ShapeOrder::ByGroup if group_count > 0 => ordinals(group_count),
        ShapeOrder::ByMeasureDesc if measure_count > 0 => format!("{} DESC", group_count + 1),
        _ => "1".to_string(),
    };
    out.push_str(&format!("\nORDER BY {order}"));
    Some(out)
}

/// `1..=n` as the comma list an ordinal clause takes.
fn ordinals(n: usize) -> String {
    (1..=n)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `name`, made distinct from every output name already emitted: a collision takes the
/// first free `_2`, `_3`, … suffix. DataFusion refuses a projection with duplicate names
/// (`count(*) AS "rows"` beside a group column named `rows`), and a query this panel hands
/// over has to run.
fn unique(name: String, named: &mut Vec<String>) -> String {
    let mut candidate = name.clone();
    let mut n = 2usize;
    while named.contains(&candidate) {
        candidate = format!("{name}_{n}");
        n += 1;
    }
    named.push(candidate.clone());
    candidate
}

/// The statement's own text: `sql` up to the last character that is not whitespace, a
/// comment or a semicolon.
///
/// The Run's gate proves the buffer holds **one** statement, but what may legally follow it
/// — semicolons and comments in any mix (`SELECT 1; -- note`, `SELECT 1;;`) — survives into
/// the text the panel wraps, and an interior `;` fails the parenthesized subquery. A forward
/// scan that honours string literals, quoted identifiers and both comment forms finds the
/// true end; scanning backwards cannot, because a quote can make a tail read as a comment
/// (`SELECT '-- not a comment'`).
fn statement_text(sql: &str) -> &str {
    let bytes = sql.as_bytes();
    let mut end = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
                end = i;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 1usize;
                i += 2;
                while i < bytes.len() && depth > 0 {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b';' => i += 1,
            c if c.is_ascii_whitespace() => i += 1,
            _ => {
                i += 1;
                end = i;
            }
        }
    }
    sql[..end].trim_start()
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

    /// **The tail after the statement is shed entire** — semicolons and comments in any mix,
    /// which the Run's own parser accepts around a single statement — because an interior
    /// `;` fails the parenthesized subquery. The scan honours strings and quoted
    /// identifiers, so a quote cannot make a tail read as a comment, and everything
    /// *inside* the statement (its own comments included) keeps its lines.
    #[test]
    fn the_tail_after_the_statement_is_shed_and_strings_are_honoured() {
        let count_only = ShapeForm {
            groups: vec![],
            measures: vec![],
            count_rows: true,
            order: ShapeOrder::ByGroup,
        };
        for (sql, inner) in [
            ("SELECT 1 AS n;", "SELECT 1 AS n"),
            ("SELECT 1 AS n; -- checked", "SELECT 1 AS n"),
            ("SELECT 1 AS n;;", "SELECT 1 AS n"),
            ("SELECT 1 AS n -- note\n;", "SELECT 1 AS n"),
            ("SELECT 1 AS n; /* tail\n over lines */", "SELECT 1 AS n"),
            (
                "SELECT 1 AS n -- inline\n+ 2 AS m",
                "SELECT 1 AS n -- inline\n+ 2 AS m",
            ),
            ("SELECT '--; not a comment'", "SELECT '--; not a comment'"),
            ("SELECT \"odd;name\" FROM t;", "SELECT \"odd;name\" FROM t"),
        ] {
            let out = compose(&count_only, sql).expect("has output");
            assert!(
                out.contains(&format!("FROM (\n{inner}\n) AS q")),
                "{sql:?} composed {out}"
            );
        }

        let quoted = ShapeForm {
            groups: vec![group("a\"b", ChartRole::Dimension, GroupBy::Exact)],
            measures: vec![],
            count_rows: false,
            order: ShapeOrder::ByGroup,
        };
        let sql = compose(&quoted, "SELECT 1").expect("has output");
        assert!(sql.contains("\"a\"\"b\""), "{sql}");
    }

    /// **A colliding output name takes the first free suffix** rather than composing a
    /// projection DataFusion refuses — a group column named `rows` beside the row count,
    /// or one named exactly like a measure's alias.
    #[test]
    fn a_colliding_alias_is_suffixed_rather_than_refused_downstream() {
        let form = ShapeForm {
            groups: vec![
                group("rows", ChartRole::Dimension, GroupBy::Exact),
                group("amount_sum", ChartRole::Dimension, GroupBy::Exact),
            ],
            measures: vec![measure("amount", Some(SqlAgg::Sum))],
            count_rows: true,
            order: ShapeOrder::ByGroup,
        };
        let sql = compose(&form, "SELECT 1").expect("has output");
        assert!(sql.contains("sum(\"amount\") AS \"amount_sum_2\""), "{sql}");
        assert!(sql.contains("count(*) AS \"rows_2\""), "{sql}");
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
