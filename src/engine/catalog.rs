//! Catalog side of the engine: registering external tables, reading their free
//! (footer) statistics, view-dependency extraction (D10), and full-scan profiling (D4).

use std::path::Path;
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field};
use datafusion::prelude::*;

use crate::model::{ColumnInfo, Kind, Stat, StatKey};

use super::{TableMeta, TableSpec};

// ---- external table registration ----

pub async fn register_external(ctx: &SessionContext, spec: &TableSpec) -> Result<TableMeta, String> {
    use datafusion::datasource::file_format::arrow::ArrowFormat;
    use datafusion::datasource::file_format::csv::CsvFormat;
    use datafusion::datasource::file_format::json::JsonFormat;
    use datafusion::datasource::file_format::parquet::ParquetFormat;
    use datafusion::datasource::file_format::FileFormat;
    use datafusion::datasource::listing::{
        ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
    };

    let _ = ctx.deregister_table(spec.name.as_str());

    let mut urls = Vec::new();
    for p in &spec.paths {
        if p.trim().is_empty() {
            continue;
        }
        let mut loc = p.clone();
        if Path::new(&loc).is_dir() && !loc.ends_with('/') {
            loc.push('/');
        }
        urls.push(ListingTableUrl::parse(&loc).map_err(|e| e.to_string())?);
    }
    if urls.is_empty() {
        return Err("No source paths".into());
    }

    let (fmt, ext): (Arc<dyn FileFormat>, &str) = match spec.format.as_str() {
        "csv" => (Arc::new(CsvFormat::default()), ".csv"),
        "json" => (Arc::new(JsonFormat::default()), ".json"),
        "arrow" => (Arc::new(ArrowFormat), ".arrow"),
        _ => (
            Arc::new(ParquetFormat::default().with_skip_metadata(true)),
            ".parquet",
        ),
    };
    // `with_session_config_options` *before* any explicit option: it carries the
    // session's `collect_statistics` (and `target_partitions`) onto the options and
    // would otherwise clobber them.
    //
    // It is not optional. `ListingOptions::new` hardcodes `collect_stat: false`, and a
    // hand-built `ListingTable` never picks the `datafusion.execution.collect_statistics`
    // key up on its own — `ListingTableConfig::with_listing_options` does no such wiring.
    // Without this, every footer statistic comes back `Absent` while the engine setting
    // claims to be on. It's baked in at `try_new`, so a registered table can't be fixed
    // after the fact — `rebuild_listing` inherits it by cloning `lt.options()`.
    let mut opts = ListingOptions::new(fmt)
        .with_session_config_options(&ctx.copied_config())
        .with_file_extension(ext);
    if !spec.partitions.is_empty() {
        let cols = spec
            .partitions
            .iter()
            .map(|(n, ty)| (n.clone(), parse_dtype(ty)))
            .collect();
        opts = opts.with_table_partition_cols(cols);
    }

    let config = ListingTableConfig::new_with_multi_paths(urls)
        .with_listing_options(opts)
        .infer_schema(&ctx.state())
        .await
        .map_err(|e| e.to_string())?;
    let table = ListingTable::try_new(config).map_err(|e| e.to_string())?;
    ctx.register_table(spec.name.as_str(), Arc::new(table))
       .map_err(|e| e.to_string())?;

    table_meta(ctx, spec.name.as_str()).await
}

/// Rebuild a registered `ListingTable` from its own `paths` + `options`, re-inferring
/// the schema, and re-register it under `name` — the schema-refresh step
/// (`RefreshCatalog`). Re-registering the *same* provider wouldn't re-infer, so we
/// construct a fresh table from a re-`infer_schema`d config. Returns its columns + free
/// metadata — `opts` is the live table's own, so `collect_stat` carries over with it.
pub async fn rebuild_listing(
    ctx: &SessionContext,
    name: &str,
    paths: Vec<datafusion::datasource::listing::ListingTableUrl>,
    opts: datafusion::datasource::listing::ListingOptions,
) -> Result<TableMeta, String> {
    use datafusion::datasource::listing::{ListingTable, ListingTableConfig};
    let config = ListingTableConfig::new_with_multi_paths(paths)
        .with_listing_options(opts)
        .infer_schema(&ctx.state())
        .await
        .map_err(|e| e.to_string())?;
    let table = ListingTable::try_new(config).map_err(|e| e.to_string())?;
    let _ = ctx.deregister_table(name);
    ctx.register_table(name, Arc::new(table))
       .map_err(|e| e.to_string())?;
    table_meta(ctx, name).await
}

// ---- schema helpers ----

/// What a view plan reads (D10): its **base tables** and the **names it inlines**.
///
/// Asks the planner, not the SQL text, which matters three ways:
///
/// - **Views are already resolved away.** DataFusion 54 inlines a view at plan-*build*
///   time, so a plan from `ctx.table("a_view")` scans the view's base tables directly.
///   That's transitive for free — a view over a view was inlined when the inner one was
///   planned, so `C → B → A → orders` collapses to a single tree carrying `orders` at
///   the leaf and `A`, `B` as the inliner's alias markers on the way down. Reading the
///   SQL would stop at `FROM b`.
/// - **`apply_with_subqueries`, not `apply`.** Plain `apply` visits only direct
///   children, so a view with `WHERE id IN (SELECT id FROM other)` would silently drop
///   `other` — and a *missed* dependency is the failure that matters: a stale profile
///   nobody invalidates, or an entry dropped without warning.
/// - **`.table()`, not `to_string()`.** A `TableReference` renders as written — `t`
///   here, `public.t` there — so `to_string()` yields two keys for one thing. The engine
///   owns a single schema, so the bare name is the identity.
pub struct PlanDeps {
    /// Base tables scanned — for profile invalidation and the table-drop warning.
    pub tables: Vec<String>,
    /// Every `SubqueryAlias` name, which for an inlined sub-view is the view's own name.
    /// Raw: also includes plain table aliases (`FROM t AS x`) and CTE names, since those
    /// are indistinguishable from a view inline in the plan. The UI keeps only the ones
    /// that are actually views. Recursion is automatic — a chain leaves one alias per
    /// hop in the tree, so this is the transitive set of referenced views.
    pub aliases: Vec<String>,
}

pub fn plan_deps(plan: &datafusion::logical_expr::LogicalPlan) -> PlanDeps {
    use datafusion::common::tree_node::TreeNodeRecursion;
    use datafusion::logical_expr::LogicalPlan;
    let mut tables = std::collections::BTreeSet::new();
    let mut aliases = std::collections::BTreeSet::new();
    let _ = plan.apply_with_subqueries(|node| {
        match node {
            LogicalPlan::TableScan(scan) => {
                // A source still carrying its own plan is a view that *didn't* inline —
                // only reachable if filters were pushed at build time, which our path
                // never does. Recording it would name the view instead of what it reads.
                if scan.source.get_logical_plan().is_none() {
                    tables.insert(scan.table_name.table().to_string());
                }
            }
            LogicalPlan::SubqueryAlias(a) => {
                aliases.insert(a.alias.table().to_string());
            }
            _ => {}
        }
        Ok(TreeNodeRecursion::Continue)
    });
    PlanDeps {
        tables: tables.into_iter().collect(),
        aliases: aliases.into_iter().collect(),
    }
}

/// Profile `name` — one full scan, every column at once (see [`crate::profile`]).
///
/// Runs on this worker like any other command, so the UI stays live and the row's
/// `profiling` flag drives the spinner. Blocking is fine here; it's *meant* to be the
/// expensive thing the user opted into.
pub async fn run_profile(ctx: &SessionContext, name: &str) -> Result<crate::profile::CatalogProfile, String> {
    let df = ctx.table(name).await.map_err(|e| e.to_string())?;
    let columns: Vec<ColumnInfo> = df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect();
    let (exprs, slots) = crate::profile::aggregates(&columns);
    // Render *before* executing, from the same `Expr`s that are about to run, so "view
    // as query" can't drift from the facts it produced. Not `plan_to_sql` on the whole
    // plan: that inlines a view's body and names no view (see `profile_sql`).
    let sql = crate::profile::profile_sql(name, &exprs);
    let batches = df
        .aggregate(vec![], exprs)
        .map_err(|e| e.to_string())?
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    let batch = batches.first().ok_or("profile returned no batches")?;
    let mut profile = crate::profile::decode(&slots, batch, &columns)?;
    profile.sql = sql;
    Ok(profile)
}

/// A table's columns plus its **free** metadata — the row count and per-column
/// min/max/nulls, read from the source's own footers. One metadata read per file, no
/// data pages. Everything lands `None` for a source that reports nothing (CSV/JSON),
/// which the inspector renders as an absent row rather than a guess.
async fn table_meta(ctx: &SessionContext, name: &str) -> Result<TableMeta, String> {
    let df = ctx.table(name).await.map_err(|e| e.to_string())?;
    // `|f| column_info(f)`, not `column_info`: `fields()` yields `&Arc<Field>` and the
    // deref coercion to `&Field` only happens at a call site.
    let mut columns: Vec<ColumnInfo> = df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect();
    let rows = free_stats(ctx, name, &mut columns).await;
    Ok(TableMeta { columns, rows })
}

/// Fold the source's free statistics onto `columns`, returning the row count. Best
/// effort throughout: anything unavailable simply stays `None`.
async fn free_stats(ctx: &SessionContext, name: &str, columns: &mut [ColumnInfo]) -> Option<u64> {
    use datafusion::datasource::listing::ListingTable;
    let provider = ctx.table_provider(name).await.ok()?;
    // Only a `ListingTable` has files whose footers can be read — a view has none.
    let lt = provider.downcast_ref::<ListingTable>()?;
    let state = ctx.state();
    // `limit: None` — a limit would make the aggregate inexact.
    let stats = lt.list_files_for_scan(&state, &[], None).await.ok()?.statistics;
    let rows = stats.num_rows.get_value().map(|n| *n as u64);
    // Zip rather than index: DataFusion promises one entry per *table*-schema field, but
    // a table with no files short-circuits to `file_schema`, which omits the partition
    // columns — indexing would then misattribute every stat.
    for (col, cs) in columns.iter_mut().zip(stats.column_statistics.iter()) {
        // Push only what's actually there — an absent fact is an absent row, not a
        // blank one. Display order.
        let nulls = match cs.null_count.get_value() {
            // `Exact(num_rows)` is *also* DataFusion's "no stats for this column"
            // fallback, so an all-null column and an unknown one are indistinguishable.
            // Say nothing; the profile answers it for real with a COUNT ... FILTER.
            Some(n) if Some(*n as u64) == rows => None,
            Some(n) => Some(Stat {
                key: StatKey::Nulls,
                text: n.to_string(),
                exact: true,
            }),
            None => None,
        };
        col.stats = [nulls, stat_of(StatKey::Min, &cs.min_value), stat_of(StatKey::Max, &cs.max_value)]
            .into_iter()
            .flatten()
            .collect();
    }
    rows
}

/// A `Precision<ScalarValue>` as a display [`Stat`]. `Absent` → `None` (say nothing).
/// A null value means the column is in the arrow schema but absent from the source's
/// own (schema evolution) — also nothing to report. `Inexact` carries through flagged.
fn stat_of(
    key: StatKey,
    p: &datafusion::common::stats::Precision<datafusion::common::ScalarValue>,
) -> Option<Stat> {
    let v = p.get_value()?;
    if v.is_null() {
        return None;
    }
    Some(Stat {
        key,
        text: v.to_string(),
        exact: p.is_exact().unwrap_or(false),
    })
}

pub fn column_info(field: &Field) -> ColumnInfo {
    let dtype = short_type(field.data_type());
    ColumnInfo {
        name: field.name().clone(),
        kind: Kind::from_arrow(&dtype),
        dtype,
        nullable: field.is_nullable(),
        children: nested_children(field.data_type()),
        // Filled by `free_stats` where the source has metadata to read; a nested child
        // never gets any — footers describe leaves, and we don't traverse into them.
        stats: Vec::new(),
    }
}

fn nested_children(dt: &DataType) -> Vec<ColumnInfo> {
    match dt {
        DataType::Struct(fields) => fields.iter().map(|f| column_info(f)).collect(),
        DataType::List(f) | DataType::LargeList(f) | DataType::FixedSizeList(f, _) => {
            vec![column_info(f)]
        }
        DataType::Map(entries, _) => nested_children(entries.data_type()),
        _ => Vec::new(),
    }
}

fn short_type(dt: &DataType) -> String {
    let full = format!("{dt:?}");
    let base: String = full.split(['(', '<']).next().unwrap_or(&full).to_string();
    match base.as_str() {
        "LargeUtf8" => "Utf8".into(),
        "LargeList" | "FixedSizeList" => "List".into(),
        other => other.to_string(),
    }
}

fn parse_dtype(label: &str) -> DataType {
    match label {
        "Int8" => DataType::Int8,
        "Int16" => DataType::Int16,
        "Int32" => DataType::Int32,
        "Int64" => DataType::Int64,
        "Float32" => DataType::Float32,
        "Float64" => DataType::Float64,
        "Date" | "Date32" => DataType::Date32,
        _ => DataType::Utf8,
    }
}
