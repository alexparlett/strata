//! Results serialization for the grid's **Copy** (Rz4 / P2-11) — text only, bounded to the
//! current page. The source is always an Arrow [`RecordBatch`] (projected/sliced to the selected
//! cells); each format is a [`RecordBatchWriter`], so types and nesting come straight from Arrow,
//! uniformly:
//!
//! - **CSV/TSV** → `arrow-csv`'s writer.
//! - **JSON** → [`PrettyJsonWriter`]: arrow-json's `ArrayWriter` encodes, then the whole document
//!   is pretty-printed at once by `serde_json` — structurally valid by construction.
//! - **Markdown** → [`MarkdownWriter`] here, buffering rows and right-aligning numerics on `close`.
//!
//! CSV/TSV/Markdown cannot represent nesting, so nested columns flatten to compact JSON strings
//! ([`flatten_nested`]), which round-trips unlike an Arrow debug blob.
//!
//! The **views** onto a nested value read a different path: [`cell_preview_json`] is bounded at the
//! encoder rather than afterwards, because a whole-value serialization is what froze the record
//! view on a document-shaped row.
//!
//! This module produces **text**; the clipboard side effect lives with the UI, so callers hand
//! `write_batch` / `write_selection` any `io::Write` sink.

use std::io::Write;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, MapArray, RecordBatch, StringArray, UInt32Array};
use arrow::compute::take;
use arrow::datatypes::{DataType, Field, FieldRef, Schema};
use arrow::error::ArrowError;
use arrow::json::writer::{make_encoder, EncoderOptions, JsonArray};
use arrow::json::{ArrayWriter, LineDelimitedWriter};
use arrow::record_batch::RecordBatchWriter;
use arrow::util::display::{ArrayFormatter, FormatOptions};
use serde_json::{from_slice, to_string, to_string_pretty, to_writer_pretty, Value};

use strata_core::util::{clip, fmt_int, plural, plural_noun, DISPLAY_CHARS};

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
/// formats the *entire* buffered document in one pass with `serde_json`'s pretty printer, writing to
/// the sink `W`. Because a complete, valid document is parsed and re-serialized as a whole (rather
/// than rewritten byte-by-byte), the output is always structurally valid and fully indented,
/// nested interiors included. Slots into [`drive`] like the CSV / Markdown writers.
struct PrettyJsonWriter<W: Write> {
    sink: W,
    buf: ArrayWriter<Vec<u8>>,
}

impl<W: Write> PrettyJsonWriter<W> {
    fn new(sink: W) -> Self {
        Self {
            sink,
            buf: ArrayWriter::new(Vec::new()),
        }
    }
}

impl<W: Write> RecordBatchWriter for PrettyJsonWriter<W> {
    fn write(&mut self, batch: &RecordBatch) -> Result<(), ArrowError> {
        self.buf.write(batch)
    }

    fn close(self) -> Result<(), ArrowError> {
        let PrettyJsonWriter { sink, mut buf } = self;
        buf.finish()?;
        let bytes = buf.into_inner();
        let value: Value =
            from_slice(&bytes).map_err(|e| ArrowError::ExternalError(Box::new(e)))?;
        to_writer_pretty(sink, &value).map_err(|e| ArrowError::ExternalError(Box::new(e)))
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
        TextFormat::Json => drive(PrettyJsonWriter::new(w), batch),
        TextFormat::Tsv | TextFormat::Csv => {
            let flat = flatten_nested(batch)?;
            let delim = if fmt == TextFormat::Tsv { b'\t' } else { b',' };
            let wr = arrow::csv::WriterBuilder::new()
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

/// Serialize a **sub-selection** of `batch` — the `cols` columns, at the given `rows`
/// (batch-row indices) — to `w` in `fmt`. The arrow projection + row `take` live here,
/// next to the rest of the batch→text machinery, so callers (the clipboard copy actions)
/// only decide *which* rows and columns, not how to slice a `RecordBatch`.
pub fn write_selection<W: Write>(
    fmt: TextFormat,
    batch: &RecordBatch,
    rows: &[u32],
    cols: &[usize],
    header: bool,
    w: W,
) -> Result<(), ArrowError> {
    let projected = batch.project(cols)?;
    let indices = UInt32Array::from(rows.to_vec());
    let taken: Vec<ArrayRef> = projected
        .columns()
        .iter()
        .map(|c| take(&**c, &indices, None))
        .collect::<Result<_, _>>()?;
    let sub = RecordBatch::try_new(projected.schema(), taken)?;
    write_batch(fmt, &sub, header, w)
}

/// Write one batch through a `RecordBatchWriter` and finalize it.
fn drive<Wr: RecordBatchWriter>(mut wr: Wr, batch: &RecordBatch) -> Result<(), ArrowError> {
    wr.write(batch)?;
    wr.close()
}

/// Pretty JSON a preview may produce before it stops expanding. Sized for the surfaces that
/// read it — the record view's 190px field block and the nested-cell modal's card — with room
/// to scroll, not for the value's true size.
const PREVIEW_BYTES: usize = 16384;
/// Entries shown in a **top-level** container before the `… N more keys` tail; see [`items_at`],
/// which halves it per level. A cap per *container* rather than only a budget for the whole
/// render: without it one wide object consumes the entire budget on its own and the level is
/// abandoned for a shallower one — which is how a 19,311-key object rendered a 62MB document as
/// two lines.
const PREVIEW_ITEMS: usize = 30;
/// The floor that cap decays to. Never zero: **landing on content is the point.** A boundary that
/// renders `{ … 20 keys … }` throws away the level a reader is looking for — IntelliJ collapses a
/// *value* and still lists its parent's keys, so a preview must too.
const PREVIEW_ITEMS_MIN: usize = 3;

/// Entries to show in a container at `depth`: [`PREVIEW_ITEMS`] halved per level, down to
/// [`PREVIEW_ITEMS_MIN`]. Wide at the top, narrow at the bottom — a fixed cap either wastes the
/// budget on one deep branch or shows two entries at the level you actually scan.
fn items_at(depth: usize) -> usize {
    (PREVIEW_ITEMS >> depth.min(usize::BITS as usize - 1)).max(PREVIEW_ITEMS_MIN)
}
/// Levels a preview expands. **Fixed, not maximised.** Chasing the deepest level that fits the
/// budget is what sends a preview five levels down one branch of a wide document — the reader
/// wants the top of the shape, broadly, with what is below it counted. Three levels is
/// "the value, its entries, and a glimpse inside each entry".
const PREVIEW_DEPTH: usize = 3;

/// A **bounded** pretty-JSON view of one cell's value (column `col`, row `row` of `batch`) — the
/// record view's nested (`struct`/`list`/`map`) blocks and the nested-cell modal.
///
/// The bound is the point. A row of the reference fixture holds 241,425 nested fields across 19
/// struct columns; serializing one whole took a second or two, and the surfaces reading it show
/// about ten lines. So this **never materializes the value**: it walks the Arrow arrays to
/// [`PREVIEW_DEPTH`] levels, **samples** each container's entries ([`items_at`]) and counts the
/// rest, and collapses whatever hangs below —
///
/// ```text
/// {
///   "contentBlocks": {
///     "0004d823-2c30-42b6-b28d-4a960fc2f03c": {
///       "content": { … 2 keys … },
///       "name": "lozenge - exclusive to you"
///     },
///     … 19296 more keys
///   }
/// }
/// ```
///
/// The shape of that output is the whole design, and two earlier versions got it wrong. It
/// collapses the **value**, never its parent — `"contentBlocks": { … 19311 keys … }` discards the
/// level the reader is on — which is why entries are sampled rather than the container counted.
/// And the depth is **fixed**: chasing the deepest level that fits the budget walks five levels
/// down one branch of a wide document. Cost is the budget, not the document.
///
/// Leaves go through arrow-json's own [`make_encoder`], so a number or timestamp reads exactly as
/// the copy path renders it; strings and binaries are clipped first. Nulls are **explicit**, since
/// a field that is missing and a field that is null are different facts.
///
/// `None` when the cell is null or out of range, or holds a type arrow-json cannot encode (a union,
/// which the engine renders as text upstream): the caller shows the display cell's text instead.
/// The complete value stays reachable through the grid's Copy as JSON, which is not a render path.
pub fn cell_preview_json(batch: &RecordBatch, col: usize, row: usize) -> Option<String> {
    let field = batch.schema_ref().fields().get(col)?.clone();
    let array = batch.columns().get(col)?;
    (row < array.len() && !array.is_null(row))
        .then(|| preview_json(&field, array.as_ref(), row, PREVIEW_BYTES))
        .flatten()
}

/// Render `array[idx]` at [`PREVIEW_DEPTH`] levels, within `budget` bytes.
///
/// The depth is **fixed, and the budget is a backstop** — not a target to fill. An earlier version
/// searched for the deepest level that fit, which is exactly the wrong instinct: on a wide document
/// the deepest *uniform* level that fits the budget is a narrow one, so the preview walked five
/// levels down one branch and never showed the second key. Shallower fallbacks exist only for the
/// value that is too wide even at three levels; depth 0 ignores the budget, so there is always
/// something to show.
fn preview_json(field: &FieldRef, array: &dyn Array, idx: usize, budget: usize) -> Option<String> {
    for max_depth in (0..=PREVIEW_DEPTH).rev() {
        let mut p = Preview {
            out: String::new(),
            budget: if max_depth == 0 { usize::MAX } else { budget },
            max_depth,
        };
        match p.value(field, array, idx, 0) {
            Ok(()) => return Some(p.out),
            Err(Halt::Budget) => {}
            Err(Halt::Unsupported) => return None,
        }
    }
    None
}

/// Why a [`Preview`] render stopped. `Budget` discards the *attempt* (a shallower one stands);
/// `Unsupported` discards the whole preview.
enum Halt {
    Budget,
    Unsupported,
}

/// One expansion attempt: pretty JSON built into `out`, expanding nested containers to at most
/// `max_depth` levels, abandoned as soon as `out` passes `budget`.
struct Preview {
    out: String,
    budget: usize,
    max_depth: usize,
}

impl Preview {
    /// Whether what has been written so far still fits. Checked after every write, so an
    /// attempt costs at most the budget however large the value is.
    fn fits(&self) -> Result<(), Halt> {
        (self.out.len() <= self.budget)
            .then_some(())
            .ok_or(Halt::Budget)
    }

    fn push(&mut self, s: &str) -> Result<(), Halt> {
        self.out.push_str(s);
        self.fits()
    }

    /// A newline plus `depth` levels of the two-space indent `serde_json`'s pretty printer uses,
    /// so a fully expanded preview is byte-identical to [`row_pretty_json`]'s formatting.
    fn indent(&mut self, depth: usize) -> Result<(), Halt> {
        self.out.push('\n');
        for _ in 0..depth {
            self.out.push_str("  ");
        }
        self.fits()
    }

    /// A container rendered as its size: `{ … 12 keys … }` / `[ … 5171 items … ]`. The collapsed
    /// form — what a fold line or a tree row shows for a child it has not opened.
    fn count(&mut self, open: char, n: usize, unit: &str, close: char) -> Result<(), Halt> {
        self.push(&format!("{open} … {} … {close}", plural(n, unit)))
    }

    /// The tail of a container whose entries were cut off at [`items_at`]: `… 19,296 more keys`.
    ///
    /// Rendering *some* entries and saying how many are left is the whole difference between a
    /// preview and a dead end. Without the per-container cap a single wide key blows the budget on
    /// its own, the level is abandoned for a shallower one, and a 19,311-key object collapses a
    /// 62MB document to two useless lines.
    fn more(&mut self, left: usize, unit: &str, depth: usize) -> Result<(), Halt> {
        self.push(",")?;
        self.indent(depth)?;
        self.push(&format!(
            "… {} more {}",
            fmt_int(left as u64),
            plural_noun(left, unit)
        ))
    }

    fn value(
        &mut self,
        field: &FieldRef,
        array: &dyn Array,
        idx: usize,
        depth: usize,
    ) -> Result<(), Halt> {
        if array.is_null(idx) {
            return self.push("null");
        }
        match array.data_type() {
            DataType::Struct(fields) => {
                if fields.is_empty() {
                    return self.push("{}");
                }
                if depth == self.max_depth {
                    return self.count('{', fields.len(), "key", '}');
                }
                let columns = array.as_struct().columns();
                let shown = fields.len().min(items_at(depth));
                self.push("{")?;
                for (i, (f, child)) in fields.iter().zip(columns).take(shown).enumerate() {
                    if i > 0 {
                        self.push(",")?;
                    }
                    self.indent(depth + 1)?;
                    self.push(&json_string(f.name())?)?;
                    self.push(": ")?;
                    self.value(f, child.as_ref(), idx, depth + 1)?;
                }
                if fields.len() > shown {
                    self.more(fields.len() - shown, "key", depth + 1)?;
                }
                self.indent(depth)?;
                self.push("}")
            }
            DataType::List(f) => self.items(f, &array.as_list::<i32>().value(idx), depth),
            DataType::LargeList(f) => self.items(f, &array.as_list::<i64>().value(idx), depth),
            DataType::ListView(f) => self.items(f, &array.as_list_view::<i32>().value(idx), depth),
            DataType::LargeListView(f) => {
                self.items(f, &array.as_list_view::<i64>().value(idx), depth)
            }
            DataType::FixedSizeList(f, _) => {
                self.items(f, &array.as_fixed_size_list().value(idx), depth)
            }
            DataType::Map(entries, _) => self.map(entries, array.as_map(), idx, depth),
            _ => self.leaf(field, array, idx),
        }
    }

    fn items(&mut self, field: &FieldRef, values: &ArrayRef, depth: usize) -> Result<(), Halt> {
        if values.is_empty() {
            return self.push("[]");
        }
        if depth == self.max_depth {
            return self.count('[', values.len(), "item", ']');
        }
        let shown = values.len().min(items_at(depth));
        self.push("[")?;
        for i in 0..shown {
            if i > 0 {
                self.push(",")?;
            }
            self.indent(depth + 1)?;
            self.value(field, values.as_ref(), i, depth + 1)?;
        }
        if values.len() > shown {
            self.more(values.len() - shown, "item", depth + 1)?;
        }
        self.indent(depth)?;
        self.push("]")
    }

    /// A map renders as a JSON object, which is why arrow-json requires UTF-8 keys — the same
    /// rule, refused the same way, so a preview and a copy agree on what is encodable.
    fn map(
        &mut self,
        entries: &FieldRef,
        array: &MapArray,
        idx: usize,
        depth: usize,
    ) -> Result<(), Halt> {
        let DataType::Struct(kv) = entries.data_type() else {
            return Err(Halt::Unsupported);
        };
        let (Some(key_field), Some(value_field)) = (kv.first(), kv.get(1)) else {
            return Err(Halt::Unsupported);
        };
        if !matches!(
            key_field.data_type(),
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
        ) {
            return Err(Halt::Unsupported);
        }
        let pairs = array.value(idx);
        if pairs.is_empty() {
            return self.push("{}");
        }
        if depth == self.max_depth {
            return self.count('{', pairs.len(), "key", '}');
        }
        let (keys, values) = (pairs.column(0), pairs.column(1));
        let shown = pairs.len().min(items_at(depth));
        self.push("{")?;
        for i in 0..shown {
            if i > 0 {
                self.push(",")?;
            }
            self.indent(depth + 1)?;
            let key = utf8_value(keys.as_ref(), i).ok_or(Halt::Unsupported)?;
            self.push(&json_string(key)?)?;
            self.push(": ")?;
            self.value(value_field, values.as_ref(), i, depth + 1)?;
        }
        if pairs.len() > shown {
            self.more(pairs.len() - shown, "key", depth + 1)?;
        }
        self.indent(depth)?;
        self.push("}")
    }

    /// A scalar. Strings and binaries are clipped *before* encoding — they are the only leaf
    /// that can be arbitrarily large, so encoding one to measure it would be the very cost the
    /// budget exists to avoid. Everything else goes through arrow-json's own encoder, which is
    /// what keeps a decimal or a timestamp reading exactly as the copy path renders it.
    fn leaf(&mut self, field: &FieldRef, array: &dyn Array, idx: usize) -> Result<(), Halt> {
        if let Some(text) = utf8_value(array, idx) {
            return self.push(&json_string(text)?);
        }
        if let Some(bytes) = binary_value(array, idx) {
            return self.push(&hex_string(bytes, DISPLAY_CHARS));
        }
        let options = EncoderOptions::default();
        let mut encoder = make_encoder(field, array, &options).map_err(|_| Halt::Unsupported)?;
        if encoder.is_null(idx) {
            return self.push("null");
        }
        let mut buf = Vec::new();
        encoder.encode(idx, &mut buf);
        let text = String::from_utf8(buf).map_err(|_| Halt::Unsupported)?;
        self.push(&text)
    }
}

/// A JSON string literal, clipped to [`DISPLAY_CHARS`] with a trailing `…` inside the
/// quotes. Field names and map keys go through it too — a key that long is a value in disguise.
fn json_string(s: &str) -> Result<String, Halt> {
    to_string(clip(s, DISPLAY_CHARS).as_ref()).map_err(|_| Halt::Unsupported)
}

/// Binary as arrow-json writes it — lowercase hex in a string — clipped to `max` characters.
fn hex_string(bytes: &[u8], max: usize) -> String {
    let kept = (max / 2).min(bytes.len());
    let mut out = String::with_capacity(kept * 2 + 3);
    out.push('"');
    for byte in &bytes[..kept] {
        out.push_str(&format!("{byte:02x}"));
    }
    if kept < bytes.len() {
        out.push('…');
    }
    out.push('"');
    out
}

/// The value at `idx` as a `&str`, for the three UTF-8 array layouts. `None` for anything else.
fn utf8_value(array: &dyn Array, idx: usize) -> Option<&str> {
    match array.data_type() {
        DataType::Utf8 => Some(array.as_string::<i32>().value(idx)),
        DataType::LargeUtf8 => Some(array.as_string::<i64>().value(idx)),
        DataType::Utf8View => Some(array.as_string_view().value(idx)),
        _ => None,
    }
}

/// The value at `idx` as bytes, for the four binary array layouts. `None` for anything else.
fn binary_value(array: &dyn Array, idx: usize) -> Option<&[u8]> {
    match array.data_type() {
        DataType::Binary => Some(array.as_binary::<i32>().value(idx)),
        DataType::LargeBinary => Some(array.as_binary::<i64>().value(idx)),
        DataType::BinaryView => Some(array.as_binary_view().value(idx)),
        DataType::FixedSizeBinary(_) => Some(array.as_fixed_size_binary().value(idx)),
        _ => None,
    }
}

/// Pretty-print one whole row of `batch` as a **bare `{column: value}` object** — the record
/// view's "Copy row as JSON" (the canvas `buildRowJSON` shape: a single object, not
/// [`write_batch`]'s array-of-objects). Nulls are explicit (`"col": null` — every column is
/// present), nested values stay real JSON, and field order follows the schema (`preserve_order`).
/// `None` only on a serialization failure — a null *row* still yields a full object.
pub fn row_pretty_json(batch: &RecordBatch, row: usize) -> Option<String> {
    let one = batch.slice(row, 1);
    let mut buf = Vec::new();
    {
        let mut w = arrow::json::WriterBuilder::new()
            .with_explicit_nulls(true)
            .build::<_, JsonArray>(&mut buf);
        w.write(&one).ok()?;
        w.finish().ok()?;
    }
    let arr: Value = from_slice(&buf).ok()?;
    to_string_pretty(arr.get(0)?).ok()
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
            self.header = schema
                .fields()
                .iter()
                .map(|f| md_escape(f.name()))
                .collect();
            self.right = schema
                .fields()
                .iter()
                .map(|f| is_numeric(f.data_type()))
                .collect();
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
        for (i, cell) in header.iter().enumerate().take(ncol) {
            out.push_str(&format!(" {} |", pad(cell, i)));
        }
        out.push('\n');
        out.push('|');
        for i in 0..ncol {
            let rule = if right[i] {
                format!("{}:", "-".repeat(width[i].saturating_sub(1)))
            } else {
                "-".repeat(width[i])
            };
            out.push_str(&format!(" {rule} |"));
        }
        out.push('\n');
        for row in &rows {
            out.push('|');
            for i in 0..ncol {
                let c = row.get(i).map(String::as_str).unwrap_or("");
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
    let mut buf = Vec::new();
    {
        let mut jw = LineDelimitedWriter::new(&mut buf);
        jw.write(batch)?;
        jw.finish()?;
    }
    let rows: Vec<Value> = buf
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| from_slice(l).unwrap_or(Value::Null))
        .collect();

    let mut cols: Vec<ArrayRef> = Vec::with_capacity(schema.fields().len());
    let mut fields: Vec<Field> = Vec::with_capacity(schema.fields().len());
    for (ci, field) in schema.fields().iter().enumerate() {
        if nested[ci] {
            let name = field.name().as_str();
            let strs: Vec<Option<String>> = rows
                .iter()
                .map(|obj| match obj.get(name) {
                    Some(v) if !v.is_null() => Some(to_string(v).unwrap_or_default()),
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

#[cfg(test)]
mod tests {
    use arrow::array::{Int32Array, ListArray, NullArray, StructArray};
    use arrow::buffer::OffsetBuffer;
    use arrow::datatypes::Fields;

    use super::*;

    /// A 2-row batch with one struct column, the second row null — the nested-cell view's
    /// exact read shape.
    fn nested_batch() -> RecordBatch {
        let fields = Fields::from(vec![
            Field::new("plan", DataType::Utf8, false),
            Field::new("seats", DataType::Int32, false),
        ]);
        let strct = StructArray::new(
            fields.clone(),
            vec![
                Arc::new(StringArray::from(vec!["pro", "free"])) as ArrayRef,
                Arc::new(Int32Array::from(vec![12, 1])) as ArrayRef,
            ],
            Some(vec![true, false].into()),
        );
        let schema = Schema::new(vec![Field::new("attrs", DataType::Struct(fields), true)]);
        RecordBatch::try_new(Arc::new(schema), vec![Arc::new(strct)]).unwrap()
    }

    #[test]
    fn cell_preview_json_indents_one_nested_value() {
        let json = cell_preview_json(&nested_batch(), 0, 0).expect("non-null cell");
        assert_eq!(json, "{\n  \"plan\": \"pro\",\n  \"seats\": 12\n}");
    }

    #[test]
    fn cell_preview_json_is_none_for_a_null_value() {
        assert!(cell_preview_json(&nested_batch(), 0, 1).is_none());
    }

    #[test]
    fn cell_preview_json_is_none_off_the_end_of_the_batch() {
        assert!(cell_preview_json(&nested_batch(), 0, 9).is_none());
        assert!(cell_preview_json(&nested_batch(), 9, 0).is_none());
    }

    /// A null field of a *rendered* struct stays visible — a preview is for reading, and
    /// arrow-json's eliding default would show a null field as an absent one.
    #[test]
    fn a_null_field_reads_as_null_rather_than_being_dropped() {
        let fields = Fields::from(vec![
            Field::new("plan", DataType::Utf8, true),
            Field::new("seats", DataType::Int32, true),
        ]);
        let strct = StructArray::new(
            fields.clone(),
            vec![
                Arc::new(StringArray::from(vec![None::<&str>])) as ArrayRef,
                Arc::new(Int32Array::from(vec![12])) as ArrayRef,
            ],
            None,
        );
        let schema = Schema::new(vec![Field::new("attrs", DataType::Struct(fields), true)]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(strct)]).unwrap();
        assert_eq!(
            cell_preview_json(&batch, 0, 0).expect("non-null cell"),
            "{\n  \"plan\": null,\n  \"seats\": 12\n}"
        );
    }

    /// One struct column whose single field is a long list — the shape a document row has, and
    /// the reason the budget expands level by level.
    fn document_batch(items: usize) -> RecordBatch {
        let item = Arc::new(Field::new("item", DataType::Int32, true));
        let values = Int32Array::from((0..items as i32).collect::<Vec<_>>());
        let offsets = OffsetBuffer::new(vec![0, items as i32].into());
        let list = ListArray::new(item.clone(), offsets, Arc::new(values), None);
        let fields = Fields::from(vec![Field::new("nbas", DataType::List(item), true)]);
        let strct = StructArray::new(fields.clone(), vec![Arc::new(list) as ArrayRef], None);
        let schema = Schema::new(vec![Field::new("config", DataType::Struct(fields), true)]);
        RecordBatch::try_new(Arc::new(schema), vec![Arc::new(strct)]).unwrap()
    }

    fn preview_of(batch: &RecordBatch, budget: usize) -> String {
        let field = batch.schema_ref().fields()[0].clone();
        preview_json(&field, batch.column(0).as_ref(), 0, budget).expect("a preview")
    }

    /// The acceptance's summary: the outer structure survives, and an oversized container shows
    /// its first entries plus how many are left — not a mid-token truncation, not the outer object
    /// collapsing, and not a bare count where there was room for content.
    #[test]
    fn an_oversized_container_shows_entries_then_counts_the_rest() {
        let shown = items_at(1);
        let json = preview_of(&document_batch(5171), PREVIEW_BYTES);
        assert!(
            json.starts_with("{\n  \"nbas\": [\n    0,\n    1,"),
            "{json}"
        );
        assert!(
            json.ends_with(&format!(
                "    {},\n    … {} more items\n  ]\n}}",
                shown - 1,
                fmt_int((5171 - shown) as u64)
            )),
            "{json}"
        );
    }

    /// The regression that prompted the per-container cap. One 19,311-key object under a single
    /// top-level key: expanding it blew the budget, so the level was abandoned for the shallower
    /// one above it — rendering a 62MB document as
    /// `{ "contentBlocks": { … 19311 keys … } }`, two lines and no way in. The cap means the
    /// budget is spent on entries instead.
    #[test]
    fn one_very_wide_key_does_not_collapse_the_whole_document() {
        let wide = Fields::from(
            (0..19_311)
                .map(|i| Field::new(format!("block{i}"), DataType::Int32, true))
                .collect::<Vec<_>>(),
        );
        let columns = (0..19_311)
            .map(|_| Arc::new(Int32Array::from(vec![1])) as ArrayRef)
            .collect::<Vec<_>>();
        let inner = StructArray::new(wide.clone(), columns, None);
        let outer_fields = Fields::from(vec![Field::new(
            "contentBlocks",
            DataType::Struct(wide),
            true,
        )]);
        let outer = StructArray::new(
            outer_fields.clone(),
            vec![Arc::new(inner) as ArrayRef],
            None,
        );
        let schema = Schema::new(vec![Field::new(
            "config",
            DataType::Struct(outer_fields),
            true,
        )]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(outer)]).unwrap();

        let json = cell_preview_json(&batch, 0, 0).expect("non-null cell");
        assert!(
            json.contains("\"block0\": 1"),
            "the wide object's entries must be shown: {json}"
        );
        assert!(
            json.contains(&format!(
                "… {} more keys",
                fmt_int((19_311 - items_at(1)) as u64)
            )),
            "and the remainder counted: {json}"
        );
        assert!(
            json.lines().count() > items_at(1),
            "two lines is the bug this test exists for: {json}"
        );
    }

    /// The floor: a budget too small even for the top-level count still yields the count, not
    /// nothing — the caller's only alternative is the display cell's flattened text.
    #[test]
    fn a_budget_below_the_floor_still_yields_the_top_level_count() {
        assert_eq!(preview_of(&document_batch(5171), 10), "{ … 1 key … }");
    }

    /// A budget with room to spare renders the whole value, elision wording included nowhere.
    #[test]
    fn a_value_that_fits_is_rendered_whole() {
        assert_eq!(
            preview_of(&document_batch(3), PREVIEW_BYTES),
            "{\n  \"nbas\": [\n    0,\n    1,\n    2\n  ]\n}"
        );
    }

    /// Above the floor the chosen render is always within budget — that is the whole claim the
    /// surfaces rest on, and the reason a 62MB value costs the same as a small one.
    #[test]
    fn no_budget_is_ever_exceeded() {
        for budget in [64, 128, 256, 512, 1024, PREVIEW_BYTES] {
            let json = preview_of(&document_batch(5171), budget);
            assert!(
                json.len() <= budget,
                "budget {budget} produced {} bytes: {json}",
                json.len()
            );
        }
    }

    /// Empty containers read as themselves rather than as a count of nothing.
    #[test]
    fn empty_containers_render_empty() {
        assert_eq!(
            preview_of(&document_batch(0), PREVIEW_BYTES),
            "{\n  \"nbas\": []\n}"
        );
    }

    /// A string leaf is the one scalar that can be arbitrarily large, so it clips — with the
    /// same `…` the grid's display cells use, and still as a valid JSON string.
    #[test]
    fn a_long_string_leaf_is_clipped_inside_its_quotes() {
        let long = "x".repeat(DISPLAY_CHARS + 50);
        let fields = Fields::from(vec![Field::new("blob", DataType::Utf8, true)]);
        let strct = StructArray::new(
            fields.clone(),
            vec![Arc::new(StringArray::from(vec![long.as_str()])) as ArrayRef],
            None,
        );
        let schema = Schema::new(vec![Field::new("attrs", DataType::Struct(fields), true)]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(strct)]).unwrap();
        let json = cell_preview_json(&batch, 0, 0).expect("non-null cell");
        let expected = format!("{{\n  \"blob\": \"{}…\"\n}}", "x".repeat(DISPLAY_CHARS));
        assert_eq!(json, expected);
    }

    /// An all-null field infers as `DataType::Null`, whose nulls are logical — found on the real
    /// `config.json`, where it panicked inside arrow-json's `NullEncoder`.
    #[test]
    fn an_all_null_typed_field_reads_as_null() {
        let fields = Fields::from(vec![
            Field::new("gone", DataType::Null, true),
            Field::new("seats", DataType::Int32, true),
        ]);
        let strct = StructArray::new(
            fields.clone(),
            vec![
                Arc::new(NullArray::new(1)) as ArrayRef,
                Arc::new(Int32Array::from(vec![12])) as ArrayRef,
            ],
            None,
        );
        let schema = Schema::new(vec![Field::new("attrs", DataType::Struct(fields), true)]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(strct)]).unwrap();
        assert_eq!(
            cell_preview_json(&batch, 0, 0).expect("non-null cell"),
            "{\n  \"gone\": null,\n  \"seats\": 12\n}"
        );
    }

    /// A map renders as an object, and refuses non-UTF-8 keys exactly as arrow-json does — so a
    /// preview and a copy agree on what is encodable.
    #[test]
    fn a_map_renders_as_an_object() {
        let keys = Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef;
        let vals = Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef;
        let kv = Fields::from(vec![
            Field::new("keys", DataType::Utf8, false),
            Field::new("values", DataType::Int32, true),
        ]);
        let entries = StructArray::new(kv.clone(), vec![keys, vals], None);
        let entries_field = Arc::new(Field::new("entries", DataType::Struct(kv), false));
        let map = MapArray::new(
            entries_field.clone(),
            OffsetBuffer::new(vec![0, 2].into()),
            entries,
            None,
            false,
        );
        let schema = Schema::new(vec![Field::new(
            "tags",
            DataType::Map(entries_field, false),
            true,
        )]);
        let batch = RecordBatch::try_new(Arc::new(schema), vec![Arc::new(map)]).unwrap();
        assert_eq!(
            cell_preview_json(&batch, 0, 0).expect("non-null cell"),
            "{\n  \"a\": 1,\n  \"b\": 2\n}"
        );
    }

    /// A 2-row batch with a scalar + a nested column — the record view's copy shape.
    fn row_batch() -> RecordBatch {
        let fields = Fields::from(vec![Field::new("plan", DataType::Utf8, false)]);
        let strct = StructArray::new(
            fields.clone(),
            vec![Arc::new(StringArray::from(vec!["pro", "free"])) as ArrayRef],
            Some(vec![true, false].into()),
        );
        let schema = Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("attrs", DataType::Struct(fields), true),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(Int32Array::from(vec![1, 2])), Arc::new(strct)],
        )
        .unwrap()
    }

    #[test]
    fn row_pretty_json_is_a_bare_object_in_schema_order() {
        let json = row_pretty_json(&row_batch(), 0).expect("row 0 serializes");
        assert_eq!(
            json,
            "{\n  \"id\": 1,\n  \"attrs\": {\n    \"plan\": \"pro\"\n  }\n}"
        );
    }

    #[test]
    fn row_pretty_json_keeps_null_columns_explicit() {
        let json = row_pretty_json(&row_batch(), 1).expect("row 1 serializes");
        assert_eq!(json, "{\n  \"id\": 2,\n  \"attrs\": null\n}");
    }
}
