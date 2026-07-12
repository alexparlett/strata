//! Results serialization for the grid's **Copy** (Rz4) — clipboard only, bounded to the current
//! page. The source is always an Arrow [`RecordBatch`] (projected/sliced to the selected cells);
//! each format is a [`RecordBatchWriter`], so types and nesting come straight from Arrow,
//! uniformly:
//!
//! - **CSV/TSV** → `arrow-csv`'s writer.
//! - **JSON** → [`PrettyJsonWriter`]: arrow-json's `ArrayWriter` encodes (nested
//!   `struct`/`list`/`map` stay real JSON), then the whole document is pretty-printed at once by
//!   serde_json — fully indented, structurally valid by construction.
//! - **Markdown** → [`MarkdownWriter`] here (buffers rows, pads + right-aligns numerics on
//!   `close`), same trait as the others.
//!
//! CSV/TSV/Markdown can't represent nesting, so nested columns are first flattened to compact
//! JSON strings ([`flatten_nested`]) — which round-trips, unlike an Arrow debug blob.
//!
//! [`ClipboardWriter`] is the `Write` **sink** the format writer targets, committing its bytes
//! to the system clipboard on [`ClipboardWriter::commit`].

use std::io::Write;
use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, RecordBatch, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::error::ArrowError;
use datafusion::arrow::record_batch::RecordBatchWriter;
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};

/// Clipboard / text serialization format.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextFormat {
    /// Tab-separated (default Copy / ⌘C — Excel-paste-friendly).
    Tsv,
    Csv,
    Json,
    Markdown,
}

/// A pretty-printing JSON [`RecordBatchWriter`]. It reuses arrow-json's `ArrayWriter` for **all**
/// encoding — types, nesting, decimals — into an in-memory buffer, then on [`close`](Self::close)
/// formats the *entire* buffered document in one pass with serde_json's pretty printer, writing to
/// the sink `W`. Because a complete, valid document is parsed and re-serialized as a whole (rather
/// than rewritten byte-by-byte), the output is always structurally valid and fully indented,
/// nested interiors included. Slots into [`drive`] like the CSV / Markdown writers.
struct PrettyJsonWriter<W: Write> {
    sink: W,
    buf: datafusion::arrow::json::ArrayWriter<Vec<u8>>,
}

impl<W: Write> PrettyJsonWriter<W> {
    fn new(sink: W) -> Self {
        Self {
            sink,
            buf: datafusion::arrow::json::ArrayWriter::new(Vec::new()),
        }
    }
}

impl<W: Write> RecordBatchWriter for PrettyJsonWriter<W> {
    fn write(&mut self, batch: &RecordBatch) -> Result<(), ArrowError> {
        self.buf.write(batch)
    }

    fn close(self) -> Result<(), ArrowError> {
        let PrettyJsonWriter { sink, mut buf } = self;
        buf.finish()?; // close the JSON array
        let bytes = buf.into_inner(); // the complete compact document
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| ArrowError::ExternalError(Box::new(e)))?;
        serde_json::to_writer_pretty(sink, &value)
            .map_err(|e| ArrowError::ExternalError(Box::new(e)))
    }
}

/// Serialize `batch` in `fmt` to `w`. `header` adds a header row for CSV/TSV (JSON keys by
/// name and Markdown always carries a header, so it's a no-op there).
pub fn write_batch<W: Write>(
    fmt: TextFormat,
    batch: &RecordBatch,
    header: bool,
    w: W,
) -> Result<(), ArrowError> {
    match fmt {
        // JSON keeps nesting — arrow-json emits real nested objects/arrays. [`PrettyJsonWriter`]
        // reuses arrow's `ArrayWriter` for the encoding and pretty-prints the whole document on
        // close; types/decimals stay exact (arrow renders them, serde_json only reformats).
        TextFormat::Json => drive(PrettyJsonWriter::new(w), batch),
        TextFormat::Tsv | TextFormat::Csv => {
            let flat = flatten_nested(batch)?;
            let delim = if fmt == TextFormat::Tsv { b'\t' } else { b',' };
            let wr = datafusion::arrow::csv::WriterBuilder::new()
                .with_delimiter(delim)
                .with_header(header)
                .build(w);
            drive(wr, &flat)
        }
        TextFormat::Markdown => {
            let flat = flatten_nested(batch)?;
            drive(MarkdownWriter::new(w), &flat)
        }
    }
}

/// Write one batch through a `RecordBatchWriter` and finalize it.
fn drive<Wr: RecordBatchWriter>(mut wr: Wr, batch: &RecordBatch) -> Result<(), ArrowError> {
    wr.write(batch)?;
    wr.close()
}

/// A `std::io::Write` sink that lands its bytes on the system clipboard (Rz4). Plug it in as
/// the writer for [`write_batch`], then [`commit`](Self::commit).
pub struct ClipboardWriter {
    buf: Vec<u8>,
}

impl ClipboardWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Push the accumulated bytes to the clipboard.
    pub fn commit(self) -> Result<(), String> {
        let text = String::from_utf8(self.buf).map_err(|e| e.to_string())?;
        arboard::Clipboard::new()
            .and_then(|mut c| c.set_text(text))
            .map_err(|e| e.to_string())
    }
}

impl Default for ClipboardWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for ClipboardWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// GitHub-flavoured markdown table writer. Buffers formatted rows across `write` calls and, on
/// `close`, emits a padded table with numeric columns right-aligned (`---:`). Alignment comes
/// from the schema `DataType`; display text from `ArrayFormatter` (same as the grid).
struct MarkdownWriter<W: Write> {
    w: W,
    header: Vec<String>,
    right: Vec<bool>,
    rows: Vec<Vec<String>>,
}

impl<W: Write> MarkdownWriter<W> {
    fn new(w: W) -> Self {
        Self {
            w,
            header: Vec::new(),
            right: Vec::new(),
            rows: Vec::new(),
        }
    }
}

impl<W: Write> RecordBatchWriter for MarkdownWriter<W> {
    fn write(&mut self, batch: &RecordBatch) -> Result<(), ArrowError> {
        let schema = batch.schema();
        if self.header.is_empty() {
            self.header = schema.fields().iter().map(|f| md_escape(f.name())).collect();
            self.right = schema.fields().iter().map(|f| is_numeric(f.data_type())).collect();
        }
        let opts = FormatOptions::default();
        let fmts = batch
            .columns()
            .iter()
            .map(|c| ArrayFormatter::try_new(&**c, &opts))
            .collect::<Result<Vec<_>, _>>()?;
        for r in 0..batch.num_rows() {
            let row = fmts
                .iter()
                .enumerate()
                .map(|(ci, f)| {
                    if batch.column(ci).is_null(r) {
                        String::new()
                    } else {
                        md_escape(&f.value(r).to_string())
                    }
                })
                .collect();
            self.rows.push(row);
        }
        Ok(())
    }

    fn close(self) -> Result<(), ArrowError> {
        let MarkdownWriter {
            mut w,
            header,
            right,
            rows,
        } = self;
        let ncol = header.len();
        let mut width = vec![3usize; ncol];
        for (i, wi) in width.iter_mut().enumerate() {
            *wi = (*wi).max(header[i].chars().count());
            for row in &rows {
                if let Some(c) = row.get(i) {
                    *wi = (*wi).max(c.chars().count());
                }
            }
        }
        let pad = |s: &str, i: usize| -> String {
            if right[i] {
                format!("{:>w$}", s, w = width[i])
            } else {
                format!("{:<w$}", s, w = width[i])
            }
        };
        let mut out = String::new();
        out.push('|');
        for i in 0..ncol {
            out.push_str(&format!(" {} |", pad(&header[i], i)));
        }
        out.push('\n');
        out.push('|');
        for i in 0..ncol {
            let rule = if right[i] {
                format!("{}:", "-".repeat(width[i].saturating_sub(1)))
            } else {
                "-".repeat(width[i])
            };
            out.push_str(&format!(" {} |", rule));
        }
        out.push('\n');
        for row in &rows {
            out.push('|');
            for i in 0..ncol {
                let c = row.get(i).map(|s| s.as_str()).unwrap_or("");
                out.push_str(&format!(" {} |", pad(c, i)));
            }
            out.push('\n');
        }
        w.write_all(out.as_bytes())
         .map_err(|e| ArrowError::ExternalError(Box::new(e)))
    }
}

/// Replace nested (`struct`/`list`/`map`/…) columns with `Utf8` columns of compact JSON, so
/// the CSV/TSV/Markdown writers (which can't nest) round-trip them. Scalar columns are left
/// as-is. A single arrow-json pass yields the per-cell values.
fn flatten_nested(batch: &RecordBatch) -> Result<RecordBatch, ArrowError> {
    let schema = batch.schema();
    let nested: Vec<bool> = schema
        .fields()
        .iter()
        .map(|f| is_nested(f.data_type()))
        .collect();
    if !nested.iter().any(|&n| n) {
        return Ok(batch.clone());
    }
    // One ndjson pass gives every cell's type-aware JSON value.
    let mut buf = Vec::new();
    {
        let mut jw = datafusion::arrow::json::LineDelimitedWriter::new(&mut buf);
        jw.write(batch)?;
        jw.finish()?;
    }
    let rows: Vec<serde_json::Value> = buf
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap_or(serde_json::Value::Null))
        .collect();

    let mut cols: Vec<ArrayRef> = Vec::with_capacity(schema.fields().len());
    let mut fields: Vec<Field> = Vec::with_capacity(schema.fields().len());
    for (ci, field) in schema.fields().iter().enumerate() {
        if nested[ci] {
            let name = field.name().as_str();
            let strs: Vec<Option<String>> = rows
                .iter()
                .map(|obj| match obj.get(name) {
                    Some(v) if !v.is_null() => Some(serde_json::to_string(v).unwrap_or_default()),
                    _ => None,
                })
                .collect();
            cols.push(Arc::new(StringArray::from(strs)));
            fields.push(Field::new(field.name().clone(), DataType::Utf8, true));
        } else {
            cols.push(batch.column(ci).clone());
            fields.push(field.as_ref().clone());
        }
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols)
}

fn is_numeric(dt: &DataType) -> bool {
    use DataType::*;
    matches!(
        dt,
        Int8 | Int16
            | Int32
            | Int64
            | UInt8
            | UInt16
            | UInt32
            | UInt64
            | Float16
            | Float32
            | Float64
            | Decimal128(..)
            | Decimal256(..)
    )
}

fn is_nested(dt: &DataType) -> bool {
    use DataType::*;
    matches!(
        dt,
        Struct(_) | List(_) | LargeList(_) | FixedSizeList(..) | Map(..) | Union(..)
    )
}

/// Escape pipes / newlines so cell text can't break a markdown table cell.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}
