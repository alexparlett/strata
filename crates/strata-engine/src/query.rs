//! The pagination engine: run each query **once**, spool the full result to a temp
//! **Arrow IPC** snapshot, then serve every page as a bounded `LIMIT/OFFSET` read — so RAM
//! only ever holds one page. Also the display-cell formatting (`CellFormat`).
//!
//! IPC rather than parquet because the snapshot is the boundary every result crosses, and
//! parquet's type system is narrower than Arrow's: it cannot write a union at all
//! (`arrow_to_parquet_schema` **panics**, ARROW-8817) nor a zero-field struct, so results had to
//! be coerced on the way in and the record view and JSON/CSV export then read the coerced form.
//! IPC round-trips anything the engine can emit. Compressed (see `ipc_write_options`) it is
//! the same size on disk as the parquet it replaced.
//!
//! Snapshots are keyed by [`SnapshotId`] — the Run's request id, unique per engine for
//! the life of the process — so a snapshot is **immutable**: a re-run materializes a
//! *new* snapshot under a new id, and every read keyed by an id targets a fixed set
//! (`docs/SNAPSHOT_SPEC.md`). Lifecycle (which ws owns which snapshot, when to
//! [`retire_snapshot`]) is the facade's own bookkeeping, in [`super::Engine`]. What lives
//! here is the *filesystem* side of it: the per-engine directory, the lock that marks it
//! live, and the startup sweep of everything no live engine still holds.

use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

use datafusion::arrow::array::{Array, ArrayRef, UInt64Array};
use datafusion::arrow::compute::concat_batches;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::ipc::writer::{FileWriter, IpcWriteOptions};
use datafusion::arrow::ipc::CompressionType;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::common::Column;
use datafusion::execution::options::{ArrowReadOptions, ReadOptions};
use datafusion::logical_expr::expr::ScalarFunction;
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::*;
use datafusion::sql::parser::Statement as DFStatement;
use datafusion::sql::TableReference;
use datafusion_functions_json::udfs::json_union_to_text_udf;
use datafusion_functions_json::JSON_UNION_DATA_TYPE;
use futures::StreamExt;
use std::sync::Arc;

use super::providers::in_workspace;
use strata_arrow::column_info;
use strata_arrow::config::effective;
use strata_core::util::{clip, DISPLAY_CHARS};
use strata_model::{Cell, ColumnInfo, QueryOutput, SnapshotId};

/// The prefix every result snapshot is registered under. Named here, next to the
/// only thing that mints one, because two other rules key off it: the statement
/// router refuses an intercepted statement that names a table with this prefix
/// (`sql::validate::classify`), and the schema provider hides such tables from every
/// enumeration (`engine::providers`) — the naming rule and the hiding rule must not
/// be able to drift apart.
const SNAPSHOT_PREFIX: &str = "__snap_";

pub fn snapshot_name(snapshot: SnapshotId) -> String {
    format!("{SNAPSHOT_PREFIX}{snapshot}")
}

/// Whether `name` is in the snapshot namespace — the one predicate the refusal and the
/// hiding both ask, so neither can answer differently from [`snapshot_name`].
///
/// Case-folded, because the one namespace is case-insensitive and `__SNAP_2` is the same
/// table — compared in place rather than through `to_ascii_lowercase`, because the router
/// runs this per identifier per statement on every keystroke and the whole answer is seven
/// bytes wide.
pub fn is_snapshot_name(name: &str) -> bool {
    name.get(..SNAPSHOT_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(SNAPSHOT_PREFIX))
}

/// Whether the *reference* `name` addresses the snapshot namespace — the prefix, **scoped to
/// the workspace catalog**, which is the only place a Run mints into.
///
/// The scoping is the DB workstream's correction and it lives here, beside the naming rule, for
/// the reason [`is_snapshot_name`] does: the refusal (`sql::validate`) and the hiding
/// (`engine::providers`) ask one question, so neither can decide on its own where the namespace
/// ends. Since a session holds a catalog per database connection, the prefix alone stopped being
/// the whole question: a server may perfectly well have a relation called `__snap_3`, where the
/// name is an ordinary one that reserves nothing, hides nothing and collides with nothing —
/// reading it is fine, and refusing it would fence off a table Strata does not own.
pub(crate) fn is_snapshot_ref(name: &TableReference) -> bool {
    in_workspace(name) && is_snapshot_name(name.table())
}

pub(crate) fn snapshots_root() -> String {
    let mut d = env::temp_dir();
    d.push("strata_snapshots");
    d.to_string_lossy().into_owned()
}

/// The name of one engine's subdirectory under the shared root. Scoped by **pid +
/// engine id**: `engine_id` is only process-unique, and the snapshot root in the OS temp
/// dir is machine-shared — without the pid, two concurrent processes (a second app
/// instance, parallel test binaries) both allocate `e_0`, `e_1`, … and would write into
/// each other's directory. (Not *deleting* each other's: that's the lock's job, see
/// [`claim_snapshot_dir`] — a pid can be recycled by a later process, so the name alone
/// never proves the owner is alive.)
fn snapshot_dir_name(engine_id: u64) -> String {
    format!("e_{}_{engine_id}", process::id())
}

/// Prefix of every engine directory (and of its lock file) — what [`purge_root`] treats
/// as "ours" and everything else as a stray.
const DIR_PREFIX: &str = "e_";

/// Suffix of an engine directory's lock file (`e_<pid>_<id>.lock`).
const LOCK_SUFFIX: &str = ".lock";

/// Per-engine snapshot subdirectory (see [`snapshot_dir_name`]).
pub fn snapshot_dir(engine_id: u64) -> String {
    let mut d = PathBuf::from(snapshots_root());
    d.push(snapshot_dir_name(engine_id));
    d.to_string_lossy().into_owned()
}

pub fn snapshot_file(engine_id: u64, snapshot: SnapshotId) -> String {
    let mut d = PathBuf::from(snapshot_dir(engine_id));
    d.push(format!("s_{snapshot}.arrow"));
    d.to_string_lossy().into_owned()
}

/// Retire one snapshot: deregister its table and delete its file. Safe on a snapshot
/// that never fully materialized (a failed / cancelled run's partial) — both halves
/// are best-effort.
pub fn retire_snapshot(ctx: &SessionContext, engine_id: u64, snapshot: SnapshotId) {
    let _ = ctx.deregister_table(snapshot_name(snapshot).as_str());
    let _ = fs::remove_file(snapshot_file(engine_id, snapshot));
}

/// Claim this engine's snapshot directory for the engine's whole lifetime. The returned
/// [`File`] **is** the claim: it holds an exclusive advisory lock that the OS releases
/// when the handle closes — on a clean drop, and on a crash for free — and
/// [`purge_snapshot_root`] skips every directory whose lock it cannot take.
///
/// The lock file sits *beside* the directory, and the order here is the guarantee: lock
/// first, `mkdir` second. So any directory a concurrent purge can see already has a held
/// lock, and there is no window in which a starting engine's directory looks abandoned.
/// (A lock file *inside* the directory would have exactly that window.)
///
/// `Err` means the claim failed, and **carries why** — an unwritable temp root, essentially.
/// The engine still runs (`materialize` creates the directory on demand), but its snapshots
/// are then unprotected against another instance's startup purge, so the caller logs the
/// reason alongside that consequence; a claim that fails the same way on every start is a
/// standing risk, not a transient one, and has to be legible.
pub fn claim_snapshot_dir(engine_id: u64) -> Result<File, String> {
    let root = PathBuf::from(snapshots_root());
    fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let name = snapshot_dir_name(engine_id);
    let path = lock_path(&root, &name);
    let lock = match claim_lock(&path) {
        Claim::Taken(lock) => lock,
        Claim::Held => return Err(format!("{} is held by another handle", path.display())),
        Claim::Unknown(e) => return Err(format!("{}: {e}", path.display())),
    };
    let dir = root.join(&name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(lock)
}

/// Give up this engine's snapshot directory (window closing): the directory and the lock
/// file beside it. The lock itself is released when the caller drops the handle, which
/// happens right after — so no other process can claim the name mid-delete.
pub fn discard_snapshot_dir(engine_id: u64) {
    let root = PathBuf::from(snapshots_root());
    let name = snapshot_dir_name(engine_id);
    let _ = fs::remove_dir_all(root.join(&name));
    let _ = fs::remove_file(lock_path(&root, &name));
}

/// Sweep the shared snapshot root of **dead** engines' leftovers: every directory whose
/// lock we can take (nobody holds it ⇒ its process is gone), plus any entry that isn't
/// one of our directory/lock pairs at all. A directory still locked by a live engine —
/// this app's other instance, a parallel test binary — is left alone.
///
/// That skip is the whole point: the pid-scoped naming ([`snapshot_dir_name`]) keeps two
/// processes out of each other's *files*, and a blanket `remove_dir_all` of the root
/// defeated it by deleting the other instance's live snapshots — after which every
/// uncached page read there fails.
///
/// Still meant to run **once at process startup**, before this process has an engine: an
/// engine that couldn't claim its lock has nothing protecting it here, and startup is the
/// one moment when this process has no snapshots of its own to lose.
///
/// **Nothing is deleted on a guess.** Only [`Claim::Taken`] proves an owner is gone; both
/// "a live engine holds it" and "we couldn't tell" leave the directory standing, because
/// the failure mode of guessing wrong is deleting a running instance's results — the very
/// bug the lock was added to fix — while the failure mode of guessing right is temp files
/// the OS reaper eventually collects. The two are not the same event, though, so the
/// indeterminate one is logged: a sweep that can never resolve anything (an unwritable
/// root, a filesystem with no working `flock`) would otherwise do nothing, forever,
/// silently.
pub fn purge_snapshot_root() {
    purge_root(Path::new(&snapshots_root()));
}

/// [`purge_snapshot_root`] against an explicit root, so the sweep is testable without
/// touching the machine-shared one.
fn purge_root(root: &Path) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(
                "snapshot purge: cannot read {} ({e}); no dead engine's spool files under \
                 it will ever be reclaimed",
                root.display()
            );
            return;
        }
    };
    let mut dirs: Vec<String> = Vec::new();
    let mut locks: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && name.starts_with(DIR_PREFIX) {
            dirs.push(name);
        } else if !is_dir && name.starts_with(DIR_PREFIX) && name.ends_with(LOCK_SUFFIX) {
            locks.push(name);
        } else {
            remove_any(&entry.path());
        }
    }
    for name in &dirs {
        let dir = root.join(name);
        let lock = lock_path(root, name);
        match claim_lock(&lock) {
            Claim::Taken(held) => {
                remove_any(&dir);
                let _ = fs::remove_file(&lock);
                drop(held);
            }
            Claim::Held => {}
            Claim::Unknown(e) => tracing::warn!(
                "snapshot purge: cannot tell whether {} is live ({e}); leaving it. If this \
                 repeats on every start, that directory's spool files are leaking and want \
                 clearing by hand",
                dir.display()
            ),
        }
    }
    for name in &locks {
        let dir = name.strip_suffix(LOCK_SUFFIX).unwrap_or(name);
        if dirs.iter().any(|d| d == dir) {
            continue;
        }
        let path = root.join(name);
        if let Claim::Taken(held) = claim_lock(&path) {
            let _ = fs::remove_file(&path);
            drop(held);
        }
    }
}

/// The lock file guarding the engine directory `name`, as a sibling of it.
fn lock_path(root: &Path, name: &str) -> PathBuf {
    root.join(format!("{name}{LOCK_SUFFIX}"))
}

/// What trying to take a lock told us. The three outcomes are **not** interchangeable:
/// [`Held`](Claim::Held) is routine contention with a live engine, while
/// [`Unknown`](Claim::Unknown) means the liveness test itself is unavailable — the same
/// skip, but a reportable condition rather than an expected one (see [`purge_snapshot_root`]).
enum Claim {
    /// We hold it. Nothing else did, so whatever owned this name is gone.
    Taken(File),
    /// Somebody else holds it — a live engine, in this or another process.
    Held,
    /// Neither could be established: the file wouldn't open (an unwritable or
    /// another-user-owned root) or the lock call itself failed (a filesystem with no
    /// working advisory locking).
    Unknown(io::Error),
}

/// Try to take the exclusive advisory lock on `path`, creating the file if needed.
fn claim_lock(path: &Path) -> Claim {
    let file = match fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => file,
        Err(e) => return Claim::Unknown(e),
    };
    match file.try_lock() {
        Ok(()) => Claim::Taken(file),
        Err(TryLockError::WouldBlock) => Claim::Held,
        Err(TryLockError::Error(e)) => Claim::Unknown(e),
    }
}

/// Remove a path whatever it is (file or directory). Best effort, but not *silent*: a
/// purge that fails to purge is exactly the invisible failure this sweep exists to close,
/// so anything but "it was already gone" is logged.
fn remove_any(path: &Path) {
    let is_dir = fs::symlink_metadata(path)
        .map(|m| m.is_dir())
        .unwrap_or(false);
    let removed = if is_dir {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match removed {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("snapshot purge: cannot remove {} ({e})", path.display()),
    }
}

/// How a snapshot's Arrow IPC file is written.
///
/// **LZ4, not uncompressed.** Measured over 1M–20M-row results in three shapes, raw IPC is 1.4–4.4x
/// the size of the parquet snapshots this replaced; with LZ4 it is **0.46–0.73x**. That is what
/// makes the format swap affordable — uncompressed IPC would have traded real disk for the type
/// fidelity. LZ4 rather than ZSTD because a snapshot is written on the query's critical path and
/// read back immediately, where ZSTD's smaller file costs again on every page read.
///
/// **Named for the format, not for the snapshot**, because an internal table's spool writes IPC too
/// and the two must not drift into two codecs. The one place it does not reach is DataFusion's own
/// `ArrowFileSink`, which a CTAS drives and which hardcodes `LZ4_FRAME` — so the sink and this
/// agree by coincidence rather than construction, and turning this dial would leave a CTAS's files
/// behind.
pub(super) fn ipc_write_options() -> Result<IpcWriteOptions, String> {
    IpcWriteOptions::default()
        .try_with_compression(Some(CompressionType::LZ4_FRAME))
        .map_err(|e| e.to_string())
}

/// The name the snapshot's ordinal column gets: `__strata_ord`, prefix-escalated until it
/// collides with nothing in the result. Result column names come out of the user's own
/// query and can be anything, including this one.
fn ordinal_name(schema: &Schema) -> String {
    let mut name = String::from(ORDINAL_BASE);
    while schema.fields().iter().any(|f| f.name() == &name) {
        name.insert(0, '_');
    }
    name
}

/// The unescalated ordinal column name (`docs/SNAPSHOT_SPEC.md` §9).
const ORDINAL_BASE: &str = "__strata_ord";

/// The spool file's schema: the result's own, with the ordinal appended last.
///
/// `UInt64` and non-nullable, which is what the plan-level `row_number()` this replaced
/// produced — so the file's shape, and every reader's view of it, is unchanged.
fn ordinal_schema(schema: &SchemaRef, ord: &str) -> SchemaRef {
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    fields.push(Field::new(ord, DataType::UInt64, false));
    Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()))
}

/// `batch` with the ordinal appended, numbering its rows from `written` — the count already
/// spooled, so the value is the row's position in the file.
///
/// **1-based**, which is what the `row_number()` this replaced produced. Nothing reads the
/// values (every reader only orders by them), but the file's shape is described in
/// `docs/SNAPSHOT_SPEC.md` §9 and there is no reason to make that description false.
///
/// One allocation per batch, of a contiguous range — as cheap as an Arrow column gets; the
/// user's own columns are carried over by reference.
fn with_ordinal(
    batch: &RecordBatch,
    schema: &SchemaRef,
    written: u64,
) -> Result<RecordBatch, String> {
    let first = written + 1;
    let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
    columns.push(Arc::new(UInt64Array::from_iter_values(
        first..first + batch.num_rows() as u64,
    )));
    RecordBatch::try_new(Arc::clone(schema), columns).map_err(|e| e.to_string())
}

/// Run the query **once**, streaming every batch straight to a fresh IPC snapshot
/// on disk while counting the exact total and capturing the first page — no separate
/// `COUNT`, no re-read, bounded memory. On failure the partial snapshot is cleaned up
/// here (nothing was ever registered); the caller only ever sees a fully-materialized
/// snapshot or none (`QueryOutput::snapshot`).
pub async fn run_and_snapshot(
    ctx: &SessionContext,
    engine_id: u64,
    snapshot: SnapshotId,
    stmt: DFStatement,
    page_size: usize,
    fmt: &CellFormat,
    policy: ReadPolicy,
) -> Result<(QueryOutput, RecordBatch, SnapshotStats), String> {
    let result = materialize(ctx, engine_id, snapshot, stmt, page_size, fmt, policy).await;
    if result.is_err() {
        let _ = fs::remove_file(snapshot_file(engine_id, snapshot));
    }
    result
}

/// What the write pass **observed**, so no later reader has to scan for it again.
///
/// Parquet's footer carried per-column statistics and `export::partition_columns_have_no_nulls`
/// read the null counts from it. Arrow IPC carries none — `ArrowFormat::infer_stats` is
/// `Statistics::new_unknown` — but nothing was ever gained by asking the *file*: `materialize`
/// streams every batch already, and `Array::null_count` is a stored field on the null buffer, so
/// the exact count is a running sum over data we are holding anyway. Free at write time, and a
/// map lookup instead of a scan at export time.
///
/// Not persisted, and deliberately so: a snapshot never outlives its process (pid-scoped temp
/// directory, retired on the next run of its workspace), so this has exactly the snapshot's
/// lifetime and lives beside the rest of its bookkeeping in `Lifecycle`. A footer or a sidecar
/// file would be a second thing to keep in step for no gain.
#[derive(Debug, Clone, Default)]
pub struct SnapshotStats {
    /// Exact null count per column, in `QueryOutput::columns` order.
    pub nulls: Vec<u64>,
    /// The name of this snapshot's **ordinal column** (`docs/SNAPSHOT_SPEC.md` §9) — the
    /// written result order every ordered read sorts by and every reader projects away.
    /// Usually `__strata_ord`; escalated by prefix when the result itself has a column of
    /// that name. `None` means the file genuinely has no ordinal — an `EXPLAIN` result or
    /// one with duplicate column names (see `materialize`) — and readers then read
    /// unordered, exactly as every snapshot did before ordinals existed.
    pub ord: Option<String>,
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
    /// The above, plus `LogicalPlan::Statement` — `EXECUTE` of a prepared query (ED-08).
    Statements,
}

/// Plan one **already-resolved** statement under `policy` — `SessionContext::sql_with_options`
/// with the parse taken out, since [`parse_one`](crate::sql::parse_one) did it.
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
    engine_id: u64,
    snapshot: SnapshotId,
    stmt: DFStatement,
    page_size: usize,
    fmt: &CellFormat,
    policy: ReadPolicy,
) -> Result<(QueryOutput, RecordBatch, SnapshotStats), String> {
    let start = Instant::now();
    let snap = snapshot_name(snapshot);
    let file = snapshot_file(engine_id, snapshot);

    if let Some(parent) = Path::new(&file).parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

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
    let mut ord_schema: Option<SchemaRef> = None;

    let mut writer: Option<FileWriter<File>> = None;
    let mut total = 0usize;
    let mut nulls = vec![0u64; arrow_schema.fields().len()];
    let mut page1: Vec<Vec<Cell>> = Vec::new();
    let mut page1_batches: Vec<RecordBatch> = Vec::new();
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(|e| e.to_string())?;
        let width = batch.num_columns().min(nulls.len());
        if writer.is_none() {
            let schema = match &ord {
                Some(ord) => {
                    let schema = ordinal_schema(&batch.schema(), ord);
                    ord_schema = Some(Arc::clone(&schema));
                    schema
                }
                None => batch.schema(),
            };
            let out = File::create(&file).map_err(|e| e.to_string())?;
            writer = Some(
                FileWriter::try_new_with_options(out, &schema, ipc_write_options()?)
                    .map_err(|e| e.to_string())?,
            );
        }
        if let Some(w) = writer.as_mut() {
            let spooled = match &ord_schema {
                Some(schema) => &with_ordinal(&batch, schema, total as u64)?,
                None => &batch,
            };
            w.write(spooled).map_err(|e| e.to_string())?;
        }
        total += batch.num_rows();
        for (i, col) in batch.columns().iter().take(width).enumerate() {
            nulls[i] += col.null_count() as u64;
        }
        append_batch_capped(&batch, &mut page1, &mut page1_batches, page_size, fmt)?;
    }

    let materialized = writer.is_some();
    if let Some(mut w) = writer {
        w.finish().map_err(|e| e.to_string())?;
        let listing = ArrowReadOptions::default()
            .to_listing_options(&ctx.copied_config(), ctx.copied_table_options());
        let listing = match &ord {
            Some(ord) => listing
                .with_file_sort_order(vec![vec![
                    col(Column::from_name(ord.clone())).sort(true, false)
                ]]),
            None => listing,
        };
        ctx.register_listing_table(snap.as_str(), file.as_str(), listing, None, None)
            .await
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
        SnapshotStats { nulls, ord },
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
        let eff = |k: &str| effective(overrides, k).unwrap_or_default();
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
    page: usize,
    page_size: usize,
    sort: Option<(String, bool)>,
    ord: Option<String>,
    fmt: &CellFormat,
) -> Result<Page, String> {
    let snap = snapshot_name(snapshot);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    read_page(ctx, &snap, offset, page_size, sort, ord, fmt).await
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch root of our own: the sweep under test is destructive, and pointing it at
    /// the machine-shared root would delete the snapshots of whatever else is running.
    fn scratch_root(tag: &str) -> PathBuf {
        let mut d = env::temp_dir();
        d.push(format!("strata_purge_test_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("scratch root");
        d
    }

    /// An engine directory with one snapshot file in it.
    fn engine_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).expect("engine dir");
        fs::write(dir.join("s_1.arrow"), b"snapshot").expect("snapshot file");
        dir
    }

    /// Take a lock, insisting it was actually free.
    fn take(path: &Path) -> File {
        match claim_lock(path) {
            Claim::Taken(f) => f,
            Claim::Held => panic!("{} is already held", path.display()),
            Claim::Unknown(e) => panic!("{}: {e}", path.display()),
        }
    }

    #[test]
    fn purge_sweeps_dead_engines_and_spares_live_ones() {
        let root = scratch_root("mixed");

        let live = engine_dir(&root, "e_1_0");
        let held = take(&lock_path(&root, "e_1_0"));

        let dead = engine_dir(&root, "e_2_0");
        drop(take(&lock_path(&root, "e_2_0")));

        let lockless = engine_dir(&root, "e_3_0");

        let orphan = lock_path(&root, "e_4_0");
        drop(take(&orphan));
        let stray = root.join("garbage.txt");
        fs::write(&stray, b"junk").expect("stray file");

        purge_root(&root);

        assert!(
            live.join("s_1.arrow").exists(),
            "a live engine's snapshots must survive another instance's startup purge"
        );
        assert!(lock_path(&root, "e_1_0").exists(), "…and so must its lock");
        assert!(!dead.exists(), "a dead engine's directory goes");
        assert!(!lock_path(&root, "e_2_0").exists(), "…and its lock with it");
        assert!(!lockless.exists(), "an unlocked directory is nobody's");
        assert!(!orphan.exists(), "an orphan lock is swept too");
        assert!(!stray.exists(), "so is anything that isn't ours");

        drop(held);
        let _ = fs::remove_dir_all(&root);
    }

    /// The purge skips a directory either way, but the two reasons are different events:
    /// contention with a live engine is routine, an unusable lock means the sweep can
    /// never reclaim anything and must say so. Collapsing them (the earlier
    /// `Err(WouldBlock) | Err(Error(_)) => None`) made a permanently-failing sweep
    /// indistinguishable from a healthy one.
    #[test]
    fn a_held_lock_and_an_unusable_one_are_different_answers() {
        let root = scratch_root("claims");

        let contested = root.join("e_1_0.lock");
        let held = take(&contested);
        assert!(matches!(claim_lock(&contested), Claim::Held));
        drop(held);
        assert!(matches!(claim_lock(&contested), Claim::Taken(_)));

        let opaque = root.join("e_2_0.lock");
        fs::create_dir_all(&opaque).expect("an unopenable lock path");
        assert!(matches!(claim_lock(&opaque), Claim::Unknown(_)));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_claimed_directory_cannot_be_claimed_twice() {
        let engine_id = u64::MAX;
        let dir = PathBuf::from(snapshot_dir(engine_id));
        let claim = claim_snapshot_dir(engine_id).expect("first claim");
        assert!(dir.is_dir(), "the claim creates the directory it guards");
        fs::write(dir.join("s_1.arrow"), b"snapshot").expect("snapshot file");

        assert!(
            claim_snapshot_dir(engine_id).is_err(),
            "a held claim is exclusive — and a refused claim must not touch the directory"
        );
        assert!(
            dir.join("s_1.arrow").exists(),
            "the refused claim left the live directory alone"
        );

        discard_snapshot_dir(engine_id);
        drop(claim);
        assert!(!dir.exists(), "discard removes the directory");
        assert!(
            !PathBuf::from(snapshots_root())
                .join(format!("{}{LOCK_SUFFIX}", snapshot_dir_name(engine_id)))
                .exists(),
            "…and the lock file beside it"
        );
    }
}
