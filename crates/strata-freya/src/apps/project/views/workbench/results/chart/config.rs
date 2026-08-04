//! **What the chart is actually drawing**: the result's column roles, what each encoder may
//! offer over them, and the one place a [`ChartConfig`] plus a schema become a [`ChartQuery`].
//!
//! Three things live here and they are deliberately the same three the strip is built from:
//!
//! - [`Roles`] — the result's columns split by [`ChartRole`], in **result order**, which is
//!   what makes "the first temporal column" something a user can predict from their own
//!   SELECT list.
//! - the per-mark **option sets** ([`x_options`], [`y_options`], [`series_options`]) — spec §4's
//!   table, as functions. The strip builds its menus from them and the resolution below
//!   validates against them, so an encoding the mark cannot take is not reported, it is
//!   *unreachable*: no control ever offers the column.
//! - [`resolve`] + [`encode`] — intent + schema → the read. `resolve` merges the schema's
//!   defaults **under** the user's own choices (spec §6) and drops any reference the current
//!   result cannot answer; `encode` is the single `ChartQuery` construction site, so cache
//!   identity is built in exactly one place.
//!
//! Dropping a stale reference is a *read-time* fallback, never a write back into the config:
//! a column that vanishes from one result and returns in the next must bring the user's
//! choice back with it, and a config the surface silently rewrote could not do that.

use strata_model::{ChartConfig, ChartMark, ChartQuery, ChartRole, ChartSort, ChartX, ColumnInfo};

/// How many result rows the renderer-first read will draw before it refuses (spec §7).
pub const ROWS_CAP: usize = 1_000;
/// A pie's own cap — slices, not rows.
pub const PIE_CAP: usize = 24;
/// Raw points a scatter will draw before it refuses.
pub const RAW_CAP: usize = 6_000;
/// How many measures an **unchosen** Y charts at once. Every measure would make a wide
/// result unreadable; the encoder strip is how you ask for more.
const DEFAULT_YS: usize = 4;

// ---- column roles (spec §3) ----

/// A result's columns, each with the role its Arrow type resolved to, in result order.
///
/// The role is read off the column, not derived here: it was resolved where the engine still
/// had the `DataType` (`engine::catalog::chart_role`), so the encoder and the read agree on
/// what a measure is by construction. [`ChartRole::Other`] never enters — a nested column has
/// no axis to sit on and no value to plot, so it is offered nowhere.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Roles {
    columns: Vec<(String, ChartRole)>,
}

impl Roles {
    pub fn of(columns: &[ColumnInfo]) -> Self {
        Self {
            columns: columns
                .iter()
                .filter(|c| c.role != ChartRole::Other)
                .map(|c| (c.name.clone(), c.role))
                .collect(),
        }
    }

    /// Every column of one role, in result order.
    fn with_role(&self, role: ChartRole) -> Vec<String> {
        self.columns
            .iter()
            .filter(|(_, r)| *r == role)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The numeric columns — Ys, both scatter axes, a histogram's value.
    pub fn measures(&self) -> Vec<String> {
        self.with_role(ChartRole::Measure)
    }

    /// Everything a category axis or a series split can take: temporal and dimension
    /// columns, interleaved in result order.
    pub fn categories(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|(_, r)| *r != ChartRole::Measure)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Every chartable column, in result order.
    pub fn all(&self) -> Vec<String> {
        self.columns.iter().map(|(name, _)| name.clone()).collect()
    }

    /// The default category axis (spec §6): the first time-like column, else the first
    /// dimension, else none — which charts against the row index.
    fn x(&self) -> Option<String> {
        self.columns
            .iter()
            .find(|(_, role)| is_time(*role))
            .or_else(|| {
                self.columns
                    .iter()
                    .find(|(_, role)| *role == ChartRole::Dimension)
            })
            .map(|(name, _)| name.clone())
    }

    /// The role this result gives `name`, or `None` for a column it does not have.
    fn role(&self, name: &str) -> Option<ChartRole> {
        self.columns
            .iter()
            .find(|(column, _)| column == name)
            .map(|(_, role)| *role)
    }
}

/// Whether this role puts a column on a **time** axis — an instant or a clock time.
///
/// On an axis the two are one thing, which is all this surface reads: the same default X, the
/// same default mark, offered in the same menus. They are separate roles because they differ
/// in **SQL**, where a day-wide `date_bin` stride is meaningful over a calendar instant and
/// refused outright over a time of day — the distinction chart-side bucketing will need, kept
/// where the Arrow `DataType` still is rather than re-derived later from a type's spelling
/// (`ChartRole::Instant`).
fn is_time(role: ChartRole) -> bool {
    matches!(role, ChartRole::Instant | ChartRole::Clock)
}

/// The default mark (spec §6): a line over a **temporal X**, a bar otherwise.
///
/// It reads the X actually being charted, not the result's column list: a user who put a
/// country on the axis of a result that also happens to carry a date is not asking for a line
/// across an unordered category. Charting against the row index is a bar for the same reason.
fn default_mark(x: Option<&str>, roles: &Roles) -> ChartMark {
    match x.and_then(|name| roles.role(name)) {
        Some(role) if is_time(role) => ChartMark::Line,
        _ => ChartMark::Bar,
    }
}

// ---- what each mark will take (spec §4) ----

/// Which columns this mark's **X** will take. Empty means the mark has no category axis at
/// all (a histogram bins one column and puts the counts on Y), so the strip shows no X row.
pub fn x_options(mark: ChartMark, roles: &Roles) -> Vec<String> {
    match mark {
        // Both scatter axes are measures — it plots one against another.
        ChartMark::Scatter => roles.measures(),
        // A pie slices a category. `categories` rather than the dimensions alone: it is the
        // app's one answer to what can carry a category axis, and a month is as sliceable as
        // a country.
        ChartMark::Pie => roles.categories(),
        ChartMark::Histogram => Vec::new(),
        // Numeric columns are valid on X too (spec §3).
        ChartMark::Bar | ChartMark::Line | ChartMark::Area => roles.all(),
    }
}

/// Whether this mark can chart against the **row index** — the "X: none" of spec §4. A pie of
/// the row number is a slice per row, and a scatter has no axis without a measure on it.
pub fn allows_row_index(mark: ChartMark) -> bool {
    matches!(mark, ChartMark::Bar | ChartMark::Line | ChartMark::Area)
}

/// Which columns this mark's **Y** will take: the measures, always — the same predicate the
/// engine's read gates a Y on.
pub fn y_options(roles: &Roles) -> Vec<String> {
    roles.measures()
}

/// Whether this mark draws **several** Ys as several series. Pie, scatter and histogram each
/// take exactly one (spec §4), so their Y picker replaces rather than accumulates.
pub fn takes_many_ys(mark: ChartMark) -> bool {
    matches!(mark, ChartMark::Bar | ChartMark::Line | ChartMark::Area)
}

/// Which columns this mark's **series** will take, given the X it already has. Empty means no
/// series row at all: only bar / line / area split, the pivot needs an X to pivot *around*,
/// and one column cannot be both the category and the split (all three are the engine's own
/// refusals — this is what keeps them unreachable).
pub fn series_options(mark: ChartMark, roles: &Roles, x: Option<&str>) -> Vec<String> {
    match (mark, x) {
        (ChartMark::Bar | ChartMark::Line | ChartMark::Area, Some(x)) => roles
            .categories()
            .into_iter()
            .filter(|name| name != x)
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether this mark's data has an order to sort — [`ChartData::Table`]'s categories. A
/// scatter draws unordered marks and a histogram's bins are ascending by construction, so
/// neither has anything the sort could mean.
///
/// [`ChartData::Table`]: strata_model::ChartData::Table
pub fn sortable(mark: ChartMark) -> bool {
    !matches!(mark, ChartMark::Scatter | ChartMark::Histogram)
}

// ---- intent + schema → the read ----

/// What the chart is drawing: the config resolved against the result actually in hand. Every
/// column here exists in that result, which is the whole point — nothing downstream has to
/// re-check.
#[derive(Clone, PartialEq, Debug)]
pub struct Encoding {
    pub mark: ChartMark,
    /// `None` charts against the row index.
    pub x: Option<String>,
    pub ys: Vec<String>,
    pub series: Option<String>,
    pub sort: ChartSort,
}

/// Merge the schema's defaults **under** the user's own choices (spec §6).
///
/// Each channel resolves the same way: take the choice if the result can still answer it,
/// otherwise derive. That single rule covers all of it — an untouched config (everything
/// derived), a tab restored from disk against a query whose SELECT list has since changed, a
/// mark switched to one whose axes the old choice cannot sit on, and a re-run whose result
/// simply dropped a column.
pub fn resolve(config: &ChartConfig, roles: &Roles) -> Encoding {
    // **X first, then the mark it implies.** The default mark is a fact about the *charted
    // axis* (spec §6), so it cannot be read before X is known — and X cannot wait for it,
    // because a mark decides which columns its axis will take. The cycle is only apparent:
    // an unset mark resolves to a bar or a line, and those two offer exactly the same X
    // ([`x_options`], [`allows_row_index`], [`default_x`]), so resolving the axis against
    // either gives the answer both would.
    let probe = config.mark.unwrap_or(ChartMark::Bar);
    let offered = x_options(probe, roles);
    let x = match &config.x {
        ChartX::Column(name) if offered.contains(name) => Some(name.clone()),
        ChartX::RowIndex if allows_row_index(probe) => None,
        // `Auto`, a column this mark's axis cannot take, and a column the result no longer
        // has are one case: nothing the user chose applies, so the default does.
        _ => default_x(probe, roles),
    };
    let mark = config
        .mark
        .unwrap_or_else(|| default_mark(x.as_deref(), roles));

    let measures = roles.measures();
    let ys = match &config.ys {
        // Deliberately nothing — the user unpicked them all, which is a real state and says
        // so on the canvas rather than quietly re-deriving under them.
        Some(chosen) if chosen.is_empty() => Vec::new(),
        Some(chosen) => {
            // Kept in **result order**, not pick order: a series' colour comes from its
            // position, and a legend that reshuffles on every tick is unreadable.
            let kept: Vec<String> = measures
                .iter()
                .filter(|name| chosen.contains(name))
                .cloned()
                .collect();
            if kept.is_empty() {
                default_ys(mark, &measures, x.as_deref())
            } else {
                kept
            }
        }
        None => default_ys(mark, &measures, x.as_deref()),
    };
    // A mark that draws one Y keeps the first of the choice rather than dropping it: flipping
    // to a pie and back must not cost the other Ys, so the *config* still holds them all.
    let ys = if takes_many_ys(mark) {
        ys
    } else {
        ys.into_iter().take(1).collect()
    };

    let series = config
        .series
        .clone()
        .filter(|name| series_options(mark, roles, x.as_deref()).contains(name));

    Encoding {
        mark,
        x,
        ys,
        series,
        sort: config.sort,
    }
}

/// The default category axis for a mark (spec §6).
fn default_x(mark: ChartMark, roles: &Roles) -> Option<String> {
    match mark {
        ChartMark::Histogram => None,
        ChartMark::Scatter => roles.measures().first().cloned(),
        ChartMark::Bar | ChartMark::Line | ChartMark::Area | ChartMark::Pie => roles.x(),
    }
}

/// The default value columns: the leading measures, minus whatever X already took — charting
/// a column against itself is never what an untouched config should mean, and for a scatter
/// it is the difference between two axes and one diagonal.
fn default_ys(mark: ChartMark, measures: &[String], x: Option<&str>) -> Vec<String> {
    let take = if takes_many_ys(mark) { DEFAULT_YS } else { 1 };
    measures
        .iter()
        .filter(|name| Some(name.as_str()) != x)
        .take(take)
        .cloned()
        .collect()
}

// ---- the read ----

/// The read this encoding asks for, or why the columns cannot answer it. The message is the
/// whole answer at this stage — Chart 04 adds the scaffold CTA beneath it.
///
/// **The one `ChartQuery` construction site.** It is freya-query cache identity, so a second
/// place building one would fork the entry into a duplicate read.
pub fn encode(
    encoding: &Encoding,
    roles: &Roles,
) -> Result<ChartQuery, (&'static str, &'static str)> {
    match encoding.mark {
        ChartMark::Scatter => match (&encoding.x, encoding.ys.first()) {
            (Some(x), Some(y)) => Ok(ChartQuery::Raw {
                x: x.clone(),
                y: y.clone(),
                cap: RAW_CAP,
            }),
            _ if roles.measures().len() < 2 => Err((
                "Pick two numeric columns",
                "A scatter plots one measure against another, and the result has fewer than two.",
            )),
            _ => Err(no_y(roles)),
        },
        ChartMark::Histogram => match encoding.ys.first() {
            Some(col) => Ok(ChartQuery::Histogram {
                col: col.clone(),
                bins: None,
            }),
            None => Err(no_y(roles)),
        },
        ChartMark::Pie => match (&encoding.x, encoding.ys.first()) {
            (Some(x), Some(y)) => Ok(ChartQuery::Rows {
                x: Some(x.clone()),
                ys: vec![y.clone()],
                series: None,
                cap: PIE_CAP,
            }),
            (None, _) => Err((
                "Pick a category column",
                "A pie slices one measure by a category, and the result has no column to slice by.",
            )),
            (_, None) => Err(no_y(roles)),
        },
        ChartMark::Bar | ChartMark::Line | ChartMark::Area => {
            if encoding.ys.is_empty() {
                return Err(no_y(roles));
            }
            Ok(ChartQuery::Rows {
                x: encoding.x.clone(),
                ys: encoding.ys.clone(),
                series: encoding.series.clone(),
                cap: ROWS_CAP,
            })
        }
    }
}

/// Why there is no Y — two different problems that would read as one message. The result
/// having no numeric column at all is a fact about the data; an empty pick is a choice the
/// user just made, and telling them their result has nothing numeric in it while its measures
/// sit in the menu they emptied would be a lie.
fn no_y(roles: &Roles) -> (&'static str, &'static str) {
    if roles.measures().is_empty() {
        (
            "Pick a numeric column",
            "This mark plots a measure, and the result has no numeric column to plot.",
        )
    } else {
        (
            "Pick a column to plot",
            "No column is chosen on the Y axis.",
        )
    }
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
    use strata_core::engine::column_info;

    use super::*;

    /// A result column built the way the engine builds one — from a real Arrow field, through
    /// the same `column_info` a Run's schema goes through. Nothing here restates the role
    /// mapping; the point of these cases is what the *encoder* does with it.
    fn column(name: &str, dtype: DataType) -> ColumnInfo {
        column_info(&Field::new(name, dtype, true))
    }

    /// The shape most of these cases resolve against: a date, a category and two measures.
    fn sales() -> Roles {
        Roles::of(&[
            column("month", DataType::Date32),
            column("country", DataType::Utf8),
            column("revenue", DataType::Int64),
            column("cost", DataType::Float64),
        ])
    }

    fn read(
        config: &ChartConfig,
        roles: &Roles,
    ) -> Result<ChartQuery, (&'static str, &'static str)> {
        encode(&resolve(config, roles), roles)
    }

    #[test]
    fn an_untouched_config_charts_a_line_over_every_measure() {
        let roles = sales();
        let resolved = resolve(&ChartConfig::default(), &roles);
        assert_eq!(resolved.mark, ChartMark::Line);
        assert_eq!(resolved.x.as_deref(), Some("month"));
        assert_eq!(
            read(&ChartConfig::default(), &roles),
            Ok(ChartQuery::Rows {
                x: Some("month".into()),
                ys: vec!["revenue".into(), "cost".into()],
                series: None,
                cap: ROWS_CAP,
            })
        );
    }

    /// A nested column has no axis to sit on and no value to plot, so it is invisible to the
    /// encoder — which is the one place the chart's taxonomy has to be finer than the display
    /// one (a union renders as a string and cannot be charted as one).
    #[test]
    fn nested_columns_are_offered_nowhere() {
        let roles = Roles::of(&[
            column(
                "payload",
                DataType::Struct(vec![Field::new("a", DataType::Int64, true)].into()),
            ),
            column(
                "tags",
                DataType::List(Field::new("item", DataType::Utf8, true).into()),
            ),
            column("elapsed", DataType::Duration(TimeUnit::Second)),
            column("n", DataType::Int64),
        ]);
        assert_eq!(roles.all(), ["n"]);
        assert_eq!(roles.measures(), ["n"]);
        assert!(roles.categories().is_empty());
    }

    #[test]
    fn without_a_temporal_column_the_first_dimension_is_the_axis_and_the_mark_is_a_bar() {
        let roles = Roles::of(&[
            column("country", DataType::Utf8),
            column("revenue", DataType::Int64),
        ]);
        assert_eq!(
            read(&ChartConfig::default(), &roles),
            Ok(ChartQuery::Rows {
                x: Some("country".into()),
                ys: vec!["revenue".into()],
                series: None,
                cap: ROWS_CAP,
            })
        );
        // A pie is the same columns under a much smaller cap (spec §7).
        let pie = ChartConfig {
            mark: Some(ChartMark::Pie),
            ..ChartConfig::default()
        };
        assert_eq!(
            read(&pie, &roles),
            Ok(ChartQuery::Rows {
                x: Some("country".into()),
                ys: vec!["revenue".into()],
                series: None,
                cap: PIE_CAP,
            })
        );
    }

    /// With no column to put on the axis the read still happens — against the row index,
    /// which is what "X: none" means (spec §4).
    #[test]
    fn measures_alone_chart_against_the_row_index() {
        let roles = Roles::of(&[column("n", DataType::Int64)]);
        assert_eq!(
            read(&ChartConfig::default(), &roles),
            Ok(ChartQuery::Rows {
                x: None,
                ys: vec!["n".into()],
                series: None,
                cap: ROWS_CAP,
            })
        );
    }

    #[test]
    fn the_marks_that_need_measures_say_so_rather_than_reading_nothing() {
        let text = Roles::of(&[column("country", DataType::Utf8)]);
        for mark in [ChartMark::Bar, ChartMark::Histogram] {
            let config = ChartConfig {
                mark: Some(mark),
                ..ChartConfig::default()
            };
            assert_eq!(read(&config, &text).unwrap_err().0, "Pick a numeric column");
        }

        // A scatter needs two, and neither none nor one is two — it says what it needs
        // rather than falling back to the one-measure message.
        let one = Roles::of(&[column("n", DataType::Int64)]);
        let scatter = ChartConfig {
            mark: Some(ChartMark::Scatter),
            ..ChartConfig::default()
        };
        for roles in [&text, &one] {
            assert_eq!(
                read(&scatter, roles).unwrap_err().0,
                "Pick two numeric columns"
            );
        }
        let histogram = ChartConfig {
            mark: Some(ChartMark::Histogram),
            ..ChartConfig::default()
        };
        assert_eq!(
            read(&histogram, &one),
            Ok(ChartQuery::Histogram {
                col: "n".into(),
                bins: None,
            })
        );

        // And a pie needs a category to slice by.
        let pie = ChartConfig {
            mark: Some(ChartMark::Pie),
            ..ChartConfig::default()
        };
        assert_eq!(read(&pie, &one).unwrap_err().0, "Pick a category column");

        // Two measures are two scatter axes, and the second is not the first.
        let two = Roles::of(&[column("x", DataType::Int64), column("y", DataType::Float64)]);
        assert_eq!(
            read(&scatter, &two),
            Ok(ChartQuery::Raw {
                x: "x".into(),
                y: "y".into(),
                cap: RAW_CAP,
            })
        );
    }

    /// **The user's choice outranks the default, and survives a mark that cannot take it.**
    /// Switching to a pie narrows four Ys to one and drops the series — but only in the
    /// *encoding*: the config still holds them, so switching back restores the bar chart the
    /// user built.
    #[test]
    fn a_mark_narrows_the_encoding_without_spending_the_config() {
        let roles = sales();
        let config = ChartConfig {
            mark: Some(ChartMark::Bar),
            x: ChartX::Column("month".into()),
            ys: Some(vec!["revenue".into(), "cost".into()]),
            series: Some("country".into()),
            sort: ChartSort::ResultOrder,
        };
        assert_eq!(
            read(&config, &roles),
            Ok(ChartQuery::Rows {
                x: Some("month".into()),
                ys: vec!["revenue".into(), "cost".into()],
                series: Some("country".into()),
                cap: ROWS_CAP,
            })
        );

        let pie = ChartConfig {
            mark: Some(ChartMark::Pie),
            ..config.clone()
        };
        assert_eq!(
            read(&pie, &roles),
            Ok(ChartQuery::Rows {
                x: Some("month".into()),
                ys: vec!["revenue".into()],
                series: None,
                cap: PIE_CAP,
            })
        );
        // …and the config the pie resolved from is untouched, so the bar comes back whole.
        assert_eq!(read(&config, &roles).unwrap(), {
            ChartQuery::Rows {
                x: Some("month".into()),
                ys: vec!["revenue".into(), "cost".into()],
                series: Some("country".into()),
                cap: ROWS_CAP,
            }
        });
    }

    /// **A stale column name never reaches the read.** A tab restored against a result whose
    /// SELECT list has moved on falls back to the defaults, channel by channel — and keeps
    /// the choices the new result *can* still answer.
    #[test]
    fn a_reference_the_result_cannot_answer_falls_back_to_the_default() {
        let roles = sales();
        let config = ChartConfig {
            mark: Some(ChartMark::Bar),
            x: ChartX::Column("quarter".into()),
            ys: Some(vec!["margin".into(), "cost".into()]),
            series: Some("region".into()),
            sort: ChartSort::ResultOrder,
        };
        let resolved = resolve(&config, &roles);
        assert_eq!(resolved.x.as_deref(), Some("month"), "X falls back");
        assert_eq!(resolved.ys, ["cost"], "the surviving Y is kept, alone");
        assert_eq!(resolved.series, None, "a series that is gone is no series");

        // Every reference dead → the whole channel derives rather than plotting nothing.
        let gone = ChartConfig {
            ys: Some(vec!["margin".into()]),
            ..config
        };
        assert_eq!(resolve(&gone, &roles).ys, ["revenue", "cost"]);
    }

    /// Unpicking every Y is a choice, not a dead reference: it stays empty, and the read says
    /// so in the user's own terms rather than the engine's.
    #[test]
    fn an_emptied_y_stays_empty_and_says_which_kind_of_empty_it_is() {
        let roles = sales();
        let none = ChartConfig {
            ys: Some(Vec::new()),
            ..ChartConfig::default()
        };
        assert!(resolve(&none, &roles).ys.is_empty());
        assert_eq!(read(&none, &roles).unwrap_err().0, "Pick a column to plot");
    }

    /// The row index is a **choice**, so a result that happens to have a date column in it
    /// does not overrule it — the distinction `ChartX` exists to keep.
    #[test]
    fn charting_against_the_row_index_survives_a_temporal_column() {
        let roles = sales();
        let config = ChartConfig {
            x: ChartX::RowIndex,
            ..ChartConfig::default()
        };
        assert_eq!(resolve(&config, &roles).x, None);

        // A mark with no such thing as a row-index axis takes its default instead.
        let pie = ChartConfig {
            mark: Some(ChartMark::Pie),
            ..config
        };
        assert_eq!(resolve(&pie, &roles).x.as_deref(), Some("month"));
    }

    /// **The menus are the constraint.** Each mark offers exactly what spec §4 says it takes,
    /// which is what makes an invalid encoding unreachable rather than reported.
    #[test]
    fn each_mark_offers_only_the_columns_it_can_take() {
        let roles = sales();

        // Bar / line / area: any column on X (numeric included), plus the row index.
        assert_eq!(
            x_options(ChartMark::Bar, &roles),
            ["month", "country", "revenue", "cost"]
        );
        assert!(allows_row_index(ChartMark::Bar));

        // Scatter: measures on both axes, no row index, no series.
        assert_eq!(x_options(ChartMark::Scatter, &roles), ["revenue", "cost"]);
        assert!(!allows_row_index(ChartMark::Scatter));
        assert!(series_options(ChartMark::Scatter, &roles, Some("revenue")).is_empty());

        // Pie: a category on X, one Y, no series.
        assert_eq!(x_options(ChartMark::Pie, &roles), ["month", "country"]);
        assert!(!takes_many_ys(ChartMark::Pie));
        assert!(series_options(ChartMark::Pie, &roles, Some("month")).is_empty());

        // Histogram: no X at all.
        assert!(x_options(ChartMark::Histogram, &roles).is_empty());

        // A series splits on a category, never on the column already carrying the axis, and
        // never without one (the pivot has nothing to pivot around).
        assert_eq!(
            series_options(ChartMark::Bar, &roles, Some("month")),
            ["country"]
        );
        assert!(series_options(ChartMark::Bar, &roles, None).is_empty());

        // Y is the measures under every mark — the read's own gate.
        assert_eq!(y_options(&roles), ["revenue", "cost"]);
    }

    /// **The default mark follows the charted axis, not the result's column list** (spec §6).
    /// The distinction only shows once the user can set X — which is what this task added:
    /// putting a country on the axis of a result that also carries a date must not leave a
    /// line running across an unordered category.
    #[test]
    fn the_default_mark_reads_the_x_it_is_drawing_not_the_schema() {
        let roles = sales();
        assert_eq!(
            resolve(&ChartConfig::default(), &roles).mark,
            ChartMark::Line,
            "the derived X is the date"
        );

        for (x, expected) in [
            (ChartX::Column("country".into()), ChartMark::Bar),
            (ChartX::Column("revenue".into()), ChartMark::Bar),
            (ChartX::RowIndex, ChartMark::Bar),
            (ChartX::Column("month".into()), ChartMark::Line),
        ] {
            let config = ChartConfig {
                x: x.clone(),
                ..ChartConfig::default()
            };
            assert_eq!(resolve(&config, &roles).mark, expected, "{x:?}");
        }

        // A mark the user chose is never re-derived from anything.
        let chosen = ChartConfig {
            mark: Some(ChartMark::Area),
            x: ChartX::Column("country".into()),
            ..ChartConfig::default()
        };
        assert_eq!(resolve(&chosen, &roles).mark, ChartMark::Area);
    }

    /// A measure on X is not also a default Y: an untouched config must never plot a column
    /// against itself, and for a scatter that is the whole difference between two axes and a
    /// diagonal line.
    #[test]
    fn the_column_on_x_is_not_also_a_default_y() {
        let roles = sales();
        let config = ChartConfig {
            mark: Some(ChartMark::Bar),
            x: ChartX::Column("revenue".into()),
            ..ChartConfig::default()
        };
        assert_eq!(resolve(&config, &roles).ys, ["cost"]);
    }
}
