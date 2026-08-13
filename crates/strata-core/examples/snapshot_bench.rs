//! Snapshot storage benchmark: parquet vs Arrow IPC, with and without compression.
//!
//! The question is whether snapshots should stay parquet. Parquet is a *storage interchange*
//! format with a narrower type system than Arrow — it cannot write a union at all
//! (`arrow_to_parquet_schema` panics, ARROW-8817) nor a zero-field struct — so every result has
//! to be coerced on the way in, and the record view and JSON/CSV export then read that coerced
//! form rather than the types the query produced. Arrow IPC round-trips anything the engine can
//! emit. What it gives up is parquet's encoding (dictionary/RLE/delta) and its footer statistics.
//!
//! This measures the part that is actually in question: **size on disk**, and whether paged reads
//! stay comparable. Four configurations, because our snapshots are written **uncompressed** today
//! (`snapshot_writer_props` sets only `EnabledStatistics::Chunk`, and parquet-rs defaults to
//! `Compression::UNCOMPRESSED`) — so comparing IPC+LZ4 against today's parquet would flatter IPC
//! by crediting parquet with compression it is not doing.
//!
//! Run: `cargo run --release -p strata-core --example snapshot_bench`
//! (`--release` matters — LZ4 throughput in a debug build is not the number you want.)

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::ipc::writer::{FileWriter, IpcWriteOptions};
use datafusion::arrow::ipc::CompressionType;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::execution::options::ArrowReadOptions;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::parquet::basic::Compression;
use datafusion::parquet::file::properties::{EnabledStatistics, WriterProperties};
use datafusion::prelude::*;

/// The shapes a result actually takes, chosen so each format's best case is represented.
const DATASETS: &[(&str, &str, usize)] = &[
    (
        "lowcard",
        "SELECT 'country_' || (value % 20)::text AS country, \
                'status_' || (value % 5)::text AS status, \
                value AS id \
         FROM generate_series(1, {N})",
        2_000_000,
    ),
    (
        "numeric",
        "SELECT value AS id, value * 1.5 AS amount, value % 1000 AS bucket \
         FROM generate_series(1, {N})",
        2_000_000,
    ),
    (
        "mixed",
        "SELECT value AS id, \
                'user_' || (value % 50000)::text AS name, \
                value % 97 AS score, \
                CASE WHEN value % 13 = 0 THEN NULL ELSE 'tag_' || (value % 5)::text END AS tag, \
                to_timestamp(value) AS ts \
         FROM generate_series(1, {N})",
        1_000_000,
    ),
];

struct Row {
    config: &'static str,
    bytes: u64,
    write: Duration,
    first_page: Duration,
    late_page: Duration,
    sorted_page: Duration,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scale: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1);

    let dir = std::env::temp_dir().join("strata_snapshot_bench");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;

    for (name, sql, n) in DATASETS {
        let ctx = SessionContext::new();
        let n = n * scale;
        let sql = sql.replace("{N}", &n.to_string());
        let df = ctx.sql(&sql).await?;
        let schema = df.schema().inner().clone();
        let t = Instant::now();
        let batches = df.collect().await?;
        let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
        let _ = n;
        println!(
            "\n=== {name}: {rows} rows x {} cols (materialized in {:?})",
            schema.fields().len(),
            t.elapsed()
        );

        let mut out = Vec::new();
        for (config, ext) in [
            ("parquet (today)", "parquet"),
            ("parquet + lz4", "parquet"),
            ("ipc", "arrow"),
            ("ipc + lz4", "arrow"),
        ] {
            let path = dir.join(format!("{name}_{}.{ext}", config.replace([' ', '+'], "")));
            let (bytes, write) = match config {
                "parquet (today)" => write_parquet(&path, &schema, &batches, None)?,
                "parquet + lz4" => {
                    write_parquet(&path, &schema, &batches, Some(Compression::LZ4_RAW))?
                }
                "ipc" => write_ipc(&path, &schema, &batches, None)?,
                _ => write_ipc(&path, &schema, &batches, Some(CompressionType::LZ4_FRAME))?,
            };
            let (first_page, late_page, sorted_page) =
                time_reads(&path, ext, rows, schema.field(0).name()).await?;
            out.push(Row {
                config,
                bytes,
                write,
                first_page,
                late_page,
                sorted_page,
            });
        }

        let base = out[0].bytes as f64;
        println!(
            "{:<18} {:>10} {:>7} {:>9} {:>10} {:>10} {:>12}",
            "config", "size", "vs now", "write", "page 1", "page last", "sorted page"
        );
        for r in &out {
            println!(
                "{:<18} {:>9.1}M {:>6.2}x {:>8.2?} {:>9.2?} {:>9.2?} {:>11.2?}",
                r.config,
                r.bytes as f64 / 1_048_576.0,
                r.bytes as f64 / base,
                r.write,
                r.first_page,
                r.late_page,
                r.sorted_page,
            );
        }
    }
    Ok(())
}

fn write_parquet(
    path: &PathBuf,
    schema: &SchemaRef,
    batches: &[RecordBatch],
    compression: Option<Compression>,
) -> Result<(u64, Duration), Box<dyn std::error::Error>> {
    let mut props = WriterProperties::builder().set_statistics_enabled(EnabledStatistics::Chunk);
    if let Some(c) = compression {
        props = props.set_compression(c);
    }
    let t = Instant::now();
    let mut w = ArrowWriter::try_new(File::create(path)?, schema.clone(), Some(props.build()))?;
    for b in batches {
        w.write(b)?;
    }
    w.close()?;
    let elapsed = t.elapsed();
    Ok((std::fs::metadata(path)?.len(), elapsed))
}

fn write_ipc(
    path: &PathBuf,
    schema: &SchemaRef,
    batches: &[RecordBatch],
    compression: Option<CompressionType>,
) -> Result<(u64, Duration), Box<dyn std::error::Error>> {
    let mut opts = IpcWriteOptions::default();
    if let Some(c) = compression {
        opts = opts.try_with_compression(Some(c))?;
    }
    let t = Instant::now();
    let mut w = FileWriter::try_new_with_options(File::create(path)?, schema, opts)?;
    for b in batches {
        w.write(b)?;
    }
    w.finish()?;
    let elapsed = t.elapsed();
    Ok((std::fs::metadata(path)?.len(), elapsed))
}

/// The three reads the grid actually performs, through the same `table -> sort -> limit` shape as
/// `query::read_page`: the first page, a page at the very end (where row-group / record-batch
/// skipping is the whole difference), and a sorted page (a full scan plus sort — the worst case,
/// and the one the status bar's pager hits whenever a column header is clicked).
async fn time_reads(
    path: &Path,
    ext: &str,
    rows: usize,
    sort_col: &str,
) -> Result<(Duration, Duration, Duration), Box<dyn std::error::Error>> {
    let ctx = SessionContext::new();
    let loc = path.to_string_lossy().to_string();
    if ext == "parquet" {
        ctx.register_parquet("snap", &loc, ParquetReadOptions::default())
            .await?;
    } else {
        ctx.register_arrow("snap", &loc, ArrowReadOptions::default())
            .await?;
    }

    let page = |offset: usize, sort: bool| {
        let ctx = ctx.clone();
        let sort_col = sort_col.to_string();
        async move {
            let t = Instant::now();
            let mut df = ctx.table("snap").await?;
            if sort {
                df = df.sort(vec![col(Column::from_name(&sort_col)).sort(false, false)])?;
            }
            let _ = df.limit(offset, Some(100))?.collect().await?;
            Ok::<Duration, datafusion::error::DataFusionError>(t.elapsed())
        }
    };

    let first = page(0, false).await?;
    let late = page(rows.saturating_sub(100), false).await?;
    let sorted = page(0, true).await?;
    Ok((first, late, sorted))
}
