//! Catalog **profiling** (D4) — the scan-derived facts behind the column inspector.
//!
//! Facts reach the inspector from two places, matched by [`StatKey`] so neither repeats
//! the other. The source's *free* metadata ([`crate::model::ColumnInfo::stats`]) is
//! read from a Parquet footer at registration and costs nothing; this computes what the
//! source didn't say. For CSV, JSON, and **any view**, that's everything — a view has
//! no footer at all, so a scan is the only way it learns more than a column's type.
//!
//! **One full scan per entry, one aggregate, all columns at once.** Distinct counts
//! can't be merged across files, so there is no cheaper form and no partial version —
//! which is exactly why profiling is opt-in rather than automatic. For a view the cost
//! isn't a file scan but its whole query: joins, aggregates and all.
//!
//! Built with the DataFrame API, not generated SQL: internal logic doesn't write SQL,
//! only the user does.
//!
//! Leaf scalars only: a nested column gets its null count and nothing else, and is
//! never descended into. Profiling a struct's elements would mean traversing
//! arbitrarily deep data on a scan we already told the user was expensive.
//!
//! Results cache on the catalog entry ([`crate::project::CatalogTable::profile`] /
//! [`crate::project::CatalogView::profile`]). A table's dies with its row when the
//! engine re-registers it; a view's dies when its SQL is rewritten. ⚠️ A view is also
//! only as fresh as the tables beneath it, and nothing currently propagates that — see
//! the view-dependency task in DEV_TASKS.

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

use strata_model::Kind;
use strata_model::{ColumnInfo, Stat, StatKey};

// The profile *result* type is shared vocabulary — it lives in `strata-model`. This module
// is the scan *logic* (DataFusion aggregate exprs + result decode) that fills it.
pub use strata_model::CatalogProfile;

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
    NonNull { name: String },
    Stat { name: String, key: StatKey },
}

/// The aggregate expressions for one entry's profile, and what each output means.
///
/// Built with the DataFrame API rather than generated SQL: internal logic doesn't write
/// SQL, only the user does. It also sidesteps identifier handling entirely — note
/// `ident`, not `col`: `col` parses its argument (so a column named `a.b` becomes
/// relation `a` column `b`) and lower-cases it (`A` → `a`). Column names come out of
/// the user's files and can be anything at all.
pub fn aggregates(columns: &[ColumnInfo]) -> (Vec<Expr>, Vec<Slot>) {
    let mut exprs = vec![count_all()];
    let mut slots = vec![Slot::Rows];
    for c in columns {
        let e = || ident(c.name.as_str());
        exprs.push(count(e()));
        slots.push(Slot::NonNull {
            name: c.name.clone(),
        });
        // What's worth computing depends entirely on the type — a mean of a timestamp
        // is nonsense, `min`/`max` of a boolean says nothing, and distinct of a boolean
        // can only ever be 1 or 2. Ask each kind what it can answer rather than running
        // the same six aggregates at everything:
        //
        // | Num    | Distinct · Min · Max · Mean · Median            |
        // | Ts     | Distinct · Min · Max — the range is the point   |
        // | Str    | Distinct · Min · Max (lexicographic)            |
        // | Bool   | nothing beyond nulls                            |
        // | nested | nothing beyond nulls                            |
        //
        // Everything gets Nulls, above. `avg()` on a timestamp isn't merely useless —
        // it's a type error that would fail the *entire* aggregate, taking every other
        // column's facts with it.
        let wants = match c.kind {
            Kind::Num => &[StatKey::Distinct, StatKey::Min, StatKey::Max, StatKey::Mean, StatKey::Median][..],
            Kind::Ts | Kind::Str => &[StatKey::Distinct, StatKey::Min, StatKey::Max][..],
            // A boolean's distinct count is 1 or 2 and its min/max are `false`/`true`.
            // Its one real fact would be the share that are true, which needs a FILTER
            // builder — worth adding if anyone misses it, not worth faking.
            Kind::Bool => &[][..],
            // You can't distinct or order a struct, and we never descend into one.
            Kind::Struct | Kind::List | Kind::Map => &[][..],
        };
        for key in wants {
            let expr = match key {
                StatKey::Distinct => count_distinct(e()),
                StatKey::Min => min(e()),
                StatKey::Max => max(e()),
                StatKey::Mean => avg(e()),
                // The `WITHIN GROUP (ORDER BY x)` migration reached into this signature:
                // the value column comes first, as a `Sort`, then the percentile, then
                // centroids. Approximate on purpose — `median()` is exact and would hold
                // the whole column in memory, on a scan already warned about.
                StatKey::Median => approx_percentile_cont(e().sort(true, false), lit(0.5), None),
                StatKey::Nulls => continue, // derived from the non-null count
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
/// Empty on any expression the unparser can't render — no button beats a broken query.
pub fn profile_sql(owner: &str, exprs: &[Expr]) -> String {
    let mut parts = Vec::with_capacity(exprs.len());
    for e in exprs {
        match datafusion::sql::unparser::expr_to_sql(e) {
            Ok(ast) => parts.push(format!("  {ast}")),
            Err(_) => return String::new(),
        }
    }
    format!(
        "SELECT\n{}\nFROM {};",
        parts.join(",\n"),
        quote_ident(owner)
    )
}

/// Quote an identifier for the generated SQL. Catalog names come from the user, so this
/// is the one place we hand-write an identifier rather than let `ident`/the unparser do
/// it — double any embedded quote.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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
                // Computed, not read from a truncatable footer — always the value.
                exact: true,
            }),
        }
    }

    let mut cols = BTreeMap::new();
    for c in columns {
        // Nulls lead: they're the one fact every column has, nested included.
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
        // The caller fills this from the plan it ran — decode only sees results.
        sql: String::new(),
        cols,
    })
}
