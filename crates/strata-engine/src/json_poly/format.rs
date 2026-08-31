//! The `FileFormat` that puts [`infer`](super::infer) and [`normalize`](super::normalize) on
//! DataFusion's read path.
//!
//! `register_external` picks a reader per [`SourceFormat`]; this is what the JSON arm selects.
//! DataFusion imposes nothing here — `FileFormat` is a public trait and `ListingOptions` /
//! `ListingTable` are generic over `Arc<dyn FileFormat>`.
//!
//! # Why the whole physical read is ours
//!
//! The swap point is *inside* `JsonOpener::open`, where `ReaderBuilder` is constructed, so none
//! of DataFusion's JSON opener can be inherited. What is deliberately **not** reimplemented is
//! its byte-range splitting: [`PolyJsonSource`] declares
//! [`supports_repartitioning`](FileSource::supports_repartitioning) `false` and each file is read
//! whole by one task. That drops `AlignedBoundaryStream` and its half of the opener, and it costs
//! nothing that matters — every record goes through serde either way, so this format was never
//! going to be the fast path. Correctness over a parallelism win we cannot spend.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::json::reader::Decoder;
use datafusion::arrow::json::ReaderBuilder;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::Session;
use datafusion::common::parsers::CompressionTypeVariant;
use datafusion::common::{not_impl_err, DataFusionError, GetExt, Result, Statistics};
use datafusion::datasource::file_format::file_compression_type::FileCompressionType;
use datafusion::datasource::file_format::{FileFormat, FileFormatFactory};
use datafusion::datasource::listing::PartitionedFile;
use datafusion::datasource::physical_plan::{
    FileOpenFuture, FileOpener, FileScanConfig, FileScanConfigBuilder, FileSinkConfig, FileSource,
};
use datafusion::datasource::projection::{ProjectionOpener, SplitProjection};
use datafusion::datasource::source::DataSourceExec;
use datafusion::datasource::table_schema::TableSchema;
use datafusion::object_store::{ObjectMeta, ObjectStore, ObjectStoreExt};
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::LexRequirement;
use datafusion::physical_plan::metrics::ExecutionPlanMetricsSet;
use datafusion::physical_plan::{DisplayFormatType, ExecutionPlan};
use futures::{StreamExt, TryStreamExt};
use serde_json::Value;
use strata_model::{FileCompression, JsonRead, JsonShape};

use crate::formats::compression_type;

use super::infer::{absorb, kind_word, schema_of, Tree};
use super::normalize::fit_record;

/// A JSON reader that stringifies a conflicted field instead of failing schema inference.
#[derive(Debug, Clone)]
pub struct PolyJsonFormat {
    opts: JsonRead,
}

impl PolyJsonFormat {
    /// A reader taking `opts`.
    pub fn new(opts: JsonRead) -> Self {
        Self { opts }
    }

    /// The def's compression, through the formats layer's one mapping — a second copy of those
    /// five arms is how JSON and CSV end up disagreeing about a newly added variant.
    fn compression(&self) -> FileCompressionType {
        compression_type(self.opts.compression)
    }
}

/// A **lazy** iterator over the records in `bytes`.
///
/// Owns its input (`Cursor<Vec<u8>>`, not a borrowed slice) so the iterator is `'static` and can
/// live inside the opener's stream. That is what lets both callers stop early: `infer_schema`
/// honours `infer_rows` by *not parsing* past it, and the scan decodes one record at a time
/// instead of holding every `Value` of the file at once.
///
/// `NewlineDelimited` goes through serde's `StreamDeserializer`, which reads *whitespace
/// separated* values rather than strictly one per line — so it covers both NDJSON and the single
/// whole-document object that `sample/config.json` actually is (62MB on one line, one record).
/// `Array` has to parse the document to find its elements; there is no streaming form of that.
pub fn record_iter(
    bytes: Vec<u8>,
    shape: JsonShape,
) -> Result<Box<dyn Iterator<Item = Result<Value>> + Send>> {
    match shape {
        JsonShape::NewlineDelimited => Ok(Box::new(
            serde_json::Deserializer::from_reader(std::io::Cursor::new(bytes))
                .into_iter::<Value>()
                .map(|r| r.map_err(syntax_error)),
        )),
        JsonShape::Array => {
            let doc: Value = serde_json::from_slice(&bytes).map_err(syntax_error)?;
            match doc {
                Value::Array(items) => Ok(Box::new(items.into_iter().map(Ok))),
                other => Err(json_error(format!(
                    "Expected JSON record to be an object, found {}",
                    kind_word(&other)
                ))),
            }
        }
    }
}

/// Arrow's `Not valid JSON: …` wording, for the same reason [`json_error`] exists: a syntax
/// failure is routed by `json_shape_error`, whose "a record does not end on its line" arm keys
/// off `EOF while parsing` — which is also serde's wording for a truncated record.
fn syntax_error(e: serde_json::Error) -> DataFusionError {
    json_error(format!("Not valid JSON: {e}"))
}

/// A read failure in the shape `catalog::json_shape_error` routes on.
///
/// That translator finds its detail with `raw.split("Json error: ").nth(1)`, which is how arrow
/// prefixes every JSON complaint. Replacing arrow's reader must not quietly replace the
/// *diagnosis* the user gets — "Cannot read 't' as JSON: the source is a JSON array. Set the JSON
/// shape to array in Table Config" is a far better message than any raw error, and it is keyed on
/// this prefix. So we speak arrow's dialect deliberately.
fn json_error(detail: impl fmt::Display) -> DataFusionError {
    DataFusionError::Execution(format!("Json error: {detail}"))
}

/// Decompress and read one object's bytes.
async fn object_bytes(
    store: &Arc<dyn ObjectStore>,
    meta: &ObjectMeta,
    compression: FileCompressionType,
) -> Result<Vec<u8>> {
    let stream = store.get(&meta.location).await?.into_stream();
    let mut decoded = compression.convert_stream(stream.map_err(DataFusionError::from).boxed())?;
    let mut out = Vec::new();
    while let Some(chunk) = decoded.next().await {
        out.extend_from_slice(&chunk?);
    }
    Ok(out)
}

#[async_trait::async_trait]
impl FileFormat for PolyJsonFormat {
    fn get_ext(&self) -> String {
        "json".to_string()
    }

    fn get_ext_with_compression(&self, c: &FileCompressionType) -> Result<String> {
        Ok(format!("{}{}", self.get_ext(), c.get_ext()))
    }

    fn compression_type(&self) -> Option<FileCompressionType> {
        Some(self.compression())
    }

    /// Infer across every object into **one** [`Tree`], then build the schema once.
    ///
    /// Deliberately not "a `Schema` per file, folded with `Schema::try_merge`". That was the first
    /// shape and it quietly threw the whole feature away across file boundaries: arrow's
    /// `Field::try_merge` hard-errors on Struct-vs-Utf8 (and on Int64-vs-Float64), so a directory
    /// of NDJSON shards where `content` is a string on Monday and an object on Tuesday failed
    /// registration with a raw arrow message naming neither key nor file — the exact failure this
    /// module exists to remove, surviving in the multi-file case because the single-file test
    /// could not see it. `Inferred::merge` is already the correct fold; it just has to be reached.
    ///
    /// **`infer_rows` is honoured but has no default cap**, unlike DataFusion's 1000
    /// (`DEFAULT_SCHEMA_INFER_MAX_RECORD`). That default is what turns a late conflict into a
    /// *scan* failure on a table the catalog already called healthy — inference sees 1000 clean
    /// records, types the column, and the read then meets the record that disagrees. Reading
    /// everything is the honest default for a reader whose entire purpose is to notice conflicts
    /// wherever they are; a user who wants the cheaper sample can still set the option, and
    /// [`record_iter`] is lazy so that option now actually stops the parse.
    async fn infer_schema(
        &self,
        _state: &dyn Session,
        store: &Arc<dyn ObjectStore>,
        objects: &[ObjectMeta],
    ) -> Result<SchemaRef> {
        if self.opts.infer_rows == Some(0) {
            return Err(json_error(
                "Rows scanned to infer must be at least 1. Set a higher value in Table Config.",
            ));
        }

        let mut tree = Tree::new();
        let mut budget = self.opts.infer_rows;
        for meta in objects {
            if budget == Some(0) {
                break;
            }
            let bytes = object_bytes(store, meta, self.compression()).await?;
            for rec in record_iter(bytes, self.opts.shape)? {
                absorb(&mut tree, &rec?).map_err(json_error)?;
                if let Some(n) = budget.as_mut() {
                    *n -= 1;
                    if *n == 0 {
                        break;
                    }
                }
            }
        }
        Ok(Arc::new(schema_of(&tree)))
    }

    async fn infer_stats(
        &self,
        _state: &dyn Session,
        _store: &Arc<dyn ObjectStore>,
        table_schema: SchemaRef,
        _object: &ObjectMeta,
    ) -> Result<Statistics> {
        Ok(Statistics::new_unknown(&table_schema))
    }

    async fn create_physical_plan(
        &self,
        _state: &dyn Session,
        conf: FileScanConfig,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let conf = FileScanConfigBuilder::from(conf)
            .with_file_compression_type(self.compression())
            .build();
        Ok(DataSourceExec::from_data_source(conf))
    }

    /// Writing is deliberately not implemented.
    ///
    /// This format exists to *read* a shape arrow cannot. There is no polymorphic JSON to write:
    /// a result set is already one type per column, so `COPY … TO 'x.json'` is the stock JSON
    /// sink's job and routing it here would only add a second way to do the same thing.
    async fn create_writer_physical_plan(
        &self,
        _input: Arc<dyn ExecutionPlan>,
        _state: &dyn Session,
        _conf: FileSinkConfig,
        _order: Option<LexRequirement>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        not_impl_err!("Writing is not supported by the polymorphic JSON reader")
    }

    fn file_source(&self, table_schema: TableSchema) -> Arc<dyn FileSource> {
        Arc::new(PolyJsonSource::new(table_schema, self.opts.shape))
    }
}

/// The scan-side configuration [`PolyJsonOpener`] needs.
#[derive(Clone)]
pub struct PolyJsonSource {
    table_schema: TableSchema,
    shape: JsonShape,
    batch_size: Option<usize>,
    metrics: ExecutionPlanMetricsSet,
    /// The column list, split into "read these from the file" and "apply this on top".
    ///
    /// Not optional, and the first attempt at this format learned why the hard way: leaving
    /// [`FileSource::projection`] at its `None` default does **not** mean "plan a projection
    /// above the scan". `FileScanConfigBuilder::build` treats a source that declines a pushdown
    /// as a bug and fails the plan outright with
    /// `FileSource json does not support projection pushdown`. A `FileSource` has to handle its
    /// own projection.
    projection: SplitProjection,
}

impl PolyJsonSource {
    /// A source reading `table_schema` out of files in `shape`.
    pub fn new(table_schema: TableSchema, shape: JsonShape) -> Self {
        Self {
            projection: SplitProjection::unprojected(&table_schema),
            table_schema,
            shape,
            batch_size: None,
            metrics: ExecutionPlanMetricsSet::new(),
        }
    }
}

impl FileSource for PolyJsonSource {
    fn create_file_opener(
        &self,
        object_store: Arc<dyn ObjectStore>,
        base_config: &FileScanConfig,
        _partition: usize,
    ) -> Result<Arc<dyn FileOpener>> {
        let file_schema = self.table_schema.file_schema();
        let projected = Arc::new(file_schema.project(&self.projection.file_indices)?);

        let opener = Arc::new(PolyJsonOpener {
            batch_size: self.batch_size.unwrap_or(8192),
            schema: projected,
            compression: base_config.file_compression_type,
            object_store,
            shape: self.shape,
        }) as Arc<dyn FileOpener>;

        ProjectionOpener::try_new(self.projection.clone(), opener, file_schema)
    }

    fn table_schema(&self) -> &TableSchema {
        &self.table_schema
    }

    fn with_batch_size(&self, batch_size: usize) -> Arc<dyn FileSource> {
        let mut s = self.clone();
        s.batch_size = Some(batch_size);
        Arc::new(s)
    }

    fn try_pushdown_projection(
        &self,
        projection: &ProjectionExprs,
    ) -> Result<Option<Arc<dyn FileSource>>> {
        let mut source = self.clone();
        let merged = self.projection.source.try_merge(projection)?;
        source.projection = SplitProjection::new(self.table_schema.file_schema(), &merged);
        Ok(Some(Arc::new(source)))
    }

    fn projection(&self) -> Option<&ProjectionExprs> {
        Some(&self.projection.source)
    }

    fn metrics(&self) -> &ExecutionPlanMetricsSet {
        &self.metrics
    }

    fn file_type(&self) -> &str {
        "json"
    }

    /// The `, key=value` fragment appended to the scan's one-line description.
    ///
    /// Nothing for `TreeRender`, which composes its own `writeln!`-separated rows — appending a
    /// leading-comma fragment there produced `, format=polymorphic-jsonfiles=1` as a single
    /// malformed line, and `datafusion.explain.format = tree` is a catalogued Engine key whose
    /// output the EXPLAIN surface parses (`engine::explain`'s `split_name_detail`). `CsvSource`
    /// declines the same way.
    fn fmt_extra(&self, t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        match t {
            DisplayFormatType::TreeRender => Ok(()),
            _ => write!(f, ", format=polymorphic-json"),
        }
    }

    /// See the module docs: each file is read whole, so there is no range splitting to align.
    fn supports_repartitioning(&self) -> bool {
        false
    }
}

/// Reads one file and yields its batches.
pub struct PolyJsonOpener {
    batch_size: usize,
    schema: SchemaRef,
    compression: FileCompressionType,
    object_store: Arc<dyn ObjectStore>,
    shape: JsonShape,
}

/// One file's decode, pulled one batch at a time.
///
/// Holds the record iterator and **one** decoder for the whole file. The first version built a
/// decoder per 1024-record chunk and collected every batch before returning `stream::iter`, which
/// was a stream in name only — `LIMIT` could not stop it, `Cancel` could not interrupt it, and
/// peak memory was the whole file as `Value`s plus the whole file as Arrow arrays.
struct Decode {
    decoder: Decoder,
    records: Box<dyn Iterator<Item = Result<Value>> + Send>,
    schema: SchemaRef,
    /// The decoder's unconsumed input. See [`Decode::next_batch`].
    pending: Vec<u8>,
}

impl Decode {
    /// The next batch, or `None` at end of file.
    ///
    /// The loop exists because **`Decoder::decode` is allowed to consume less than it is given**:
    /// it "returns once `batch_size` objects have been parsed since the last call to `flush`",
    /// and "any remaining bytes should be included in the next call". Discarding that count is
    /// silent data loss — with `datafusion.execution.batch_size` (a user-settable Engine key) at
    /// 4, a 50-row file returned 4 rows and no error. So the unconsumed tail is kept in `pending`
    /// and re-fed after the flush.
    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        loop {
            while !self.pending.is_empty() {
                let n = self.decoder.decode(&self.pending)?;
                if n == 0 {
                    break;
                }
                self.pending.drain(..n);
            }
            if !self.pending.is_empty() {
                return self.decoder.flush().map_err(Into::into);
            }
            match self.records.next() {
                Some(rec) => {
                    let mut rec = rec?;
                    fit_record(&mut rec, self.schema.fields());
                    self.pending = serde_json::to_vec(&rec)
                        .map_err(|e| DataFusionError::External(e.into()))?;
                }
                None => return self.decoder.flush().map_err(Into::into),
            }
        }
    }
}

impl FileOpener for PolyJsonOpener {
    fn open(&self, file: PartitionedFile) -> Result<FileOpenFuture> {
        if file.range.is_some() {
            return not_impl_err!(
                "The JSON reader does not support range-based file scanning. \
                 Set datafusion.optimizer.repartition_file_scans to false."
            );
        }

        let store = Arc::clone(&self.object_store);
        let schema = Arc::clone(&self.schema);
        let batch_size = self.batch_size;
        let compression = self.compression;
        let shape = self.shape;

        Ok(Box::pin(async move {
            let bytes = object_bytes(&store, &file.object_meta, compression).await?;
            let state = Decode {
                decoder: ReaderBuilder::new(Arc::clone(&schema))
                    .with_batch_size(batch_size)
                    .build_decoder()?,
                records: record_iter(bytes, shape)?,
                schema,
                pending: Vec::new(),
            };
            Ok(futures::stream::try_unfold(state, |mut st| async move {
                tokio::task::yield_now().await;
                Ok(st.next_batch()?.map(|batch| (batch, st)))
            })
            .boxed())
        }))
    }
}

/// A [`FileFormatFactory`] over [`PolyJsonFormat`], for a session that is not this engine's.
///
/// **The engine does not use it**, and that is a decision rather than an omission: registering a
/// factory under `json` replaces the one DataFusion resolves `COPY … STORED AS JSON` against, so a
/// session holding this one would read JSON through the union-tolerant reader *and write it*
/// through whatever this returns. Strata selects the reader per table instead
/// ([`formats`](crate::formats)), which leaves the writer alone.
///
/// It is offered because an embedder composing their own `SessionContext` may well want the
/// reader everywhere — `CREATE EXTERNAL TABLE … STORED AS JSON` in a plain DataFusion session
/// resolves through this map, and there is no per-table seam there to select on. The trade is the
/// one above: take it knowing the writer moves with it.
///
/// ```no_run
/// use std::sync::Arc;
/// use datafusion::prelude::SessionContext;
/// use strata_engine::json_poly::PolyJsonFormatFactory;
///
/// let mut ctx = SessionContext::new();
/// ctx.state_ref()
///     .write()
///     .register_file_format(Arc::new(PolyJsonFormatFactory::default()), true)
///     .unwrap();
/// ```
#[derive(Debug, Default)]
pub struct PolyJsonFormatFactory {
    /// What the reader is built with when `COPY`'s own `format.*` options say nothing.
    pub defaults: JsonRead,
}

impl GetExt for PolyJsonFormatFactory {
    fn get_ext(&self) -> String {
        "json".to_string()
    }
}

impl FileFormatFactory for PolyJsonFormatFactory {
    /// Reads the two options this reader has beyond the defaults it was built with —
    /// `format.compression` and `format.schema_infer_max_rec`, in DataFusion's own spelling.
    /// A key it does not take is left to whatever else reads the map, because a factory that
    /// refused one would refuse a session-level setting it has no business judging.
    fn create(
        &self,
        _state: &dyn Session,
        format_options: &HashMap<String, String>,
    ) -> Result<Arc<dyn FileFormat>> {
        let mut opts = self.defaults.clone();
        if let Some(raw) = format_options.get("format.compression") {
            opts.compression = match raw.parse::<FileCompressionType>()?.get_variant() {
                CompressionTypeVariant::UNCOMPRESSED => FileCompression::None,
                CompressionTypeVariant::GZIP => FileCompression::Gzip,
                CompressionTypeVariant::BZIP2 => FileCompression::Bzip2,
                CompressionTypeVariant::XZ => FileCompression::Xz,
                CompressionTypeVariant::ZSTD => FileCompression::Zstd,
            };
        }
        if let Some(raw) = format_options.get("format.schema_infer_max_rec") {
            let rows: usize = raw
                .parse()
                .map_err(|_| json_error("format.schema_infer_max_rec takes a number of records"))?;
            opts.infer_rows = (rows > 0).then_some(rows);
        }
        Ok(Arc::new(PolyJsonFormat::new(opts)))
    }

    fn default(&self) -> Arc<dyn FileFormat> {
        Arc::new(PolyJsonFormat::new(self.defaults.clone()))
    }
}
