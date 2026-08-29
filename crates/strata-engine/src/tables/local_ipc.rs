//! The default store: a directory of **Arrow IPC** files per table, published by rename.
//!
//! The engine's own default follows the project folder — `.strata/tables/<slug>/` under whatever
//! [`Engine::set_data_dir`](crate::Engine::set_data_dir) said — so the def's project-relative
//! source path and this store's directory are two renderings of one location.
//! [`new_in`](LocalIpcTableStore::new_in) roots a store somewhere of the embedder's own instead.
//!
//! IPC rather than parquet for the snapshot store's reason (`crate::snapshots::local_ipc`): the
//! rows arrive as whatever a query produced, and IPC round-trips anything the engine can emit.
//! The codec is `crate::ipc`'s, shared with the snapshot spool so the two cannot drift.

use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use datafusion::arrow::ipc::writer::FileWriter;
use datafusion::catalog::TableProvider;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::execution::SendableRecordBatchStream;
use datafusion::prelude::SessionContext;
use futures::StreamExt;

use crate::arrow_stats::StrataArrowFormat;
use crate::ipc::ipc_write_options;
use crate::tables::InternalTableStore;
use strata_core::project::tables_dir;
use strata_core::util::temp_dir_name;

/// The extension every file this store writes carries, and the one its listing filters on —
/// the same answer `Formats::extension` gives for [`SourceFormat::Arrow`](strata_model::SourceFormat).
const ARROW_EXT: &str = ".arrow";

/// The one file a [`create`](InternalTableStore::create) publishes. A fixed name rather than a
/// minted one because a create owns its whole directory, and the path is occasionally read by
/// people.
const CREATE_FILE: &str = "part-0.arrow";

/// A directory of IPC files under a tables root, one subdirectory per slug.
pub struct LocalIpcTableStore {
    root: Root,
}

/// Where the tables root comes from — fixed by the embedder, or following the engine's project
/// folder, which is set after the engine is built ([`Engine::set_data_dir`](crate::Engine::set_data_dir)).
enum Root {
    /// The tables directory itself, as [`new_in`](LocalIpcTableStore::new_in) was given it.
    Dir(PathBuf),
    /// The engine's project-folder cell; the tables directory is `.strata/tables` under it, and
    /// `None` is an engine with no project behind it — the arms refuse a create before it can
    /// reach a rootless store, and registration falls back to the def's own resolved paths.
    Following(Arc<Mutex<Option<PathBuf>>>),
}

impl LocalIpcTableStore {
    /// A store over the directory `dir` — for an embedder that keeps Strata-owned tables
    /// somewhere of its own, and for a test that must not touch a project.
    ///
    /// `dir` is the tables root itself: each table is a subdirectory of it, named by slug.
    pub fn new_in(dir: impl AsRef<Path>) -> Self {
        Self {
            root: Root::Dir(dir.as_ref().to_path_buf()),
        }
    }

    /// The engine default: a store under the project folder's `.strata/tables`, following the
    /// engine's own data-root cell so a project opened after the engine was built is still where
    /// the tables land.
    pub(crate) fn following(root: Arc<Mutex<Option<PathBuf>>>) -> Self {
        Self {
            root: Root::Following(root),
        }
    }

    /// The tables root as of now, or `None` for a following store whose engine has no project
    /// folder yet.
    fn dir(&self) -> Option<PathBuf> {
        match &self.root {
            Root::Dir(dir) => Some(dir.clone()),
            Root::Following(cell) => cell.lock().unwrap().as_deref().map(tables_dir),
        }
    }

    /// [`dir`](Self::dir) where an operation cannot proceed without one.
    fn tables(&self) -> Result<PathBuf, String> {
        self.dir()
            .ok_or_else(|| "No project folder holds internal table data".to_string())
    }
}

#[async_trait]
impl InternalTableStore for LocalIpcTableStore {
    /// Write `rows` under a `.tmp-…` sibling and move the whole directory into place in one
    /// step — **published by rename**, the discipline the snapshot writer keeps, so a crash
    /// mid-spool leaves nothing but a temp directory the next `.strata` write sweeps
    /// (`project::tidy_strata_dir`) rather than a half-written table under a real slug.
    ///
    /// The staging directory is a **sibling** of the destination, which is what makes the
    /// publish a rename at all: the move is within one filesystem and atomic. A caller free to
    /// name any destination could ask for one across a mount point and lose the whole spool to
    /// `EXDEV` at the last step.
    ///
    /// One file, written with the stream's own schema — so a stream with no batches still
    /// publishes a schema-carrying, zero-row file. IPC self-describes, and that file is where a
    /// replay's schema comes back from; nothing is copied into the def.
    async fn create(&self, slug: &str, rows: SendableRecordBatchStream) -> Result<u64, String> {
        let tables = self.tables()?;
        fs::create_dir_all(&tables).map_err(|e| format!("{}: {e}", tables.display()))?;
        let staging = Staging::open(&tables)?;

        let count = drain(&staging.dir.join(CREATE_FILE), rows).await?;

        let dest = tables.join(slug);
        if dest.exists() {
            fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        fs::rename(&staging.dir, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        staging.published();
        Ok(count)
    }

    /// One LZ4-frame IPC file per statement, spooled into a `.tmp-…` sibling and renamed into
    /// the table's directory once the stream ends — so the unit is visible entire or not at
    /// all, and an interrupted append leaves what the `.strata` sweep collects rather than a
    /// truncated file the table's every later scan trips over.
    async fn append(&self, slug: &str, rows: SendableRecordBatchStream) -> Result<u64, String> {
        let tables = self.tables()?;
        let dir = tables.join(slug);
        if !dir.is_dir() {
            return Err(format!("{}: no table data to append to", dir.display()));
        }
        let staging = Staging::open(&tables)?;

        let name = part_name();
        let count = drain(&staging.dir.join(&name), rows).await?;

        let landed = dir.join(&name);
        if landed.exists() {
            return Err(format!("{}: already exists", landed.display()));
        }
        fs::rename(staging.dir.join(&name), &landed)
            .map_err(|e| format!("{}: {e}", landed.display()))?;
        Ok(count)
    }

    /// A `ListingTable` over the slug's directory: the Arrow reader with footer row counts
    /// ([`StrataArrowFormat`]), the session's config, and the runtime's per-file statistics
    /// cache handed over by name — a hand-built listing opts into every default
    /// `register_listing_table` would have applied.
    ///
    /// It re-`LIST`s per scan (this engine runs no list-files cache), which is the
    /// append-visibility rule the module contract states.
    async fn provider(
        &self,
        ctx: &SessionContext,
        slug: &str,
    ) -> Result<Option<Arc<dyn TableProvider>>, String> {
        let Some(tables) = self.dir() else {
            return Ok(None);
        };
        let dir = tables.join(slug);
        if !dir.is_dir() {
            return Ok(None);
        }
        let opts = ListingOptions::new(Arc::new(StrataArrowFormat::default()))
            .with_session_config_options(&ctx.copied_config())
            .with_file_extension(ARROW_EXT);
        let url = ListingTableUrl::parse(dir_path(&dir)).map_err(|e| e.to_string())?;
        let schema = opts
            .infer_schema(&ctx.state(), &url)
            .await
            .map_err(|e| e.to_string())?;
        let config = ListingTableConfig::new(url)
            .with_listing_options(opts)
            .with_schema(schema);
        let table = ListingTable::try_new(config)
            .map_err(|e| e.to_string())?
            .with_cache(ctx.runtime_env().cache_manager.get_file_statistic_cache());
        Ok(Some(Arc::new(table)))
    }

    /// Destroy the table's directory — **by rename first**.
    ///
    /// The spool publishes by rename; this discards by rename, and for the mirror-image reason.
    /// A `remove_dir_all` walks the directory in place, so anything that interrupts it — a
    /// killed process, a permission failure partway down, a window torn down while the delete
    /// runs on a background thread — leaves a half-emptied directory under the table's *real*
    /// slug, which nothing collects: the def naming it is already gone, and
    /// `project::tidy_strata_dir` sweeps only `.tmp-…`. The rename is a single atomic step
    /// within one directory, so the moment it returns the data is unreachable under that slug
    /// whatever happens next, and whatever is left is exactly what the sweep already exists to
    /// collect.
    ///
    /// **The rename is the operation; the delete is housekeeping.** A failure to remove the
    /// moved directory is litter, not a failed drop — the table is gone either way — so it is
    /// logged and not reported, or the app would tell the user a drop failed that plainly
    /// succeeded.
    async fn discard(&self, slug: &str) -> Result<(), String> {
        let tables = self.tables()?;
        let dir = tables.join(slug);
        if !dir.exists() {
            return Ok(());
        }
        let aside = tables.join(temp_dir_name());
        fs::rename(&dir, &aside).map_err(|e| format!("{}: {e}", dir.display()))?;
        if let Err(e) = fs::remove_dir_all(&aside) {
            tracing::warn!(
                "could not remove {} after dropping its table ({e}); the .strata sweep will",
                aside.display()
            );
        }
        Ok(())
    }

    /// The tables root **as of now**, which is what a following store can honestly answer: the
    /// project it follows is set after the engine is built, and a store with no project behind it
    /// holds nothing anywhere yet.
    fn owned_storage(&self) -> Vec<PathBuf> {
        self.dir().into_iter().collect()
    }
}

/// Write every batch of `rows` into one new IPC file at `path`, in `crate::ipc`'s codec, and
/// answer with the row count the pass observed.
async fn drain(path: &Path, mut rows: SendableRecordBatchStream) -> Result<u64, String> {
    let out = File::create_new(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut writer = FileWriter::try_new_with_options(out, &rows.schema(), ipc_write_options()?)
        .map_err(|e| e.to_string())?;
    let mut count = 0u64;
    while let Some(batch) = rows.next().await {
        let batch = batch.map_err(|e| e.to_string())?;
        writer.write(&batch).map_err(|e| e.to_string())?;
        count += batch.num_rows() as u64;
    }
    writer.finish().map_err(|e| e.to_string())?;
    Ok(count)
}

/// The file name one append lands under. Pid plus wall-clock nanoseconds, because the directory
/// accumulates files across processes and restarts and two appends must never share a name;
/// [`append`](InternalTableStore::append) still refuses rather than overwrites if they somehow do.
fn part_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("part-{}-{nanos}.arrow", process::id())
}

/// The `.tmp-…` directory a spool fills, removed on **every** way out that is not a successful
/// rename — an error, and a **cancel**.
///
/// The cancel is why this is a guard rather than an `if published.is_err()`: a CTAS is registered
/// as the workspace's in-flight call, so `Workspace::cancel` and a re-press both abort the task,
/// and an aborted task's future is *dropped* at its next await — no error path runs. Without
/// this, every cancelled CTAS would leave its partial spool behind, and
/// [`sweep_stale_temp_dirs`](strata_core::util::sweep_stale_temp_dirs) deliberately never touches
/// this process's own directories, so nothing would clear them for the life of the window.
/// Cancelling a large CTAS twice is enough to notice. The snapshot writer has the same rule from
/// the other side (`Workspace::query` retires again once its handle reports cancelled).
struct Staging {
    dir: PathBuf,
    armed: bool,
}

impl Staging {
    fn open(tables: &Path) -> Result<Staging, String> {
        let dir = tables.join(temp_dir_name());
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Staging { dir, armed: true })
    }

    /// The directory was renamed into place, so it is no longer ours to remove.
    fn published(mut self) {
        self.armed = false;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// A directory as the writer and the reader both have to name it: with a trailing separator.
/// Without it `ListingTableUrl::parse` reads the path as a single **file**, which turns a
/// directory sink into one file called `<slug>` and a directory listing into a miss.
pub(crate) fn dir_path(dir: &Path) -> String {
    format!("{}/", dir.display())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::env;

    use datafusion::arrow::array::{ArrayRef, Int32Array};
    use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
    use futures::stream;

    use crate::builder::test_context;

    use super::*;

    /// A scratch tables root of our own, per test — these run concurrently in one process.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_table_store_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new("n", DataType::Int32, true)]))
    }

    fn rows(values: Vec<i32>) -> SendableRecordBatchStream {
        let schema = schema();
        let n: ArrayRef = Arc::new(Int32Array::from(values));
        let batch = RecordBatch::try_new(Arc::clone(&schema), vec![n]).expect("a batch");
        Box::pin(RecordBatchStreamAdapter::new(
            schema,
            stream::iter(vec![Ok(batch)]),
        ))
    }

    /// A directory's entries, sorted.
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// **A cancelled create takes its staging directory with it.** A cancel aborts the task, so
    /// the future is *dropped* mid-await and no error path runs — and the sweep never touches
    /// this process's own `.tmp-` directories, so anything left here would sit under the tables
    /// root for the life of the window. Cancelling a large CTAS a few times is all it takes.
    ///
    /// Driven by dropping the future rather than by racing a real cancel, because that is
    /// exactly what `tokio`'s abort does to it and it is the state under test. The stream never
    /// yields, so the poll parks exactly where an abort would land.
    #[tokio::test]
    async fn a_cancelled_create_takes_its_staging_directory_with_it() {
        let tables = scratch("cancelled");
        let store = LocalIpcTableStore::new_in(&tables);
        let pending: SendableRecordBatchStream =
            Box::pin(RecordBatchStreamAdapter::new(schema(), stream::pending()));

        let mut creating = Box::pin(store.create("big", pending));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            std::future::Future::poll(creating.as_mut(), &mut cx).is_pending(),
            "the spool has started and not finished"
        );
        assert_eq!(entries(&tables).len(), 1, "its staging directory is there");

        drop(creating);

        assert!(
            entries(&tables).is_empty(),
            "and dropping the future removed it: {:?}",
            entries(&tables)
        );
        let _ = fs::remove_dir_all(&tables);
    }

    /// Driven at [`discard`](InternalTableStore::discard) with the *removal* made to fail while
    /// the rename can still land — a read-only directory refuses `unlink` of what it holds, and
    /// the rename needs write on the tables root only. That is the shape of every interruption
    /// this exists for: the rename landed, the walk did not.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_discard_that_cannot_finish_still_takes_the_table_out_of_the_way() {
        use std::os::unix::fs::PermissionsExt;

        let tables = scratch("discard");
        let store = LocalIpcTableStore::new_in(&tables);
        let dir = tables.join("t");
        let locked = dir.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("part-0.arrow"), b"x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();

        store
            .discard("t")
            .await
            .expect("the rename is the operation, and it landed");

        let left = entries(&tables);
        for name in &left {
            let _ = fs::set_permissions(
                tables.join(name).join("locked"),
                fs::Permissions::from_mode(0o700),
            );
        }

        assert!(!dir.exists(), "gone from under the table's own slug");
        assert!(
            !left.is_empty() && left.iter().all(|name| name.starts_with(".tmp-")),
            "and what survives is only ever a temp the sweep collects: {left:?}"
        );
        let _ = fs::remove_dir_all(&tables);
    }

    /// **Residue from an interrupted discard is nobody's problem but the sweep's.** A `.tmp-…`
    /// sibling already in the tables root — exactly what a kill mid-walk leaves — changes
    /// nothing about a later create, read or discard, and the store never touches it: collecting
    /// it is `project::tidy_strata_dir`'s, on the next `.strata` write.
    #[tokio::test]
    async fn a_preseeded_tmp_sibling_disturbs_nothing_and_is_left_for_the_sweep() {
        let tables = scratch("residue");
        let residue = tables.join(".tmp-9999-0");
        fs::create_dir_all(&residue).unwrap();
        fs::write(residue.join("part-0.arrow"), b"x").unwrap();
        let store = LocalIpcTableStore::new_in(&tables);

        let created = store.create("t", rows(vec![1, 2])).await.expect("created");
        assert_eq!(created, 2);

        let ctx = test_context(&BTreeMap::new());
        let provider = store
            .provider(&ctx, "t")
            .await
            .expect("served")
            .expect("held");
        assert_eq!(provider.schema().field(0).name(), "n");

        store.discard("t").await.expect("discarded");
        assert!(residue.is_dir(), "the residue is the sweep's, not ours");
        let _ = fs::remove_dir_all(&tables);
    }

    /// A store following an engine that has no project folder holds nothing and can create
    /// nothing — the arms refuse a create before it can land here, and registration reads the
    /// `None` as "fall back to the def's own paths".
    #[tokio::test]
    async fn a_rootless_following_store_answers_none_and_refuses_a_create() {
        let store = LocalIpcTableStore::following(Arc::new(Mutex::new(None)));
        let ctx = test_context(&BTreeMap::new());

        assert!(store.provider(&ctx, "t").await.expect("answered").is_none());
        let refused = store.create("t", rows(vec![1])).await.expect_err("refused");
        assert!(refused.contains("project folder"), "{refused}");
    }
}
