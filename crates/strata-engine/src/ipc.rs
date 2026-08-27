//! How Strata writes Arrow IPC.
//!
//! One codec decision, shared by everything of ours that spools Arrow: the default snapshot store
//! ([`snapshots::local_ipc`](crate::snapshots::local_ipc)) and the default internal-table store
//! ([`tables::local_ipc`](crate::tables::local_ipc)). Named for the format rather than for either
//! of them, because the two must not drift into two codecs.

use datafusion::arrow::ipc::writer::IpcWriteOptions;
use datafusion::arrow::ipc::CompressionType;

/// The options every Arrow IPC file Strata writes is written with.
///
/// **LZ4, not uncompressed.** Measured over 1M–20M-row results in three shapes, raw IPC is 1.4–4.4x
/// the size of the parquet snapshots this replaced; with LZ4 it is **0.46–0.73x**. That is what
/// makes the format swap affordable — uncompressed IPC would have traded real disk for the type
/// fidelity. LZ4 rather than ZSTD because a snapshot is written on the query's critical path and
/// read back immediately, where ZSTD's smaller file costs again on every page read.
///
/// The one place it does not reach is DataFusion's own `ArrowFileSink`, which a typed
/// `COPY … STORED AS ARROW` drives and which hardcodes `LZ4_FRAME` — so that sink and this agree
/// by coincidence rather than by construction. And turning this dial would leave every existing
/// internal table's files behind on the old codec, so it is not a dial to turn casually.
pub(crate) fn ipc_write_options() -> Result<IpcWriteOptions, String> {
    IpcWriteOptions::default()
        .try_with_compression(Some(CompressionType::LZ4_FRAME))
        .map_err(|e| e.to_string())
}
