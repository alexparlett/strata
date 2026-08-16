//! Catalog **profiling** (D4) — the scan-derived facts behind the column inspector.
//!
//! Facts reach the inspector from two places, matched by [`StatKey`] so neither repeats the other.
//! The source's *free* metadata is read from a Parquet footer at registration and costs nothing;
//! this computes what the source did not say — for CSV, JSON and **any view**, that is everything.
//!
//! **One full scan per entry, one aggregate, all columns at once.** Distinct counts cannot be
//! merged across files, so there is no cheaper form and no partial version, which is why profiling
//! is opt-in. For a view the cost is its whole query, joins and aggregates included.
//!
//! Built with the `DataFrame` API, not generated SQL: internal logic does not write SQL. Leaf
//! scalars only — a nested column gets its null count and is never descended into.
//!
//! **A relation inside a database connection's catalog is profiled on the same terms, with its own
//! expression set** ([`Profiled`]): the aggregate federates into one statement the server runs, so
//! every expression in it has to be one the unparser renders and `PostgreSQL` has.
//!
//! Results cache on the project store's catalog rows: a table's dies with its row when the engine
//! re-registers it, a view's when its SQL is rewritten. ⚠️ A view is only as fresh as the tables
//! beneath it, and nothing currently propagates that.

use std::collections::BTreeMap;
use std::time::SystemTime;

use datafusion::arrow::array::Array;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::functions_aggregate::count::count_all;
use datafusion::functions_aggregate::expr_fn::{
    approx_percentile_cont, avg, count, count_distinct, max, min,
};
use datafusion::prelude::{ident, lit, Expr};
use datafusion::sql::unparser::expr_to_sql;

use strata_model::{ColumnInfo, Stat, StatKey};

pub use strata_model::CatalogProfile;

/// Where a scan runs, what it computes there and what it therefore leaves out — the half of
/// profiling every surface has to agree with this one about, so it lives in
/// [`strata_arrow::profile`] and is re-exported here at the path callers already name (EA-01).
pub use strata_arrow::profile::{stats_footnote, Profiled};

/// What one output column of the aggregate means.
///
/// The decoder reads results **by position** — there are no aliases to collide and no
/// names to match on.
#[derive(Clone, Debug, PartialEq)]
pub enum Slot {
    Rows,
    /// The column's non-null count. Nulls are derived (`rows - non_null`) rather than
    /// aggregated: `count(col)` already skips nulls, so this is exact and avoids the
    /// `ExprFunctionExt` FILTER builder (and its fallible `build()`) for free.
    NonNull {
        name: String,
    },
    Stat {
        name: String,
        key: StatKey,
    },
}

/// The aggregate expressions for one entry's profile, and what each output means.
///
/// Built with the `DataFrame` API rather than generated SQL: internal logic doesn't write
/// SQL, only the user does. It also sidesteps identifier handling entirely — note
/// `ident`, not `col`: `col` parses its argument (so a column named `a.b` becomes
/// relation `a` column `b`) and lower-cases it (`A` → `a`). Column names come out of
/// the user's files and can be anything at all.
pub fn aggregates(columns: &[ColumnInfo], at: Profiled) -> (Vec<Expr>, Vec<Slot>) {
    let mut exprs = vec![count_all()];
    let mut slots = vec![Slot::Rows];
    for c in columns {
        let e = || ident(c.name.as_str());
        exprs.push(count(e()));
        slots.push(Slot::NonNull {
            name: c.name.clone(),
        });
        for key in at.wanted(c.kind) {
            let expr = match key {
                StatKey::Distinct => count_distinct(e()),
                StatKey::Min => min(e()),
                StatKey::Max => max(e()),
                StatKey::Mean => avg(e()),
                StatKey::Median => approx_percentile_cont(e().sort(true, false), lit(0.5), None),
                StatKey::Nulls => continue,
            };
            exprs.push(expr);
            slots.push(Slot::Stat {
                name: c.name.clone(),
                key: *key,
            });
        }
    }
    (exprs, slots)
}

/// Render the profile as SQL the user can read and re-run — the "view as query" button.
///
/// The SELECT list is unparsed from the very `Expr`s that execute (`expr_to_sql`), so
/// the facts can't drift from the numbers on screen. Only the `FROM` is ours, and
/// deliberately so: `plan_to_sql` on the whole plan names *no* view, because DataFusion
/// inlines a view's definition during planning — by the time there's a plan, the view is
/// gone and its body is spliced in. Handing someone `FROM (SELECT … JOIN …)` when they
/// clicked on `active_users` is technically the plan and practically useless. `FROM
/// active_users` is the same query and the one they can actually work with.
///
/// `from` arrives **already rendered**, by whichever of the engine's two renderers the scanned
/// name's identity calls for — [`quote_ident`](super::quote_ident) for a workspace def's
/// registered identity, [`sql::qualified`](super::sql::qualified) for a server's own spelling.
/// The choice is [`run_profile`](super::catalog::run_profile)'s, made once beside the decision
/// about which expressions may run at all, because both turn on the same fact and the wrong
/// renderer is silently wrong in opposite directions.
///
/// That matters because this SQL is not display-only: "view as query" drops it into a
/// scratch tab for the user to edit and re-run.
///
/// Empty on any expression the unparser can't render — no button beats a broken query.
pub fn profile_sql(from: &str, exprs: &[Expr]) -> String {
    let mut parts = Vec::with_capacity(exprs.len());
    for e in exprs {
        match expr_to_sql(e) {
            Ok(ast) => parts.push(format!("  {ast}")),
            Err(_) => return String::new(),
        }
    }
    format!("SELECT\n{}\nFROM {};", parts.join(",\n"), from)
}

/// Decode the aggregate's single result row into per-column facts.
///
/// `columns` is the entry's schema, giving the decode a stable column order. A null
/// result cell means the scan had nothing to say: that becomes an absent fact, never a
/// blank row.
pub fn decode(
    slots: &[Slot],
    batch: &RecordBatch,
    columns: &[ColumnInfo],
) -> Result<CatalogProfile, String> {
    if batch.num_rows() == 0 {
        return Err("profile returned no rows".into());
    }
    let opts = FormatOptions::default();
    let mut rows = 0u64;
    let mut stats: BTreeMap<String, Vec<Stat>> = BTreeMap::new();
    let mut non_null: BTreeMap<String, u64> = BTreeMap::new();
    for (i, slot) in slots.iter().enumerate() {
        let Some(cell) = batch.columns().get(i) else {
            continue;
        };
        if cell.is_null(0) {
            continue;
        }
        let f = ArrayFormatter::try_new(&**cell, &opts).map_err(|e| e.to_string())?;
        let text = f.value(0).to_string();
        match slot {
            Slot::Rows => rows = text.parse().unwrap_or(0),
            Slot::NonNull { name } => {
                if let Ok(n) = text.parse::<u64>() {
                    non_null.insert(name.clone(), n);
                }
            }
            Slot::Stat { name, key } => stats.entry(name.clone()).or_default().push(Stat {
                key: *key,
                text,
                exact: true,
            }),
        }
    }

    let mut cols = BTreeMap::new();
    for c in columns {
        let mut facts = Vec::new();
        if let Some(n) = non_null.get(&c.name) {
            facts.push(Stat {
                key: StatKey::Nulls,
                text: rows.saturating_sub(*n).to_string(),
                exact: true,
            });
        }
        facts.extend(stats.remove(&c.name).unwrap_or_default());
        if !facts.is_empty() {
            cols.insert(c.name.clone(), facts);
        }
    }
    Ok(CatalogProfile {
        at: SystemTime::now(),
        rows,
        sql: String::new(),
        cols,
    })
}

/// **What the remote expression set is allowed to be** — checked against DataFusion's own
/// `PostgreSQL` dialect rather than against a belief about it.
///
/// This is the half of the claim that needs no server. `postgres_federation.rs` pins the other
/// half — that the aggregate really does federate and the server really does run it — but a
/// container is a thing a working tree may not have, and "which expressions may cross the wire"
/// must not be a question only CI can answer.
#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};
    use datafusion::sql::unparser::dialect::PostgreSqlDialect;
    use datafusion::sql::unparser::Unparser;

    use super::*;
    use crate::column_info;
    use crate::sql::qualified;

    fn col(name: &str, dtype: DataType) -> ColumnInfo {
        column_info(&Field::new(name, dtype, true))
    }

    /// Every aggregate a scan builds, as the `PostgreSQL` dialect writes it — or the refusal.
    ///
    /// `Result`, not a panic, because the two failure modes are the thing under test: an
    /// expression the unparser *cannot render at all* and one it renders into a function
    /// `PostgreSQL` does not have are both fatal to a federated scan, and which one the median is
    /// should be read off the unparser rather than assumed.
    fn rendered(at: Profiled, columns: &[ColumnInfo]) -> Vec<Result<String, String>> {
        let unparser = Unparser::new(&PostgreSqlDialect {});
        aggregates(columns, at)
            .0
            .iter()
            .map(|e| {
                unparser
                    .expr_to_sql(e)
                    .map(|ast| ast.to_string())
                    .map_err(|why| why.to_string())
            })
            .collect()
    }

    /// The renderings that succeeded.
    fn sql(at: Profiled, columns: &[ColumnInfo]) -> Vec<String> {
        rendered(at, columns)
            .into_iter()
            .map(|r| r.unwrap_or_else(|why| panic!("the unparser refused an expression: {why}")))
            .collect()
    }

    /// **The headline.** A remote scan's every expression renders into SQL `PostgreSQL` has, and
    /// the one that does not is absent — not substituted, not approximated, not left to fail on
    /// the server.
    ///
    /// The expected list says what the unparser *emits*, not what one would write by hand:
    /// `count_all()` renders `count(1)` rather than `count(*)`, and `ident` renders a quoted
    /// identifier. Both are plain SQL, and the quoting is what preserves a server's own column
    /// spelling. The first version of this list was written from memory and was wrong about both.
    #[test]
    fn the_remote_set_renders_into_sql_postgresql_runs() {
        let columns = [
            col("total", DataType::Float64),
            col("name", DataType::Utf8),
            col("seen", DataType::Timestamp(TimeUnit::Microsecond, None)),
            col("paid", DataType::Boolean),
        ];

        assert_eq!(
            sql(Profiled::Database, &columns),
            vec![
                "count(1)",
                "count(\"total\")",
                "count(DISTINCT \"total\")",
                "min(\"total\")",
                "max(\"total\")",
                "avg(\"total\")",
                "count(\"name\")",
                "count(DISTINCT \"name\")",
                "count(\"seen\")",
                "count(DISTINCT \"seen\")",
                "min(\"seen\")",
                "max(\"seen\")",
                "count(\"paid\")",
            ],
            "every one of these is standard SQL PostgreSQL has for every type that can reach it"
        );
    }

    /// **The median is the one *numeric* expression the wire cannot carry, and this is what would
    /// happen if it were left in.** Federation is not a fallback: it sweeps the whole aggregate
    /// into one remote statement or none, so an expression `PostgreSQL` does not have does not cost
    /// a fact — it fails the scan of every remote table with a numeric column in it.
    ///
    /// Both fatal shapes are checked, because only the unparser can say which one this is: an
    /// expression it refuses outright never reaches SQL at all, and one it renders into a function
    /// name `PostgreSQL` has no entry for fails on the server. The assertion is that the median is
    /// one of the two and that nothing else in the remote set is either.
    #[test]
    fn the_median_is_the_one_expression_the_wire_cannot_carry() {
        let columns = [col("total", DataType::Float64)];
        let local = rendered(Profiled::Workspace, &columns);
        let remote = rendered(Profiled::Database, &columns);

        let dropped: Vec<&Result<String, String>> =
            local.iter().filter(|e| !remote.contains(e)).collect();
        assert_eq!(
            dropped.len(),
            1,
            "exactly one expression differs: {local:?}"
        );
        let median = dropped[0];
        assert!(
            match median {
                Err(_) => true,
                Ok(rendered) => rendered.contains("approx_percentile_cont"),
            },
            "the median is fatal to a federated scan either by not rendering at all or by \
             rendering a function PostgreSQL does not have — it is neither: {median:?}"
        );

        for expression in &remote {
            let rendered = expression
                .as_ref()
                .unwrap_or_else(|why| panic!("a remote expression must render: {why}"));
            assert!(
                !rendered.contains("approx"),
                "nothing approximate survives into the remote set: {rendered}"
            );
        }
        assert!(
            stats_footnote(Profiled::Database).is_some_and(|note| note.contains("medians")),
            "…and the zone says so where the numbers are"
        );
        assert_eq!(stats_footnote(Profiled::Workspace), None);
    }

    /// **The bug this arm exists to prevent, from the type side.** `Kind::Str` is not "a text
    /// column": DB-02 maps every type the connector cannot represent to `Utf8`, so a `jsonb`
    /// column is indistinguishable from a `text` one by the time a profile is built — and
    /// `min(<jsonb>)` is a function `PostgreSQL` does not have, which takes the whole relation's
    /// scan with it. The first version of the remote set emitted exactly that, and the fixture's
    /// `orders.tags JSONB` would have failed on the integration test's first run.
    #[test]
    fn a_remote_string_column_is_counted_but_never_ordered() {
        let columns = [col("tags", DataType::Utf8)];

        assert_eq!(
            sql(Profiled::Database, &columns),
            vec!["count(1)", "count(\"tags\")", "count(DISTINCT \"tags\")"],
            "a distinct count needs only equality, which jsonb has; min/max need an aggregate it \
             does not"
        );
        assert_eq!(
            sql(Profiled::Workspace, &columns).len(),
            5,
            "…and the workspace set is untouched: DataFusion orders its own Utf8 perfectly well"
        );
        assert!(
            stats_footnote(Profiled::Database)
                .is_some_and(|note| note.contains("medians") && note.contains("text columns")),
            "the zone states both omissions"
        );
    }

    /// A remote scan's `FROM` is the server's own spelling, segment by segment — the renderer
    /// choice `run_profile` makes, seen from the SQL it produces. Rendering the three parts as one
    /// name would print `FROM "pg.public.Orders"`, a bare relation with dots in it.
    #[test]
    fn a_remote_profile_renders_a_from_the_server_resolves() {
        let (exprs, _) = aggregates(&[col("id", DataType::Int64)], Profiled::Database);
        let sql = profile_sql(&qualified(["pg", "public", "Orders"]), &exprs);

        assert!(
            sql.contains("FROM pg.public.\"Orders\";"),
            "each segment quoted on its own account: {sql}"
        );
    }
}
