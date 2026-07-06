//! DataFusion engine on a dedicated thread with its own Tokio runtime.
//!
//! Pagination model (bounded memory): each query is executed **once** and its
//! full result is spooled to a temporary parquet **snapshot** on disk. The true
//! row count comes from a `COUNT(*)` over the snapshot, and every page is a
//! bounded `LIMIT/OFFSET` read from it — so RAM only ever holds one page, no
//! matter how far the user pages, and no query is ever recomputed per page.
//!
//! UI → engine: `tokio::mpsc::unbounded` of [`Command`]. engine → UI:
//! `tokio::mpsc::unbounded` of [`Event`], drained by a Dioxus coroutine.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::plan::{PlanKind, PlanNode, QueryPlan};
use crate::util::Kind;
use datafusion::arrow::array::Array;
use datafusion::arrow::datatypes::{DataType, Field};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::logical_expr::LogicalPlan;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::physical_plan::display::DisplayableExecutionPlan;
use datafusion::physical_plan::{collect, displayable, ExecutionPlan};
use datafusion::prelude::*;
use futures::StreamExt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const MAX_CELL_LEN: usize = 400;

#[derive(Clone, Debug)]
pub struct ColumnInfo {
    pub name: String,
    pub dtype: String,
    pub kind: Kind,
    pub nullable: bool,
    pub children: Vec<ColumnInfo>,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub null: bool,
}

/// The current page of a query, plus the snapshot's true total.
#[derive(Clone, Debug, Default)]
pub struct QueryOutput {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Cell>>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug)]
pub struct TableSpec {
    pub name: String,
    pub paths: Vec<String>,
    pub format: String,
    pub partitions: Vec<(String, String)>,
}

pub enum Command {
    Register(TableSpec),
    Deregister {
        table: String,
    },
    CreateView {
        name: String,
        sql: String,
    },
    DropView {
        name: String,
    },
    /// Run a query → spool a snapshot → return page 1 + total.
    Query {
        req_id: u64,
        ws_id: u64,
        sql: String,
        page_size: usize,
    },
    /// Run an `EXPLAIN [ANALYZE]` and return its parsed plan tree (no snapshot).
    Explain {
        req_id: u64,
        ws_id: u64,
        sql: String,
    },
    /// Read a page from the workspace's existing snapshot (no recompute).
    FetchPage {
        ws_id: u64,
        page: usize,
        page_size: usize,
    },
    /// Drop one workspace's snapshot (table + temp file) — e.g. on tab close.
    CleanupWorkspace {
        ws_id: u64,
    },
    /// Remove all snapshots (e.g. on app exit).
    CleanupAll,
    /// Write a workspace's snapshot to a file (or, with `partition_cols`, a
    /// Hive-partitioned directory) via `COPY … TO`.
    Export {
        ws_id: u64,
        path: String,
        format: String,
        all: bool,
        page: usize,
        page_size: usize,
        csv_delimiter: char,
        csv_header: bool,
        csv_null: String,
        pq_compression: String,
        pq_level: u32,
        partition_cols: Vec<String>,
        keep_partition: bool,
    },
}

pub enum Event {
    Registered {
        table: String,
        path: String,
        result: Result<Vec<ColumnInfo>, String>,
    },
    Deregistered {
        table: String,
    },
    ViewChanged {
        name: String,
        sql: String,
        dropped: bool,
        result: Result<Vec<ColumnInfo>, String>,
    },
    QueryResult {
        req_id: u64,
        ws_id: u64,
        result: Result<QueryOutput, String>,
    },
    /// Result of an `EXPLAIN [ANALYZE]` — a parsed plan tree or an error.
    ExplainResult {
        req_id: u64,
        ws_id: u64,
        result: Result<QueryPlan, String>,
    },
    PageResult {
        ws_id: u64,
        page: usize,
        result: Result<Vec<Vec<Cell>>, String>,
    },
    /// Result of an export: `Ok((path, rows_written))` or an error message.
    Exported {
        result: Result<(String, usize), String>,
    },
    Notice(String),
}

pub struct Handle {
    pub cmd_tx: UnboundedSender<Command>,
    pub evt_rx: UnboundedReceiver<Event>,
}

/// Process-unique id per spawned engine (one per project window), used to scope
/// snapshot files so windows never collide.
static ENGINE_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn spawn() -> Handle {
    let (cmd_tx, cmd_rx) = unbounded_channel::<Command>();
    let (evt_tx, evt_rx) = unbounded_channel::<Event>();
    let engine_id = ENGINE_SEQ.fetch_add(1, Ordering::Relaxed);
    std::thread::Builder::new()
        .name(format!("df-engine-{engine_id}"))
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(engine_loop(cmd_rx, evt_tx, engine_id));
        })
        .expect("spawn engine");
    Handle { cmd_tx, evt_rx }
}

async fn engine_loop(
    mut cmd_rx: UnboundedReceiver<Command>,
    evt_tx: UnboundedSender<Event>,
    engine_id: u64,
) {
    // Leftover snapshots from a previous run are cleared once at process start
    // (`purge_snapshot_root`), not here — wiping the shared root at runtime would
    // clobber other windows' engines.
    let ctx = SessionContext::new();
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::Register(spec) => {
                let path = spec.paths.first().cloned().unwrap_or_default();
                let result = register_external(&ctx, &spec).await;
                let _ = evt_tx.send(Event::Registered {
                    table: spec.name,
                    path,
                    result,
                });
            }
            Command::Deregister { table } => {
                let _ = ctx.deregister_table(table.as_str());
                let _ = evt_tx.send(Event::Deregistered { table });
            }
            Command::CreateView { name, sql } => {
                let stmt = format!("CREATE OR REPLACE VIEW {name} AS {sql}");
                let result = match ctx.sql(&stmt).await {
                    Ok(df) => {
                        let _ = df.collect().await;
                        match ctx.table(name.as_str()).await {
                            Ok(t) => {
                                Ok(t.schema().fields().iter().map(|f| column_info(f)).collect())
                            }
                            Err(e) => Err(e.to_string()),
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = evt_tx.send(Event::ViewChanged {
                    name,
                    sql,
                    dropped: false,
                    result,
                });
            }
            Command::DropView { name } => {
                let result = ctx
                    .sql(&format!("DROP VIEW IF EXISTS {name}"))
                    .await
                    .map(|_| Vec::new())
                    .map_err(|e| e.to_string());
                let _ = evt_tx.send(Event::ViewChanged {
                    name,
                    sql: String::new(),
                    dropped: true,
                    result,
                });
            }
            Command::Query {
                req_id,
                ws_id,
                sql,
                page_size,
            } => {
                let result = run_and_snapshot(&ctx, engine_id, ws_id, &sql, page_size).await;
                let _ = evt_tx.send(Event::QueryResult {
                    req_id,
                    ws_id,
                    result,
                });
            }
            Command::Explain { req_id, ws_id, sql } => {
                let result = run_explain(&ctx, &sql).await;
                let _ = evt_tx.send(Event::ExplainResult {
                    req_id,
                    ws_id,
                    result,
                });
            }
            Command::FetchPage {
                ws_id,
                page,
                page_size,
            } => {
                let result = fetch_page(&ctx, ws_id, page, page_size).await;
                let _ = evt_tx.send(Event::PageResult {
                    ws_id,
                    page,
                    result,
                });
            }
            Command::CleanupWorkspace { ws_id } => {
                let _ = ctx.deregister_table(snapshot_name(ws_id).as_str());
                let _ = std::fs::remove_file(snapshot_file(engine_id, ws_id));
            }
            Command::CleanupAll => {
                // Only this engine's (this window's) snapshots.
                let _ = std::fs::remove_dir_all(snapshot_dir(engine_id));
            }
            Command::Export {
                ws_id,
                path,
                format,
                all,
                page,
                page_size,
                csv_delimiter,
                csv_header,
                csv_null,
                pq_compression,
                pq_level,
                partition_cols,
                keep_partition,
            } => {
                let result = run_export(
                    &ctx,
                    ws_id,
                    ExportArgs {
                        path,
                        format,
                        all,
                        page,
                        page_size,
                        csv_delimiter,
                        csv_header,
                        csv_null,
                        pq_compression,
                        pq_level,
                        partition_cols,
                        keep_partition,
                    },
                )
                .await;
                let _ = evt_tx.send(Event::Exported { result });
            }
        }
    }

    // Command channel closed → this window's engine is done → tidy its snapshots
    // (belt-and-suspenders with the `CleanupAll` from `use_drop`; the startup
    // `purge_snapshot_root` covers an abrupt app exit that skips both).
    let _ = std::fs::remove_dir_all(snapshot_dir(engine_id));
}

/// Everything an export needs (one struct to dodge a too-many-arguments fn).
struct ExportArgs {
    path: String,
    format: String,
    all: bool,
    page: usize,
    page_size: usize,
    csv_delimiter: char,
    csv_header: bool,
    csv_null: String,
    pq_compression: String,
    pq_level: u32,
    partition_cols: Vec<String>,
    keep_partition: bool,
}

/// Export a workspace's snapshot via `COPY (…) TO … STORED AS`. A plain file path
/// (extension) → one file; `partition_cols` → a Hive-partitioned directory.
/// Returns `(path, rows_written)`.
async fn run_export(
    ctx: &SessionContext,
    ws_id: u64,
    a: ExportArgs,
) -> Result<(String, usize), String> {
    let snap = snapshot_name(ws_id);
    if ctx.table(snap.as_str()).await.is_err() {
        return Err("No results to export — run a query first".to_string());
    }

    let select = if a.all {
        format!("SELECT * FROM {snap}")
    } else {
        let offset = a.page.saturating_sub(1) * a.page_size;
        format!("SELECT * FROM {snap} LIMIT {} OFFSET {offset}", a.page_size)
    };
    let stored = match a.format.as_str() {
        "json" => "JSON",
        "parquet" => "PARQUET",
        "arrow" => "ARROW",
        _ => "CSV",
    };
    let part_clause = if a.partition_cols.is_empty() {
        String::new()
    } else {
        format!(" PARTITIONED BY ({})", a.partition_cols.join(", "))
    };
    // Format options (JSON/Arrow take none here; JSON always writes NDJSON).
    let opts = match a.format.as_str() {
        "csv" => {
            let nv = match a.csv_null.as_str() {
                "null" => "NULL",
                "nan" => "NaN",
                _ => "",
            };
            format!(
                " OPTIONS ('HAS_HEADER' '{}', 'DELIMITER' '{}', 'NULL_VALUE' '{}')",
                a.csv_header, a.csv_delimiter, nv
            )
        }
        "parquet" => format!(
            " OPTIONS ('COMPRESSION' '{}')",
            pq_codec(&a.pq_compression, a.pq_level)
        ),
        _ => String::new(),
    };

    // `keep_partition_by_columns` is a session config, not a COPY option — set it
    // explicitly per partitioned export (default off).
    if !a.partition_cols.is_empty() {
        if let Ok(df) = ctx
            .sql(&format!(
                "SET datafusion.execution.keep_partition_by_columns = {}",
                a.keep_partition
            ))
            .await
        {
            let _ = df.collect().await;
        }
    }

    let esc = a.path.replace('\'', "''");
    let stmt = format!("COPY ({select}) TO '{esc}' STORED AS {stored}{part_clause}{opts}");

    let df = ctx.sql(&stmt).await.map_err(|e| e.to_string())?;
    let batches = df.collect().await.map_err(|e| e.to_string())?;
    Ok((a.path, copy_row_count(&batches)))
}

/// Parquet compression codec string, with a level for the codecs that take one.
fn pq_codec(codec: &str, level: u32) -> String {
    match codec {
        "snappy" => "snappy".into(),
        "lz4" => "lz4".into(),
        "none" | "uncompressed" => "uncompressed".into(),
        "gzip" => format!("gzip({})", level.clamp(1, 9)),
        "brotli" => format!("brotli({})", level.clamp(1, 11)),
        _ => format!("zstd({})", level.clamp(1, 22)),
    }
}

/// `COPY … TO` returns a single `UInt64` "count" column with the rows written.
fn copy_row_count(batches: &[RecordBatch]) -> usize {
    use datafusion::arrow::array::UInt64Array;
    let Some(batch) = batches.first() else {
        return 0;
    };
    if batch.num_columns() == 0 {
        return 0;
    }
    batch
        .column(0)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .filter(|a| !a.is_empty())
        .map(|a| a.value(0) as usize)
        .unwrap_or(0)
}

// ---- query → snapshot → page ----

fn snapshot_name(ws_id: u64) -> String {
    format!("__snap_{ws_id}")
}

fn snapshots_root() -> String {
    let mut d = std::env::temp_dir();
    d.push("strata_snapshots");
    d.to_string_lossy().into_owned()
}

/// Per-engine snapshot subdirectory. Each window runs its own engine with a
/// unique `engine_id`, so windows never share (or clobber) snapshot files even
/// though their workspace ids overlap (every project numbers workspaces from 1).
fn snapshot_dir(engine_id: u64) -> String {
    let mut d = std::path::PathBuf::from(snapshots_root());
    d.push(format!("e_{engine_id}"));
    d.to_string_lossy().into_owned()
}

fn snapshot_file(engine_id: u64, ws_id: u64) -> String {
    let mut d = std::path::PathBuf::from(snapshot_dir(engine_id));
    d.push(format!("ws_{ws_id}.parquet"));
    d.to_string_lossy().into_owned()
}

/// Remove *all* engines' snapshots. Safe only at process startup, before any
/// engine exists — at runtime an engine only ever cleans its own `snapshot_dir`.
pub fn purge_snapshot_root() {
    let _ = std::fs::remove_dir_all(snapshots_root());
}

/// Run the query **once**, streaming every batch straight to a parquet snapshot
/// on disk while counting the exact total and capturing the first page — no
/// separate `COUNT`, no re-read, bounded memory.
async fn run_and_snapshot(
    ctx: &SessionContext,
    engine_id: u64,
    ws_id: u64,
    sql: &str,
    page_size: usize,
) -> Result<QueryOutput, String> {
    let start = Instant::now();
    let snap = snapshot_name(ws_id);
    let file = snapshot_file(engine_id, ws_id);

    // reset the previous snapshot for this workspace
    let _ = ctx.deregister_table(snap.as_str());
    let _ = std::fs::remove_file(&file);
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
    let mut stream = df.execute_stream().await.map_err(|e| e.to_string())?;

    let opts = FormatOptions::default();
    let mut writer: Option<ArrowWriter<std::fs::File>> = None;
    let mut total = 0usize;
    let mut page1: Vec<Vec<Cell>> = Vec::new();
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
        append_batch_capped(&batch, &mut page1, page_size, &opts)?;
    }

    // Only register a snapshot if the query produced rows; an empty result has
    // no pages to fetch.
    if let Some(w) = writer {
        w.close().map_err(|e| e.to_string())?;
        ctx.register_parquet(snap.as_str(), file.as_str(), ParquetReadOptions::default())
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(QueryOutput {
        columns,
        rows: page1,
        total,
        page: 1,
        page_size,
        elapsed_ms: start.elapsed().as_millis(),
    })
}

/// Append up to `cap` rows of `batch` (as display cells) to `out`.
fn append_batch_capped(
    batch: &RecordBatch,
    out: &mut Vec<Vec<Cell>>,
    cap: usize,
    opts: &FormatOptions,
) -> Result<(), String> {
    if out.len() >= cap {
        return Ok(());
    }
    let cols = batch.columns();
    let fmts = cols
        .iter()
        .map(|c| ArrayFormatter::try_new(&**c, opts))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for r in 0..batch.num_rows() {
        if out.len() >= cap {
            break;
        }
        let mut row = Vec::with_capacity(fmts.len());
        for (ci, f) in fmts.iter().enumerate() {
            let null = cols[ci].is_null(r);
            let text = if null {
                "null".to_string()
            } else {
                truncate_cell(&f.value(r).to_string())
            };
            row.push(Cell { text, null });
        }
        out.push(row);
    }
    Ok(())
}

async fn fetch_page(
    ctx: &SessionContext,
    ws_id: u64,
    page: usize,
    page_size: usize,
) -> Result<Vec<Vec<Cell>>, String> {
    let snap = snapshot_name(ws_id);
    let offset = page.saturating_sub(1) * page_size;
    read_page(ctx, &snap, offset, page_size).await
}

async fn read_page(
    ctx: &SessionContext,
    snap: &str,
    offset: usize,
    limit: usize,
) -> Result<Vec<Vec<Cell>>, String> {
    let batches = ctx
        .table(snap)
        .await
        .map_err(|e| e.to_string())?
        .limit(offset, Some(limit))
        .map_err(|e| e.to_string())?
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    batches_to_rows(&batches)
}

fn batches_to_rows(batches: &[RecordBatch]) -> Result<Vec<Vec<Cell>>, String> {
    let opts = FormatOptions::default();
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
                    "null".to_string()
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

/// Build a structured [`QueryPlan`] for an `EXPLAIN [ANALYZE]` statement by
/// walking DataFusion's own typed plans — **no plan-text parsing**.
///
/// We plan the EXPLAIN, unwrap the `Explain`/`Analyze` wrapper to the real inner
/// `LogicalPlan`, re-plan it to a physical `ExecutionPlan`, and (for ANALYZE)
/// execute it so each operator's live `MetricsSet` is populated. Then we walk the
/// logical and physical trees into `PlanNode`s, reading each node's name,
/// one-line detail, and metrics directly from the DataFusion types.
async fn run_explain(ctx: &SessionContext, sql: &str) -> Result<QueryPlan, String> {
    let opts = SQLOptions::new()
        .with_allow_dml(false)
        .with_allow_ddl(false)
        .with_allow_statements(false);

    let df = ctx
        .sql_with_options(sql, opts)
        .await
        .map_err(|e| e.to_string())?;

    // Unwrap `EXPLAIN`/`EXPLAIN ANALYZE` to the plan being explained.
    let (inner, analyze) = match df.logical_plan() {
        LogicalPlan::Explain(e) => (e.plan.as_ref(), false),
        LogicalPlan::Analyze(a) => (a.input.as_ref(), true),
        other => (other, false),
    };

    let mut plan = QueryPlan {
        analyze,
        logical: walk_logical(inner),
        logical_text: inner.display_indent().to_string(),
        ..Default::default()
    };

    // Re-plan the inner logical plan to physical. `SessionState` has an inherent
    // `create_physical_plan` in DataFusion 43 (no `Session` trait import needed).
    let state = ctx.state();
    let physical = state
        .create_physical_plan(inner)
        .await
        .map_err(|e| e.to_string())?;

    // ANALYZE: run the query so live metrics land on the plan's operators.
    if analyze {
        let _ = collect(physical.clone(), ctx.task_ctx())
            .await
            .map_err(|e| e.to_string())?;
    }

    plan.physical = walk_physical(physical.as_ref());
    plan.physical_text = if analyze {
        DisplayableExecutionPlan::with_metrics(physical.as_ref())
            .indent(false)
            .to_string()
    } else {
        displayable(physical.as_ref()).indent(false).to_string()
    };

    if !plan.is_some() {
        return Err("Could not build the query plan".to_string());
    }
    Ok(plan)
}

/// Flatten a logical plan into depth-tagged `PlanNode`s. `LogicalPlan::display`
/// renders one node without its children (e.g. `Projection: id`).
fn walk_logical(root: &LogicalPlan) -> Vec<PlanNode> {
    fn go(p: &LogicalPlan, depth: usize, out: &mut Vec<PlanNode>) {
        let (name, detail) = crate::plan::split_name_detail(p.display().to_string().trim());
        out.push(PlanNode {
            kind: PlanKind::classify(&name),
            name,
            detail,
            depth,
            rows: None,
            ms_val: None,
            ms_label: String::new(),
            extra: String::new(),
        });
        for c in p.inputs() {
            go(c, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    go(root, 0, &mut out);
    out
}

/// Flatten a physical plan into depth-tagged `PlanNode`s, reading each operator's
/// one-line display and (if executed) its metrics.
fn walk_physical(root: &dyn ExecutionPlan) -> Vec<PlanNode> {
    fn go(p: &dyn ExecutionPlan, depth: usize, out: &mut Vec<PlanNode>) {
        let line = displayable(p).one_line().to_string();
        let (name, detail) = crate::plan::split_name_detail(line.trim());
        let (rows, ms_val, ms_label, extra) = node_metrics(p);
        out.push(PlanNode {
            kind: PlanKind::classify(&name),
            name,
            detail,
            depth,
            rows,
            ms_val,
            ms_label,
            extra,
        });
        for c in p.children() {
            go(c.as_ref(), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    go(root, 0, &mut out);
    out
}

/// Read a physical operator's metrics: output rows, compute time (ns → ms), and
/// every other named metric formatted via its `MetricValue` (units preserved).
fn node_metrics(p: &dyn ExecutionPlan) -> (Option<u64>, Option<f64>, String, String) {
    let Some(ms) = p.metrics() else {
        return (None, None, String::new(), String::new());
    };
    let ms = ms.aggregate_by_name();
    let rows = ms.output_rows().map(|r| r as u64);
    let ms_val = ms.elapsed_compute().map(|ns| ns as f64 / 1_000_000.0);
    let ms_label = ms_val.map(crate::plan::fmt_ms).unwrap_or_default();

    let mut extras = Vec::new();
    for m in ms.iter() {
        let v = m.value();
        match v.name() {
            // Shown separately (rows/time) or noise.
            "output_rows" | "elapsed_compute" | "start_timestamp" | "end_timestamp" => {}
            name => extras.push(format!("{name}={v}")),
        }
    }
    (rows, ms_val, ms_label, extras.join(" · "))
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

// ---- external table registration ----

async fn register_external(
    ctx: &SessionContext,
    spec: &TableSpec,
) -> Result<Vec<ColumnInfo>, String> {
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
    let mut opts = ListingOptions::new(fmt).with_file_extension(ext);
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

    let df = ctx
        .table(spec.name.as_str())
        .await
        .map_err(|e| e.to_string())?;
    Ok(df
        .schema()
        .fields()
        .iter()
        .map(|f| column_info(f))
        .collect())
}

// ---- schema helpers ----

fn column_info(field: &Field) -> ColumnInfo {
    let dtype = short_type(field.data_type());
    ColumnInfo {
        name: field.name().clone(),
        kind: Kind::from_arrow(&dtype),
        dtype,
        nullable: field.is_nullable(),
        children: nested_children(field.data_type()),
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
