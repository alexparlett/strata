//! Arrow IPC row counts from the file's own footer (ED-04) — the one thing DataFusion's
//! `ArrowFormat` does not answer.
//!
//! `ArrowFormat::infer_stats` returns `Statistics::new_unknown`, and with internal tables that gap
//! became load-bearing: an internal table's whole data set is one Strata wrote in this format, so a
//! catalog row that cannot say how many rows it holds is the one table in the project with no
//! answer.
//!
//! [`StrataArrowFormat`] is `ArrowFormat` with one method replaced. The count comes from the IPC
//! **file footer**, which lists a block per record batch carrying that batch's `length` — so the
//! read is metadata-only and never touches a data page, and the ranges go through
//! [`get_ranges`](datafusion::object_store::ObjectStore::get_ranges), which coalesces adjacent
//! requests.
//!
//! **Row counts only.** Null counts would need the batches' own buffers, and the profile answers
//! that for real with a full scan; absent stays absent. A footer that will not parse is not an
//! error here — `infer_stats` is best-effort by contract, and the *scan* is where an unreadable
//! file has to fail with its own diagnosis.

use std::sync::Arc;

use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::ipc::reader::read_footer_length;
use datafusion::arrow::ipc::{root_as_footer, root_as_message};
use datafusion::catalog::Session;
use datafusion::common::stats::Precision;
use datafusion::common::{Result, Statistics};
use datafusion::datasource::file_format::arrow::ArrowFormat;
use datafusion::datasource::file_format::file_compression_type::FileCompressionType;
use datafusion::datasource::file_format::FileFormat;
use datafusion::datasource::physical_plan::{FileScanConfig, FileSinkConfig, FileSource};
use datafusion::datasource::table_schema::TableSchema;
use datafusion::object_store::{ObjectMeta, ObjectStore, ObjectStoreExt};
use datafusion::physical_expr::{LexOrdering, LexRequirement};
use datafusion::physical_plan::ExecutionPlan;

/// The IPC file trailer: a little-endian `i32` footer length followed by `ARROW1`.
const TRAILER: u64 = 10;

/// DataFusion's `ArrowFormat`, reading exact row counts out of the IPC footer.
///
/// Composition rather than a fork: every other method delegates verbatim, so the schema
/// inference, the scan and the write sink are DataFusion's own and stay so through an upgrade.
/// [`infer_stats_and_ordering`](FileFormat::infer_stats_and_ordering) is deliberately **not**
/// delegated — the trait's default calls `infer_stats`, and delegating it would route right past
/// the one method this type exists to replace.
#[derive(Debug, Default)]
pub struct StrataArrowFormat {
    inner: ArrowFormat,
}

#[async_trait::async_trait]
impl FileFormat for StrataArrowFormat {
    fn get_ext(&self) -> String {
        self.inner.get_ext()
    }

    fn get_ext_with_compression(&self, c: &FileCompressionType) -> Result<String> {
        self.inner.get_ext_with_compression(c)
    }

    fn compression_type(&self) -> Option<FileCompressionType> {
        self.inner.compression_type()
    }

    async fn infer_schema(
        &self,
        state: &dyn Session,
        store: &Arc<dyn ObjectStore>,
        objects: &[ObjectMeta],
    ) -> Result<SchemaRef> {
        self.inner.infer_schema(state, store, objects).await
    }

    /// The whole point of this type: `ArrowFormat` answers `new_unknown` here.
    ///
    /// **The row count and nothing else.** `total_byte_size` stays `Absent`, which is what
    /// DataFusion's own parquet reader leaves it as
    /// (`DFParquetMetadata::statistics_from_parquet_metadata` sets `num_rows` and never touches
    /// it) — and the object's length is not that number anyway: these files are LZ4-frame IPC,
    /// so the size on disk is a fraction of the size in memory. `JoinSelection` reads
    /// `total_byte_size` first and decides from it whether to **collect a side of a join into
    /// RAM** (`supports_collect_by_thresholds` against
    /// `hash_join_single_partition_threshold`, 1 MB), so declaring a compressed length `Exact`
    /// would invite it to buffer a table many times that size. Absent makes it fall back to
    /// `num_rows` — which is the number this type exists to supply, and an honest one.
    async fn infer_stats(
        &self,
        _state: &dyn Session,
        store: &Arc<dyn ObjectStore>,
        table_schema: SchemaRef,
        object: &ObjectMeta,
    ) -> Result<Statistics> {
        let mut stats = Statistics::new_unknown(&table_schema);
        if let Some(rows) = footer_rows(store, object).await {
            stats.num_rows = Precision::Exact(rows);
        }
        Ok(stats)
    }

    async fn infer_ordering(
        &self,
        state: &dyn Session,
        store: &Arc<dyn ObjectStore>,
        table_schema: SchemaRef,
        object: &ObjectMeta,
    ) -> Result<Option<LexOrdering>> {
        self.inner
            .infer_ordering(state, store, table_schema, object)
            .await
    }

    async fn create_physical_plan(
        &self,
        state: &dyn Session,
        conf: FileScanConfig,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        self.inner.create_physical_plan(state, conf).await
    }

    async fn create_writer_physical_plan(
        &self,
        input: Arc<dyn ExecutionPlan>,
        state: &dyn Session,
        conf: FileSinkConfig,
        order_requirements: Option<LexRequirement>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        self.inner
            .create_writer_physical_plan(input, state, conf, order_requirements)
            .await
    }

    fn file_source(&self, table_schema: TableSchema) -> Arc<dyn FileSource> {
        self.inner.file_source(table_schema)
    }
}

/// The exact row count of one Arrow IPC file, or `None` for anything this cannot read.
///
/// Two metadata reads and no data pages:
///
/// 1. the last [`TRAILER`] bytes give the footer's length, and the footer lists one block per
///    record batch — each block's `offset` and `metaDataLength`;
/// 2. one coalesced batch of small ranges over those block metadata regions, each of which is a
///    `Message` whose `RecordBatch` header carries that batch's `length`.
///
/// A file too short to hold a trailer, a footer that will not parse, a block whose message is not
/// a record batch — all answer `None` rather than an error, per the module doc.
async fn footer_rows(store: &Arc<dyn ObjectStore>, object: &ObjectMeta) -> Option<usize> {
    let size = object.size;
    if size < TRAILER {
        return None;
    }
    let trailer = store
        .get_range(&object.location, size - TRAILER..size)
        .await
        .ok()?;
    let footer_len =
        read_footer_length(<[u8; TRAILER as usize]>::try_from(&trailer[..]).ok()?).ok()? as u64;
    let footer_end = size - TRAILER;
    let footer_start = footer_end.checked_sub(footer_len)?;
    let footer = store
        .get_range(&object.location, footer_start..footer_end)
        .await
        .ok()?;
    let blocks = root_as_footer(&footer).ok()?.recordBatches()?;

    let mut ranges: Vec<std::ops::Range<u64>> = Vec::with_capacity(blocks.len());
    for block in blocks {
        let start = u64::try_from(block.offset()).ok()?;
        let len = u64::try_from(block.metaDataLength()).ok()?;
        let end = start.checked_add(len).filter(|end| *end <= size)?;
        ranges.push(start..end);
    }
    if ranges.is_empty() {
        return Some(0);
    }
    let metas = store.get_ranges(&object.location, &ranges).await.ok()?;

    let mut rows = 0usize;
    for meta in &metas {
        rows = rows.checked_add(message_rows(meta)?)?;
    }
    Some(rows)
}

/// The 4 bytes that may precede a message's own length prefix (arrow-rs's `CONTINUATION_MARKER`,
/// which is private to that crate).
const CONTINUATION_MARKER: [u8; 4] = [0xff; 4];

/// The `length` of the record batch one block's metadata describes.
///
/// The framing rule is arrow-rs's own `parse_message`, reproduced because that function is
/// private: a message is preceded by a 4-byte length, optionally in turn preceded by the
/// continuation marker. Reproduced rather than approximated — guessing at the prefix would read
/// a flatbuffer from the wrong offset, and a flatbuffer read from the wrong offset does not
/// reliably fail.
fn message_rows(bytes: &[u8]) -> Option<usize> {
    let body = match bytes.get(..4)? == CONTINUATION_MARKER {
        true => bytes.get(8..)?,
        false => bytes.get(4..)?,
    };
    let length = root_as_message(body)
        .ok()?
        .header_as_record_batch()?
        .length();
    usize::try_from(length).ok()
}

#[cfg(test)]
mod tests {
    use std::{env, fs, process};

    use datafusion::arrow::array::{ArrayRef, Int32Array};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::ipc::writer::FileWriter;
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::object_store::local::LocalFileSystem;
    use datafusion::object_store::path::Path as StorePath;

    use crate::query::ipc_write_options;

    use super::*;

    /// An IPC file of `batches` batches of `rows` rows each, written exactly as the spool writes
    /// one, and the store + meta a `FileFormat` would be handed for it.
    fn written(tag: &str, batches: usize, rows: i32) -> (Arc<dyn ObjectStore>, ObjectMeta) {
        let dir = env::temp_dir().join(format!("strata_arrow_stats_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("part-0.arrow");

        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int32, false)]));
        let file = fs::File::create(&path).unwrap();
        let mut writer =
            FileWriter::try_new_with_options(file, &schema, ipc_write_options().unwrap()).unwrap();
        for batch in 0..batches {
            let values: ArrayRef = Arc::new(Int32Array::from(
                (0..rows)
                    .map(|r| batch as i32 * rows + r)
                    .collect::<Vec<_>>(),
            ));
            writer
                .write(&RecordBatch::try_new(Arc::clone(&schema), vec![values]).unwrap())
                .unwrap();
        }
        writer.finish().unwrap();

        let store: Arc<dyn ObjectStore> = Arc::new(LocalFileSystem::new());
        let location = StorePath::from_absolute_path(&path).unwrap();
        let size = fs::metadata(&path).unwrap().len();
        let meta = ObjectMeta {
            location,
            last_modified: chrono::DateTime::UNIX_EPOCH,
            size,
            e_tag: None,
            version: None,
        };
        (store, meta)
    }

    /// **Every batch, not the first.** The count is a sum over the footer's blocks, and a file
    /// written in several batches is the ordinary case — the spool emits one per `batch_size`.
    #[tokio::test]
    async fn a_multi_batch_file_counts_every_batch() {
        let (store, meta) = written("multi", 4, 3);
        assert_eq!(footer_rows(&store, &meta).await, Some(12));
    }

    /// A file with a schema and no batches is what a zero-row `CREATE TABLE` writes, and zero
    /// is the true answer for it — not "unknown", which would leave the one table Strata itself
    /// wrote unable to say how many rows it holds.
    #[tokio::test]
    async fn an_empty_file_counts_zero_rather_than_unknown() {
        let (store, meta) = written("empty", 0, 0);
        assert_eq!(footer_rows(&store, &meta).await, Some(0));
    }

    /// **The row count and nothing else.** `total_byte_size` stays absent on purpose: the object's
    /// length is the LZ4-compressed size, and `JoinSelection` reads that field first to decide
    /// whether to collect a side of a join into memory. Declaring a compressed length would
    /// invite it to buffer a table several times larger than the number it was given.
    #[tokio::test]
    async fn only_the_row_count_is_claimed() {
        let (store, meta) = written("claims", 2, 4);
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int32, false)]));
        let ctx = datafusion::prelude::SessionContext::new();
        let stats = StrataArrowFormat::default()
            .infer_stats(&ctx.state(), &store, schema, &meta)
            .await
            .expect("stats");

        assert_eq!(stats.num_rows, Precision::Exact(8));
        assert_eq!(stats.total_byte_size, Precision::Absent);
        assert!(
            stats
                .column_statistics
                .iter()
                .all(|c| c.null_count == Precision::Absent),
            "null counts are the profile's, from a real scan"
        );
    }

    /// Anything this cannot read answers `None`, never an error: `infer_stats` is best effort by
    /// contract, and the *scan* is where an unreadable file has to fail with its own diagnosis.
    #[tokio::test]
    async fn a_file_that_is_not_arrow_ipc_reports_nothing() {
        let (store, mut meta) = written("garbage", 1, 1);
        let target = env::temp_dir()
            .join(format!("strata_arrow_stats_{}_garbage", process::id()))
            .join("part-0.arrow");
        fs::write(&target, b"not an arrow file").unwrap();
        meta.size = fs::metadata(&target).unwrap().len();
        assert_eq!(footer_rows(&store, &meta).await, None);
    }
}
