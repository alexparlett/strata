//! The default store: one **Arrow IPC** file per snapshot, in a claimed directory under the
//! machine's temp root.
//!
//! IPC rather than parquet because the snapshot is the boundary every result crosses, and
//! parquet's type system is narrower than Arrow's: it cannot write a union at all
//! (`arrow_to_parquet_schema` **panics**, ARROW-8817) nor a zero-field struct, so results had to
//! be coerced on the way in and the record view and JSON/CSV export then read the coerced form.
//! IPC round-trips anything the engine can emit. Compressed (see `crate::ipc`) it is
//! the same size on disk as the parquet it replaced.
//!
//! What lives here is the *filesystem* side of a snapshot's life: the per-store directory, the
//! lock that marks it live, and the startup sweep of everything no live store still holds.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::ipc::writer::FileWriter;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::catalog::TableProvider;
use datafusion::common::Column;
use datafusion::datasource::listing::{ListingTable, ListingTableConfig, ListingTableUrl};
use datafusion::execution::options::{ArrowReadOptions, ReadOptions};
use datafusion::prelude::{col, SessionContext};
use datafusion::sql::TableReference;
use strata_model::SnapshotId;

use crate::ipc::ipc_write_options;
use crate::snapshots::name::snapshot_name;
use crate::snapshots::ordinal::{ordinal_schema, with_ordinal};
use crate::snapshots::{SnapshotSink, SnapshotStats, SnapshotStore};

/// The scope-id allocator: what makes two stores in one process name different directories.
static SCOPE_SEQ: AtomicU64 = AtomicU64::new(0);

/// The roots this process has already swept, and the guard that makes sweeping and claiming
/// mutually exclusive.
///
/// Both jobs at once, because they are one rule. A claim is lock-then-`mkdir`
/// ([`claim_dir`]), so a sweep that lands between a *concurrent* claim's `open` and its
/// `try_lock` takes the very lock that claimer is about to hold, deletes the lock file, and
/// leaves a live directory with nothing defending it — after which the next sweep deletes a
/// running engine's results, which is the exact bug the lock exists to prevent. Serializing
/// them keeps that window shut for every store in this process; a second *process* mid-claim
/// is the same narrow window the sweep has always had, and is why it runs once, as early as
/// this process has anywhere to put a snapshot at all.
static SWEPT: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Prefix of every store directory (and of its lock file) — what [`purge_root`] treats
/// as "ours" and everything else as a stray.
const DIR_PREFIX: &str = "e_";

/// Suffix of a store directory's lock file (`e_<pid>_<scope>.lock`).
const LOCK_SUFFIX: &str = ".lock";

/// One file per snapshot, under a directory this store claims for its whole life.
///
/// The claim is the point: a directory whose lock nobody holds belonged to a process that is
/// gone, and the sweep in `purge_root` is what reclaims it at the next start.
pub struct LocalIpcSnapshotStore {
    /// The shared root every store's directory sits under.
    root: PathBuf,
    /// This store's own directory under [`root`](Self::root).
    dir: PathBuf,
    /// The exclusive lock on [`dir`](Self::dir), held open for the store's whole life: it is
    /// what tells *another* process's sweep that these snapshots are live. Never read —
    /// closing it is the entire contract, so it drops with the store, after `Drop` has removed
    /// the file it guards.
    _lock: Option<File>,
    /// The ordinal each settled snapshot was written with, which is what
    /// [`open`](SnapshotStore::open) declares the file's sort order from.
    settled: Arc<Mutex<HashMap<SnapshotId, Option<String>>>>,
}

impl LocalIpcSnapshotStore {
    /// A store under the machine's shared snapshot root (`<tmp>/strata_snapshots`).
    ///
    /// Claiming the directory can fail — an unwritable temp root, essentially — and that is not
    /// fatal: the store still works (the directory is created on demand), but its snapshots are
    /// unprotected against another instance's sweep, so the reason is logged alongside that
    /// consequence. A claim that fails the same way on every start is a standing risk, not a
    /// transient one, and has to be legible.
    pub fn new() -> Self {
        Self::new_in(snapshots_root())
    }

    /// A store under `root` rather than the shared one — for an embedder that keeps its result
    /// spool somewhere of its own, and for a test that must not touch the machine-shared root.
    ///
    /// The **first** store under a given root sweeps it before claiming anything of its own
    /// (`SWEPT`) — which is the moment this process first has somewhere to put a snapshot, and
    /// so the last moment at which it has none of its own to lose.
    pub fn new_in(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let name = dir_name();
        let dir = root.join(&name);
        let claimed = {
            let mut swept = SWEPT.lock().unwrap();
            if swept.insert(root.clone()) {
                purge_root(&root);
            }
            claim_dir(&root, &name)
        };
        let lock = match claimed {
            Ok(lock) => Some(lock),
            Err(why) => {
                tracing::warn!(
                    "snapshot store: could not claim {} ({why}); its snapshots are \
                     unprotected against another instance's sweep",
                    dir.display()
                );
                None
            }
        };
        Self {
            root,
            dir,
            _lock: lock,
            settled: Arc::default(),
        }
    }

    /// Where this store spools — the directory it claimed.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// This snapshot's file, whether or not it exists yet.
    fn file(&self, id: SnapshotId) -> PathBuf {
        self.dir.join(format!("s_{id}.arrow"))
    }
}

impl Default for LocalIpcSnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LocalIpcSnapshotStore {
    /// Give up the claim: the lock file, and then — as the field drops right after — the lock
    /// itself, so no other process can take the name mid-delete. What was *in* the directory is
    /// [`purge_orphans`](SnapshotStore::purge_orphans)'s, which the engine calls on its way out;
    /// a directory left standing with no lock beside it is what the next start's sweep reclaims.
    fn drop(&mut self) {
        let _ = fs::remove_file(lock_path(&self.root, &self.dir));
    }
}

#[async_trait]
impl SnapshotStore for LocalIpcSnapshotStore {
    fn begin(
        &self,
        id: SnapshotId,
        schema: SchemaRef,
        ord: Option<String>,
    ) -> Result<Box<dyn SnapshotSink>, String> {
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let ord_schema = ord.as_deref().map(|ord| ordinal_schema(&schema, ord));
        let stats = SnapshotStats::new(&schema, ord);
        let out = File::create(self.file(id)).map_err(|e| e.to_string())?;
        let written_schema = ord_schema.clone().unwrap_or(schema);
        let writer = FileWriter::try_new_with_options(out, &written_schema, ipc_write_options()?)
            .map_err(|e| e.to_string())?;
        Ok(Box::new(IpcSink {
            id,
            writer,
            ord_schema,
            rows: 0,
            stats,
            settled: Arc::clone(&self.settled),
        }))
    }

    async fn open(
        &self,
        ctx: &SessionContext,
        id: SnapshotId,
    ) -> Result<Arc<dyn TableProvider>, String> {
        let ord = self
            .settled
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("snapshot {id} has not settled"))?;
        let listing = ArrowReadOptions::default()
            .to_listing_options(&ctx.copied_config(), ctx.copied_table_options());
        let listing = match &ord {
            Some(ord) => listing
                .with_file_sort_order(vec![vec![
                    col(Column::from_name(ord.clone())).sort(true, false)
                ]]),
            None => listing,
        };
        let path = self.file(id).to_string_lossy().into_owned();
        let url = ListingTableUrl::parse(&path)
            .map_err(|e| e.to_string())?
            .with_table_ref(TableReference::bare(snapshot_name(id)));
        let schema = listing
            .infer_schema(&ctx.state(), &url)
            .await
            .map_err(|e| e.to_string())?;
        let config = ListingTableConfig::new(url)
            .with_listing_options(listing)
            .with_schema(schema);
        let table = ListingTable::try_new(config)
            .map_err(|e| e.to_string())?
            .with_cache(ctx.runtime_env().cache_manager.get_file_statistic_cache());
        Ok(Arc::new(table))
    }

    fn retire(&self, id: SnapshotId) {
        self.settled.lock().unwrap().remove(&id);
        let _ = fs::remove_file(self.file(id));
    }

    /// Delete every snapshot file outside `live`; an **empty** `live` takes the directory with
    /// them, which is what an engine on its way out means.
    ///
    /// The lock file is untouched either way — giving up the claim is `Drop`'s, and a store that
    /// is merely shedding snapshots is still holding its directory's name.
    fn purge_orphans(&self, live: &HashSet<SnapshotId>) {
        if live.is_empty() {
            self.settled.lock().unwrap().clear();
            let _ = fs::remove_dir_all(&self.dir);
            return;
        }
        self.settled
            .lock()
            .unwrap()
            .retain(|id, _| live.contains(id));
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            match spool_id(&name) {
                Some(id) if !live.contains(&id) => remove_any(&entry.path()),
                Some(_) => {}
                None => remove_any(&entry.path()),
            }
        }
    }

    /// The **shared root**, not this store's own claimed directory — because the fence is about
    /// where snapshot bytes live, and every store in every process claims a sibling directory
    /// under the same root ([`purge_root`]). A write landing in another store's directory is
    /// read back as a result by whichever process holds that claim.
    fn owned_storage(&self) -> Vec<PathBuf> {
        vec![self.root.clone()]
    }
}

/// One snapshot's IPC write pass.
struct IpcSink {
    id: SnapshotId,
    writer: FileWriter<File>,
    /// The written schema when there is an ordinal — the shape [`with_ordinal`] builds against.
    ord_schema: Option<SchemaRef>,
    /// How many rows are already spooled, which is what the next batch's ordinal counts from.
    rows: u64,
    stats: SnapshotStats,
    settled: Arc<Mutex<HashMap<SnapshotId, Option<String>>>>,
}

impl SnapshotSink for IpcSink {
    fn write(&mut self, batch: &RecordBatch) -> Result<(), String> {
        let spooled = match &self.ord_schema {
            Some(schema) => &with_ordinal(batch, schema, self.rows)?,
            None => batch,
        };
        self.writer.write(spooled).map_err(|e| e.to_string())?;
        self.stats.observe(batch);
        self.rows += batch.num_rows() as u64;
        Ok(())
    }

    fn settle(mut self: Box<Self>) -> Result<SnapshotStats, String> {
        self.writer.finish().map_err(|e| e.to_string())?;
        self.settled
            .lock()
            .unwrap()
            .insert(self.id, self.stats.ord.clone());
        Ok(self.stats)
    }
}

/// The shared root every [`LocalIpcSnapshotStore::new`] store sits under.
///
/// `pub(crate)` because `COPY` refuses a target inside it (`export::refuse_owned_target`): a
/// stray file under a snapshot's directory is read back as a result.
pub(crate) fn snapshots_root() -> String {
    let mut d = env::temp_dir();
    d.push("strata_snapshots");
    d.to_string_lossy().into_owned()
}

/// The name of one store's subdirectory under the shared root. Scoped by **pid + a
/// process-local counter**: the counter is only process-unique, and the snapshot root in the
/// OS temp dir is machine-shared — without the pid, two concurrent processes (a second app
/// instance, parallel test binaries) both allocate `e_0`, `e_1`, … and would write into
/// each other's directory. (Not *deleting* each other's: that's the lock's job, see
/// [`claim_dir`] — a pid can be recycled by a later process, so the name alone
/// never proves the owner is alive.)
fn dir_name() -> String {
    format!(
        "{DIR_PREFIX}{}_{}",
        process::id(),
        SCOPE_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// The snapshot id a spool file's name carries, or `None` for anything that is not one.
fn spool_id(name: &str) -> Option<SnapshotId> {
    name.strip_prefix("s_")?
        .strip_suffix(".arrow")?
        .parse()
        .ok()
        .map(SnapshotId)
}

/// Claim the directory `name` under `root` for a store's whole lifetime. The returned
/// [`File`] **is** the claim: it holds an exclusive advisory lock that the OS releases
/// when the handle closes — on a clean drop, and on a crash for free — and
/// [`purge_root`] skips every directory whose lock it cannot take.
///
/// The lock file sits *beside* the directory, and the order here is the guarantee: lock
/// first, `mkdir` second. So any directory a concurrent purge can see already has a held
/// lock, and there is no window in which a starting store's directory looks abandoned.
/// (A lock file *inside* the directory would have exactly that window.)
///
/// `Err` **carries why**, because the caller reports it: see [`LocalIpcSnapshotStore::new`].
fn claim_dir(root: &Path, name: &str) -> Result<File, String> {
    fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let path = root.join(format!("{name}{LOCK_SUFFIX}"));
    let lock = match claim_lock(&path) {
        Claim::Taken(lock) => lock,
        Claim::Held => return Err(format!("{} is held by another handle", path.display())),
        Claim::Unknown(e) => return Err(format!("{}: {e}", path.display())),
    };
    let dir = root.join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(lock)
}

/// Sweep `root` of **dead** stores' leftovers: every directory whose lock we can take (nobody
/// holds it ⇒ its process is gone), plus any entry that isn't one of our directory/lock pairs at
/// all. A directory still locked by a live store — this app's other instance, a parallel test
/// binary — is left alone.
///
/// That skip is the whole point: the pid-scoped naming ([`dir_name`]) keeps two
/// processes out of each other's *files*, and a blanket `remove_dir_all` of the root
/// defeated it by deleting the other instance's live snapshots — after which every
/// uncached page read there fails.
///
/// Run by the first [`LocalIpcSnapshotStore::new_in`] under a root rather than by whoever
/// starts the process: a spool only this store knows the shape of is not something an app can
/// be asked to remember to sweep, and there is no earlier moment to do it in than the one where
/// a store first exists. Once per root and never beside a claim — see `SWEPT`.
///
/// **Nothing is deleted on a guess.** Only [`Claim::Taken`] proves an owner is gone; both
/// "a live store holds it" and "we couldn't tell" leave the directory standing, because
/// the failure mode of guessing wrong is deleting a running instance's results — the very
/// bug the lock was added to fix — while the failure mode of guessing right is temp files
/// the OS reaper eventually collects. The two are not the same event, though, so the
/// indeterminate one is logged: a sweep that can never resolve anything (an unwritable
/// root, a filesystem with no working `flock`) would otherwise do nothing, forever,
/// silently.
fn purge_root(root: &Path) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(
                "snapshot purge: cannot read {} ({e}); no dead store's spool files under \
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
        let lock = root.join(format!("{name}{LOCK_SUFFIX}"));
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

/// The lock file guarding `dir`, as a sibling of it.
fn lock_path(root: &Path, dir: &Path) -> PathBuf {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    root.join(format!("{name}{LOCK_SUFFIX}"))
}

/// What trying to take a lock told us. The three outcomes are **not** interchangeable:
/// [`Held`](Claim::Held) is routine contention with a live store, while
/// [`Unknown`](Claim::Unknown) means the liveness test itself is unavailable — the same
/// skip, but a reportable condition rather than an expected one (see [`purge_root`]).
enum Claim {
    /// We hold it. Nothing else did, so whatever owned this name is gone.
    Taken(File),
    /// Somebody else holds it — a live store, in this or another process.
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

    /// A store directory with one snapshot file in it.
    fn store_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).expect("store dir");
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

    fn lock_of(root: &Path, name: &str) -> PathBuf {
        root.join(format!("{name}{LOCK_SUFFIX}"))
    }

    #[test]
    fn purge_sweeps_dead_stores_and_spares_live_ones() {
        let root = scratch_root("mixed");

        let live = store_dir(&root, "e_1_0");
        let held = take(&lock_of(&root, "e_1_0"));

        let dead = store_dir(&root, "e_2_0");
        drop(take(&lock_of(&root, "e_2_0")));

        let lockless = store_dir(&root, "e_3_0");

        let orphan = lock_of(&root, "e_4_0");
        drop(take(&orphan));
        let stray = root.join("garbage.txt");
        fs::write(&stray, b"junk").expect("stray file");

        purge_root(&root);

        assert!(
            live.join("s_1.arrow").exists(),
            "a live store's snapshots must survive another instance's sweep"
        );
        assert!(lock_of(&root, "e_1_0").exists(), "…and so must its lock");
        assert!(!dead.exists(), "a dead store's directory goes");
        assert!(!lock_of(&root, "e_2_0").exists(), "…and its lock with it");
        assert!(!lockless.exists(), "an unlocked directory is nobody's");
        assert!(!orphan.exists(), "an orphan lock is swept too");
        assert!(!stray.exists(), "so is anything that isn't ours");

        drop(held);
        let _ = fs::remove_dir_all(&root);
    }

    /// The purge skips a directory either way, but the two reasons are different events:
    /// contention with a live store is routine, an unusable lock means the sweep can
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

    /// The first store under a root sweeps what nobody holds, two stores claim two
    /// directories, and a store's teardown takes its directory and its lock and nothing else.
    #[test]
    fn a_claimed_directory_is_the_claiming_stores_alone() {
        let root = scratch_root("claims_are_exclusive");
        let abandoned = store_dir(&root, "e_0_0");

        let first = LocalIpcSnapshotStore::new_in(&root);
        assert!(
            !abandoned.exists(),
            "the first store under a root reclaims one nobody holds"
        );
        fs::write(first.dir().join("s_1.arrow"), b"snapshot").expect("snapshot file");

        let second = LocalIpcSnapshotStore::new_in(&root);
        assert_ne!(first.dir(), second.dir(), "two stores, two directories");
        assert!(
            first.dir().join("s_1.arrow").exists(),
            "and a later store leaves what a live one is holding alone"
        );

        let dir = first.dir().to_path_buf();
        let lock = lock_path(&root, &dir);
        first.purge_orphans(&HashSet::new());
        assert!(!dir.exists(), "an empty live set discards the directory");
        assert!(lock.exists(), "…and leaves the claim standing");
        drop(first);
        assert!(!lock.exists(), "which the store gives up when it drops");

        drop(second);
        let _ = fs::remove_dir_all(&root);
    }

    /// A live set spares what it names and nothing else — including a stray that is not a
    /// spool file at all, which is what an aborted write can leave behind.
    #[test]
    fn purge_orphans_keeps_only_what_is_live() {
        let root = scratch_root("orphans");
        let store = LocalIpcSnapshotStore::new_in(&root);
        for name in ["s_1.arrow", "s_2.arrow", "s_3.arrow", "junk.txt"] {
            fs::write(store.dir().join(name), b"x").expect("spool file");
        }

        store.purge_orphans(&HashSet::from([SnapshotId(2)]));

        assert!(store.dir().join("s_2.arrow").exists(), "the live one stays");
        assert!(!store.dir().join("s_1.arrow").exists());
        assert!(!store.dir().join("s_3.arrow").exists());
        assert!(!store.dir().join("junk.txt").exists(), "so does a stray");

        drop(store);
        let _ = fs::remove_dir_all(&root);
    }
}
