//! The pagination engine: run each query **once**, spool the full result to a temp
//! parquet snapshot, then serve every page as a bounded `LIMIT/OFFSET` read — so RAM
//! only ever holds one page. Also the display-cell formatting (`CellFormat`).
//!
//! Snapshots are keyed by [`SnapshotId`] — the Run's request id, unique per engine for
//! the life of the process — so a snapshot is **immutable**: a re-run materializes a
//! *new* snapshot under a new id, and every read keyed by an id targets a fixed set
//! (`docs/SNAPSHOT_SPEC.md`). Lifecycle (which ws owns which snapshot, when to
//! [`retire_snapshot`]) is the worker loop's bookkeeping.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use datafusion::arrow::array::Array;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::prelude::*;
use futures::StreamExt;

use super::catalog::column_info;
use strata_model::{Cell, ColumnInfo, QueryOutput, SnapshotId};

/// Max characters kept per display cell (the grid truncates with an ellipsis).
const MAX_CELL_LEN: usize = 400;

// ---- query → snapshot → page ----

pub fn snapshot_name(snapshot: SnapshotId) -> String {
    format!("__snap_{snapshot}")
}

fn snapshots_root() -> String {
    let mut d = std::env::temp_dir();
    d.push("strata_snapshots");
    d.to_string_lossy().into_owned()
}

/// Per-engine snapshot subdirectory. Scoped by **pid + engine id**: `engine_id` is only
/// process-unique, and the snapshot root in the OS temp dir is machine-shared — without
/// the pid, two concurrent processes (a second app instance, parallel test binaries)
/// both allocate `e_0`, `e_1`, … and one process's cleanup deletes the other's live
/// snapshots.
pub fn snapshot_dir(engine_id: u64) -> String {
    let mut d = std::path::PathBuf::from(snapshots_root());
    d.push(format!("e_{}_{engine_id}", std::process::id()));
    d.to_string_lossy().into_owned()
}

pub fn snapshot_file(engine_id: u64, snapshot: SnapshotId) -> String {
    let mut d = std::path::PathBuf::from(snapshot_dir(engine_id));
    d.push(format!("s_{snapshot}.parquet"));
    d.to_string_lossy().into_owned()
}

/// Retire one snapshot: deregister its table and delete its file. Safe on a snapshot
/// that never fully materialized (a failed / cancelled run's partial) — both halves
/// are best-effort.
pub fn retire_snapshot(ctx: &SessionContext, engine_id: u64, snapshot: SnapshotId) {
    let _ = ctx.deregister_table(snapshot_name(snapshot).as_str());
    let _ = std::fs::remove_file(snapshot_file(engine_id, snapshot));
}

/// Remove *all* engines' snapshots. Safe only at process startup, before any
/// engine exists — at runtime an engine only ever cleans its own `snapshot_dir`.
pub fn purge_snapshot_root() {
    let _ = std::fs::remove_dir_all(snapshots_root());
}

/// Run the query **once**, streaming every batch straight to a fresh parquet snapshot
/// on disk while counting the exact total and capturing the first page — no separate
/// `COUNT`, no re-read, bounded memory. On failure the partial snapshot is cleaned up
/// here (nothing was ever registered); the caller only ever sees a fully-materialized
/// snapshot or none (`QueryOutput::snapshot`).
pub async fn run_and_snapshot(
    ctx: &SessionContext,
    engine_id: u64,
    snapshot: SnapshotId,
    sql: &str,
    page_size: usize,
    fmt: &CellFormat,
) -> Result<(QueryOutput, RecordBatch), String> {
    let result = materialize(ctx, engine_id, snapshot, sql, page_size, fmt).await;
    if result.is_err() {
        // The stream may have died mid-spool — drop the partial file (no table was
        // registered yet, so the id is simply never a readable snapshot).
        let _ = std::fs::remove_file(snapshot_file(engine_id, snapshot));
    }
    result
}

async fn materialize(
    ctx: &SessionContext,
    engine_id: u64,
    snapshot: SnapshotId,
    sql: &str,
    page_size: usize,
    fmt: &CellFormat,
) -> Result<(QueryOutput, RecordBatch), String> {
    let start = Instant::now();
    let snap = snapshot_name(snapshot);
    let file = snapshot_file(engine_id, snapshot);

    if let Some(parent) = Path::new(&file).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let opts = SQLOptions::new()
        .with_allow_dml(false)
        .with_allow_ddl(false)
        .with_allow_statements(false);

    let df = ctx
        .sql_with_options(sql, opts)
        .await
        .map_err(|e| e.to_string())?;
    // capture columns before the DataFrame is consumed by the stream
    let columns: Vec<ColumnInfo> = df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect();
    // Arrow schema of the result — captured before the DataFrame is consumed by the stream,
    // for concatenating page 1 into its `RecordBatch`.
    let arrow_schema = df.schema().inner().clone();
    let mut stream = df.execute_stream().await.map_err(|e| e.to_string())?;

    let mut writer: Option<ArrowWriter<std::fs::File>> = None;
    let mut total = 0usize;
    let mut page1: Vec<Vec<Cell>> = Vec::new();
    let mut page1_batches: Vec<RecordBatch> = Vec::new();
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(|e| e.to_string())?;
        total += batch.num_rows();
        if writer.is_none() {
            let out = std::fs::File::create(&file).map_err(|e| e.to_string())?;
            writer =
                Some(ArrowWriter::try_new(out, batch.schema(), None).map_err(|e| e.to_string())?);
        }
        if let Some(w) = writer.as_mut() {
            w.write(&batch).map_err(|e| e.to_string())?;
        }
        append_batch_capped(&batch, &mut page1, &mut page1_batches, page_size, fmt)?;
    }

    // Only register a snapshot if the query produced rows; an empty result has
    // no pages to fetch (`QueryOutput::snapshot` stays `None`).
    let materialized = writer.is_some();
    if let Some(w) = writer {
        w.close().map_err(|e| e.to_string())?;
        ctx.register_parquet(snap.as_str(), file.as_str(), ParquetReadOptions::default())
           .await
           .map_err(|e| e.to_string())?;
    }

    let page1_batch = datafusion::arrow::compute::concat_batches(&arrow_schema, &page1_batches)
        .map_err(|e| e.to_string())?;
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
    ))
}

/// Display formatting for grid cells, derived from the engine's `datafusion.format.*`
/// overrides (W2). Owns the format strings so an arrow [`FormatOptions`] can borrow
/// them; `null` is the literal shown for NULL cells (which stay flagged `null: true`
/// for the grid's own dimmed styling, so only the text changes).
pub struct CellFormat {
    null: String,
    date: String,
    ts: String,
}

impl CellFormat {
    pub fn new(overrides: &BTreeMap<String, String>) -> Self {
        let eff = |k: &str| crate::engine::config::effective(overrides, k).unwrap_or_default();
        Self {
            null: eff("datafusion.format.null"),
            date: eff("datafusion.format.date_format"),
            ts: eff("datafusion.format.timestamp_format"),
        }
    }

    /// An arrow [`FormatOptions`] borrowing this config's date/timestamp patterns.
    fn opts(&self) -> FormatOptions<'_> {
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
    page: usize,
    page_size: usize,
    sort: Option<(String, bool)>,
    fmt: &CellFormat,
) -> Result<Page, String> {
    let snap = snapshot_name(snapshot);
    let offset = page.saturating_sub(1) * page_size;
    read_page(ctx, &snap, offset, page_size, sort, fmt).await
}

async fn read_page(
    ctx: &SessionContext,
    snap: &str,
    offset: usize,
    limit: usize,
    sort: Option<(String, bool)>,
    fmt: &CellFormat,
) -> Result<Page, String> {
    let mut df = ctx.table(snap).await.map_err(|e| e.to_string())?;
    // Arrow schema of the page (sort/limit preserve it) — for concatenating the page batch.
    let schema = df.schema().inner().clone();
    if let Some((name, asc)) = sort {
        // ORDER BY the chosen column over the whole snapshot, then take the page window.
        // `Column::from_name` avoids identifier parsing on odd column names; `nulls_first =
        // false` ⇒ nulls always sort last, both directions (Rz6).
        let expr = col(datafusion::common::Column::from_name(name)).sort(asc, false);
        df = df.sort(vec![expr]).map_err(|e| e.to_string())?;
    }
    let batches = df
        .limit(offset, Some(limit))
        .map_err(|e| e.to_string())?
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    let batch =
        datafusion::arrow::compute::concat_batches(&schema, &batches).map_err(|e| e.to_string())?;
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
    if s.len() <= MAX_CELL_LEN {
        return s.to_string();
    }
    let mut end = MAX_CELL_LEN;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
