//! Export one result snapshot to disk via `COPY … TO` (one file, or a Hive
//! directory when partition columns are given).
//!
//! Not yet reachable from the facade — `Engine::export` lands with the Freya export
//! task (dead-code-allowed until then, like the other feature reservoirs).
#![allow(dead_code)]

use datafusion::arrow::array::Array;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::prelude::*;

use super::query::snapshot_name;
use strata_model::SnapshotId;

/// Everything an export needs (one struct to dodge a too-many-arguments fn).
pub struct ExportArgs {
    pub path: String,
    pub format: String,
    pub all: bool,
    pub page: usize,
    pub page_size: usize,
    pub csv_delimiter: char,
    pub csv_header: bool,
    pub csv_null: String,
    pub pq_compression: String,
    pub pq_level: u32,
    pub partition_cols: Vec<String>,
    pub keep_partition: bool,
}

/// Export one snapshot via `COPY (…) TO … STORED AS`. A plain file path (extension)
/// → one file; `partition_cols` → a Hive-partitioned directory.
/// Returns `(path, rows_written)`.
pub async fn run_export(
    ctx: &SessionContext,
    snapshot: SnapshotId,
    a: ExportArgs,
) -> Result<(String, usize), String> {
    let snap = snapshot_name(snapshot);
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
    // `PARTITIONED BY` takes **bare** identifiers, and quoting is not an option:
    // DataFusion 54's COPY parser re-renders each one with `Ident::to_string()`, so a
    // quoted name reaches the planner with its quotes still attached and matches no
    // field. Bare is also case-preserving here (that parser doesn't normalise), so every
    // name the tokenizer reads as one word round-trips — and one it doesn't simply can't
    // be expressed, which is worth saying plainly instead of emitting a statement that
    // fails with a parser message about a stray token.
    if let Some(bad) = a.partition_cols.iter().find(|c| !is_bare_word(c)) {
        return Err(format!(
            "Can't partition by {bad:?}: COPY takes unquoted column names, so a partition \
             column has to be a single plain word"
        ));
    }
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
            // Every value here lands inside a `'…'` literal. `nv` and the bool are ours;
            // the delimiter is the user's, so it gets the same `''` doubling as the path
            // below (a `'` delimiter would otherwise close the literal early).
            let delim = a.csv_delimiter.to_string().replace('\'', "''");
            format!(
                " OPTIONS ('HAS_HEADER' '{}', 'DELIMITER' '{delim}', 'NULL_VALUE' '{}')",
                a.csv_header, nv
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

/// Whether `name` tokenises as a single **unquoted** identifier, asked of the very
/// dialect DataFusion parses with rather than a hardcoded character set (`lex` does the
/// same for the editor's word boundaries).
fn is_bare_word(name: &str) -> bool {
    use datafusion::sql::sqlparser::dialect::{Dialect, GenericDialect};
    let d = GenericDialect {};
    let mut rest = name.chars();
    matches!(rest.next(), Some(c) if d.is_identifier_start(c))
        && rest.all(|c| d.is_identifier_part(c))
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
