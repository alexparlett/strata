//! The pagination engine: run each query **once**, spool the full result to a
//! [snapshot](crate::snapshots) keyed by [`SnapshotId`], then serve every page as a bounded
//! `LIMIT/OFFSET` read — so RAM only ever holds one page. Also the display-cell formatting
//! ([`CellFormat`]).
//!
//! Where those bytes go is [`SnapshotStore`]'s: the write pass here streams every batch into a
//! sink and asks for a provider once it settles, and knows nothing about what is on the other
//! side of it.
//!
//! Snapshots are keyed by [`SnapshotId`] — the Run's request id, unique per engine for
//! the life of the process — so a snapshot is **immutable**: a re-run materializes a
//! *new* snapshot under a new id, and every read keyed by an id targets a fixed set
//! (`docs/SNAPSHOT_SPEC.md`). Lifecycle (which ws owns which snapshot, when to retire one) is the
//! facade's own bookkeeping, in [`super::Engine`].

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use datafusion::arrow::compute::concat_batches;
use datafusion::arrow::datatypes::Field;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::common::Column;
use datafusion::logical_expr::expr::ScalarFunction;
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::*;
use datafusion::sql::parser::Statement as DFStatement;
use datafusion_functions_json::udfs::json_union_to_text_udf;
use datafusion_functions_json::JSON_UNION_DATA_TYPE;
use futures::StreamExt;

use crate::snapshots::ordinal::ordinal_name;
use crate::snapshots::{snapshot_name, SnapshotSink, SnapshotStats, SnapshotStore};
use strata_arrow::column_info;
use strata_arrow::config::DisplayStamp;
use strata_core::util::{clip, DISPLAY_CHARS};
use strata_model::{Cell, ColumnInfo, PageQuery, QueryOutput, SnapshotId};

/// Run the query **once**, streaming every batch straight into a fresh snapshot while counting
/// the exact total and capturing the first page — no separate `COUNT`, no re-read, bounded
/// memory. On failure the partial snapshot is discarded here (nothing was ever registered); the
/// caller only ever sees a fully-materialized snapshot or none (`QueryOutput::snapshot`).
pub async fn run_and_snapshot(
    ctx: &SessionContext,
    store: &dyn SnapshotStore,
    snapshot: SnapshotId,
    stmt: DFStatement,
    page_size: usize,
    fmt: &CellFormat,
    policy: ReadPolicy,
) -> Result<(QueryOutput, RecordBatch, SnapshotStats), String> {
    let result = materialize(ctx, store, snapshot, stmt, page_size, fmt, policy).await;
    if result.is_err() {
        store.retire(snapshot);
    }
    result
}

/// Render a `json_get` result as its canonical JSON text.
///
/// This used to be a **storage** gate: parquet cannot write an Arrow union at all, so a bare
/// `->` panicked the writer and every union had to be projected away or refused. The IPC
/// snapshot stores unions natively, so none of that is needed — the refusal arms (nested unions,
/// dictionary-wrapped unions, empty structs) are gone, and those results now round-trip as
/// themselves.
///
/// What remains is **presentation**, and it is lossless. `json_get`'s sparse union is the crate's
/// stand-in for Postgres `jsonb`; arrow renders it as `{str=x}` / `{int=7}`, which is not what
/// someone who typed `content -> 'type'` expects to read. `json_union_to_text` gives back exactly
/// the JSON the value came from, so this changes how the column reads and not what it holds.
///
/// Only a **top-level** union column is projected. One nested inside a struct or list is left
/// alone: there is nothing to wrap it with, and unlike before that is now merely cosmetic rather
/// than a crash.
fn json_unions_as_text(df: DataFrame) -> Result<DataFrame, String> {
    let schema = df.schema().clone();
    let is_union = |f: &Arc<Field>| f.data_type() == &*JSON_UNION_DATA_TYPE;
    if !schema.fields().iter().any(is_union) {
        return Ok(df);
    }
    let exprs = schema
        .columns()
        .into_iter()
        .zip(schema.fields())
        .map(|(column, field)| {
            if is_union(field) {
                Expr::ScalarFunction(ScalarFunction::new_udf(
                    json_union_to_text_udf(),
                    vec![Expr::Column(column)],
                ))
                .alias(field.name())
            } else {
                Expr::Column(column)
            }
        })
        .collect::<Vec<Expr>>();
    df.select(exprs).map_err(|e| e.to_string())
}

/// What a read is allowed to **plan** — the `SQLOptions` [`materialize`] puts in front of the
/// statement it is about to spool.
///
/// The read path's triple is all-false and that is the default: it is defense in depth behind the
/// router's classification (spec §4), so it may only ever narrow. The one widening is `EXECUTE`,
/// whose plan *is* a `LogicalPlan::Statement` — and it is safe for exactly one reason, which is
/// why it rides the dispatch rather than the path: `PREPARE` already verified the inner plan under
/// the read triple, and `verify_plan` cannot see through an `Execute` node (it has no inputs) to
/// do it again. A read that has not been through that fence never gets this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReadPolicy {
    /// Queries and introspection: DDL, DML and statements all refused.
    #[default]
    ReadOnly,
    /// The above, plus `LogicalPlan::Statement` — `EXECUTE` of a prepared query.
    Statements,
}

/// Plan one **already-resolved** statement under `policy` — `SessionContext::sql_with_options`
/// with the parse taken out, since `statements::pipeline::parse_one` did it.
///
/// The same three steps in the same order: plan, verify, `execute_logical_plan` (the half of that
/// method which performs a DDL plan — none the read policy admits). Rendering the resolved
/// statement back to text to keep the old signature is what this exists to avoid: the statement
/// judged has to be the statement that runs.
pub(crate) async fn plan_statement(
    ctx: &SessionContext,
    stmt: DFStatement,
    policy: ReadPolicy,
) -> Result<DataFrame, String> {
    let plan = ctx
        .state()
        .statement_to_plan(stmt)
        .await
        .map_err(|e| e.to_string())?;
    policy
        .options()
        .verify_plan(&plan)
        .map_err(|e| e.to_string())?;
    ctx.execute_logical_plan(plan)
        .await
        .map_err(|e| e.to_string())
}

impl ReadPolicy {
    fn options(self) -> SQLOptions {
        SQLOptions::new()
            .with_allow_dml(false)
            .with_allow_ddl(false)
            .with_allow_statements(self == ReadPolicy::Statements)
    }
}

async fn materialize(
    ctx: &SessionContext,
    store: &dyn SnapshotStore,
    snapshot: SnapshotId,
    stmt: DFStatement,
    page_size: usize,
    fmt: &CellFormat,
    policy: ReadPolicy,
) -> Result<(QueryOutput, RecordBatch, SnapshotStats), String> {
    let start = Instant::now();

    let df = plan_statement(ctx, stmt, policy).await?;
    let df = json_unions_as_text(df)?;
    let columns: Vec<ColumnInfo> = df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect();
    let arrow_schema = df.schema().inner().clone();

    let plain = !matches!(
        df.logical_plan(),
        LogicalPlan::Explain(_) | LogicalPlan::Analyze(_)
    );
    let unique = {
        let mut seen = HashSet::new();
        columns.iter().all(|c| seen.insert(c.name.as_str()))
    };
    let ord = (plain && unique).then(|| ordinal_name(&arrow_schema));
    let mut stream = df.execute_stream().await.map_err(|e| e.to_string())?;

    let mut sink: Option<Box<dyn SnapshotSink>> = None;
    let mut total = 0usize;
    let mut page1: Vec<Vec<Cell>> = Vec::new();
    let mut page1_batches: Vec<RecordBatch> = Vec::new();
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(|e| e.to_string())?;
        if sink.is_none() {
            sink = Some(store.begin(snapshot, batch.schema(), ord.clone())?);
        }
        if let Some(sink) = sink.as_mut() {
            sink.write(&batch)?;
        }
        total += batch.num_rows();
        append_batch_capped(&batch, &mut page1, &mut page1_batches, page_size, fmt)?;
    }

    let materialized = sink.is_some();
    let stats = match sink {
        Some(sink) => sink.settle()?,
        None => SnapshotStats::new(&arrow_schema, ord),
    };
    if materialized {
        let provider = store.open(ctx, snapshot).await?;
        ctx.register_table(snapshot_name(snapshot).as_str(), provider)
            .map_err(|e| e.to_string())?;
    }

    let page1_batch = concat_batches(&arrow_schema, &page1_batches).map_err(|e| e.to_string())?;
    Ok((
        QueryOutput {
            snapshot: materialized.then_some(snapshot),
            columns,
            rows: page1,
            total,
            page: 1,
            page_size,
            elapsed_ms: start.elapsed().as_millis(),
        },
        page1_batch,
        stats,
    ))
}

/// Display formatting for grid cells, resolved from a [`DisplayStamp`]. Owns the format strings
/// so an arrow [`FormatOptions`] can borrow them; `null` is the
/// literal shown for NULL cells (which stay flagged `null: true` for the grid's own dimmed
/// styling, so only the text changes).
///
/// From a stamp rather than from the engine's live config, so a cached read renders through the
/// value it is keyed on.
pub struct CellFormat {
    null: String,
    date: String,
    ts: String,
}

impl CellFormat {
    pub fn new(display: &DisplayStamp) -> Self {
        let eff = |k: &str| display.effective(k).unwrap_or_default();
        Self {
            null: eff("datafusion.format.null"),
            date: eff("datafusion.format.date_format"),
            ts: eff("datafusion.format.timestamp_format"),
        }
    }

    /// An arrow [`FormatOptions`] borrowing this config's date/timestamp patterns. Reachable
    /// from the sibling modules so a surface that renders the same values — the chart's axis
    /// labels ([`super::chart`]) — renders them the way the grid does.
    pub(super) fn opts(&self) -> FormatOptions<'_> {
        let mut o = FormatOptions::default();
        if !self.date.is_empty() {
            o = o.with_date_format(Some(&self.date));
        }
        if !self.ts.is_empty() {
            o = o.with_timestamp_format(Some(&self.ts));
        }
        o
    }
}

/// Append up to `cap` rows of `batch` to `out` (display cells), collecting the sliced batch
/// into `batches_out` (concatenated later into the page's type-aware `RecordBatch`).
fn append_batch_capped(
    batch: &RecordBatch,
    out: &mut Vec<Vec<Cell>>,
    batches_out: &mut Vec<RecordBatch>,
    cap: usize,
    fmt: &CellFormat,
) -> Result<(), String> {
    if out.len() >= cap {
        return Ok(());
    }
    let take = (cap - out.len()).min(batch.num_rows());
    let batch = batch.slice(0, take);
    let cols = batch.columns();
    let opts = fmt.opts();
    let fmts = cols
        .iter()
        .map(|c| ArrayFormatter::try_new(&**c, &opts))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for r in 0..take {
        let mut row = Vec::with_capacity(fmts.len());
        for (ci, f) in fmts.iter().enumerate() {
            let null = cols[ci].is_null(r);
            let text = if null {
                fmt.null.clone()
            } else {
                truncate_cell(&f.value(r).to_string())
            };
            row.push(Cell { text, null });
        }
        out.push(row);
    }
    batches_out.push(batch.clone());
    Ok(())
}

pub async fn fetch_page(
    ctx: &SessionContext,
    snapshot: SnapshotId,
    q: PageQuery,
    ord: Option<String>,
    fmt: &CellFormat,
) -> Result<Page, String> {
    let snap = snapshot_name(snapshot);
    let offset = q.page.saturating_sub(1).saturating_mul(q.page_size);
    read_page(ctx, &snap, offset, q.page_size, q.sort, ord, fmt).await
}

async fn read_page(
    ctx: &SessionContext,
    snap: &str,
    offset: usize,
    limit: usize,
    sort: Option<(String, bool)>,
    ord: Option<String>,
    fmt: &CellFormat,
) -> Result<Page, String> {
    let mut df = ctx.table(snap).await.map_err(|e| e.to_string())?;
    let mut order = Vec::new();
    if let Some((name, asc)) = sort {
        order.push(col(Column::from_name(name)).sort(asc, false));
    }
    if let Some(ord) = &ord {
        order.push(col(Column::from_name(ord.clone())).sort(true, false));
    }
    if !order.is_empty() {
        df = df.sort(order).map_err(|e| e.to_string())?;
    }
    let mut df = df.limit(offset, Some(limit)).map_err(|e| e.to_string())?;
    if let Some(ord) = &ord {
        df = df
            .drop_columns(&[ord.as_str()])
            .map_err(|e| e.to_string())?;
    }
    let schema = df.schema().inner().clone();
    let batches = df.collect().await.map_err(|e| e.to_string())?;
    let batch = concat_batches(&schema, &batches).map_err(|e| e.to_string())?;
    let rows = batches_to_rows(&batches, fmt)?;
    Ok((rows, batch))
}

/// A page of results: display cells for the grid + the page `RecordBatch` (type-aware source
/// for Copy/Export, Rz4).
type Page = (Vec<Vec<Cell>>, RecordBatch);

fn batches_to_rows(batches: &[RecordBatch], fmt: &CellFormat) -> Result<Vec<Vec<Cell>>, String> {
    let opts = fmt.opts();
    let mut rows = Vec::new();
    for batch in batches {
        let cols = batch.columns();
        let fmts = cols
            .iter()
            .map(|c| ArrayFormatter::try_new(&**c, &opts))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for r in 0..batch.num_rows() {
            let mut row = Vec::with_capacity(fmts.len());
            for (ci, f) in fmts.iter().enumerate() {
                let null = cols[ci].is_null(r);
                let text = if null {
                    fmt.null.clone()
                } else {
                    truncate_cell(&f.value(r).to_string())
                };
                row.push(Cell { text, null });
            }
            rows.push(row);
        }
    }
    Ok(rows)
}

fn truncate_cell(s: &str) -> String {
    clip(s, DISPLAY_CHARS).into_owned()
}
