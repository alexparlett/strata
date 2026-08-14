//! The app-global **update status** (UP-02) — what the updater last learned, and the presses
//! that move it along. The mechanism itself is `strata_core::update`; this is the app's half of
//! it, and the surfaces are UP-03.
//!
//! **Why app-global.** There is one running app to update, so there is one answer to "is there
//! a newer release", and the surfaces that show it live in different windows (the launcher
//! rail, the restart confirm each workspace root mounts). Per window it would be two bugs at
//! once: a check per window against the same endpoint, and a download staged twice.
//!
//! **Not persisted**, on [`Probes`](super::Probes)' reasoning. A check result is a fact about a
//! request made minutes ago, and an "up to date" restored from disk at launch would be a claim
//! nothing had checked. The thing that does outlive the run is the *staged bundle*, which is a
//! file on disk rather than a value in here.
//!
//! **A worker outlives the window that started it.** The blocking calls run on a thread of their
//! own ([`crate::task::offload`]), but a task is bound to its window's root scope and a workspace
//! window can go away mid-job — the launcher closes the moment a project opens. So the worker parks
//! the settled status in [`SETTLED`] and whoever reaches it first takes it, either the awaiting
//! task or the next window to mount ([`use_updates`]). Nothing polls, because there is always a
//! workspace window and its mount is the second wake. That matters most for a download, which ends
//! in a verified bundle on disk that losing the answer would orphan.
//!
//! **A dev build can be pointed at a local server.** The mechanism is inert outside a bundle and
//! there is never a newer release to hand, so the surfaces draw nothing in a `cargo run` — set
//! `STRATA_UPDATE_ORIGIN` (debug builds only, `strata_core::update::local_origin`) and the whole
//! ladder runs for real against `crates/strata-core/examples/fake_releases.rs`. Two things here
//! have to answer differently while it is: [`install_site`], because a dev build has no bundle to
//! light the surfaces up with, and [`install`], because there is nowhere to swap one in.
//!
//! **The install is a quit.** A running app's bundle is never mutated: the press records the swap
//! in [`PENDING`] — a process-global, because it outlives every window and scope — and calls the
//! ordinary [`quit`](crate::platform::quit), so every close confirm keeps its say. A cancelled quit
//! clears the intent through [`abandon_install`], leaving the staged bundle and a `Ready` status.
//! The swap happens in `main`, after `launch` returns and no window is left.

#[cfg(debug_assertions)]
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use freya::prelude::{spawn_forever, use_side_effect, State, WritableUtils};
use futures::channel::mpsc;
use futures::StreamExt;
use strata_core::update::{self, Asset, Check, Site};

use crate::platform::quit;
use crate::state::ConfigStation;
use crate::task::offload;

/// **The running app's version** — the number the check compares against, and the one the
/// launcher rail prints.
///
/// This crate's `CARGO_PKG_VERSION` is *the* number: `scripts/version.sh` bumps it, and the
/// tag, the DMG and the bundle's plist are all derived from it. `strata-core` is versioned
/// independently, which is why `check_blocking` takes it as an argument.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// How much has to arrive before a download moves the status again. A write per network chunk
/// is a repaint per network chunk, which for this archive is thousands of them; a megabyte is
/// finer than the progress bar can show and coarse enough to cost nothing.
const PROGRESS_STEP: u64 = 1 << 20;

/// The app-global status slot — created in `main`, handed to every window root.
pub type UpdateStatus = State<Update>;

/// What the updater last learned. One value for the whole app.
#[derive(Clone, PartialEq, Debug, Default)]
pub enum Update {
    /// Nothing has been asked yet. The rail shows nothing at all for this — a version line, and
    /// no talk of updates.
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// A newer release exists. `asset` is `None` for a release carrying no update archive, in
    /// which case the offer is the page and nothing more.
    Available {
        version: String,
        page_url: String,
        /// What changed, in the release's own Markdown — see [`Asset`]'s neighbour
        /// `update::Offer::notes`. It travels through all three offer states because the check
        /// is the only thing that reads it and the dialog can be raised over any of them.
        notes: String,
        asset: Option<Asset>,
    },
    Downloading {
        version: String,
        page_url: String,
        notes: String,
        got: u64,
        /// What the server declared, which is not always something it knows.
        total: Option<u64>,
    },
    /// Downloaded, unpacked and verified against Apple's chain. `staged` is the bundle that
    /// will take the running one's place.
    Ready {
        version: String,
        page_url: String,
        notes: String,
        staged: PathBuf,
    },
    /// Whatever went wrong, in its own words. Never nagged about — a failed check is a log line
    /// and a quiet status on the rail — but it is reported in the dialog of whoever asked for
    /// the check by name.
    Failed {
        why: String,
    },
}

/// Create the status slot. Call **once**, in `main`, like its neighbours.
pub fn create_global_updates() -> UpdateStatus {
    State::create_global(Update::default())
}

/// **Where this app is installed, and whether it can be replaced there.**
///
/// Answered once per process and kept: the question cannot change under a running app in any
/// way that matters, and answering it writes a probe file into the application folder — which
/// is the only honest way to ask, and not something to do on every repaint.
pub fn install_site() -> &'static Site {
    static SITE: OnceLock<Site> = OnceLock::new();
    SITE.get_or_init(|| match update::is_local() {
        true => Site::Writable(env::temp_dir().join("strata-update-local/Strata.app")),
        false => update::site(),
    })
}

/// **Ask GitHub whether there is a newer release.**
///
/// A no-op while a job is already running, and a no-op once an update is downloading or staged:
/// a check that found something newer would leave a verified bundle nobody asked to discard, and
/// the press that installs the one in hand is right there.
///
/// **`Downloading` stands this down as firmly as `Ready` does**, and the guard alone is not
/// enough for it: a download that has parked its answer but not yet been adopted has released
/// the guard while the status still reads `Downloading`, and a check slipping through there
/// would park over the staged bundle's only path.
pub fn check(status: UpdateStatus) {
    let mut status = status;
    if matches!(
        *status.peek(),
        Update::Downloading { .. } | Update::Ready { .. }
    ) {
        return;
    }
    let Some(working) = Working::start() else {
        return;
    };
    status.set(Update::Checking);

    let job = offload(move || {
        let settled = match update::check_blocking(CURRENT) {
            Ok(Check::UpToDate) => Update::UpToDate,
            Ok(Check::Newer(offer)) => Update::Available {
                version: offer.version,
                page_url: offer.page_url,
                notes: offer.notes,
                asset: offer.asset,
            },
            Err(why) => failed(why),
        };
        park(settled);
        drop(working);
    });
    spawn_forever(async move {
        settle(status, job.await, "The update check could not be run.");
    });
}

/// **Download, unpack and verify the offered update.** A no-op unless the status is an offer
/// carrying an archive.
pub fn download(status: UpdateStatus) {
    let mut status = status;
    let offered = match &*status.peek() {
        Update::Available {
            version,
            page_url,
            notes,
            asset: Some(asset),
        } => (
            version.clone(),
            page_url.clone(),
            notes.clone(),
            asset.clone(),
        ),
        _ => return,
    };
    let (version, page_url, notes, asset) = offered;
    let Some(working) = Working::start() else {
        return;
    };
    status.set(Update::Downloading {
        version: version.clone(),
        page_url: page_url.clone(),
        notes: notes.clone(),
        got: 0,
        total: Some(asset.size),
    });

    let (progress, mut arriving) = mpsc::unbounded();
    let job = offload(move || {
        let mut reported = 0u64;
        let settled = match update::download_blocking(&asset, |got, total| {
            if got == 0 || got - reported >= PROGRESS_STEP || Some(got) == total {
                reported = got;
                let _ = progress.unbounded_send((got, total));
            }
        }) {
            Ok(staged) => Update::Ready {
                version,
                page_url,
                notes,
                staged,
            },
            Err(why) => failed(why),
        };
        park(settled);
        drop(working);
    });

    spawn_forever(async move {
        let shown = async {
            while let Some((got, total)) = arriving.next().await {
                let mut status = status;
                if let Update::Downloading {
                    got: at, total: of, ..
                } = &mut *status.write()
                {
                    *at = got;
                    *of = total;
                }
            }
        };
        let (ran, ()) = futures::future::join(job, shown).await;
        settle(status, ran, "The download could not be run.");
    });
}

/// **Install the staged update**, which is a quit: record what the swap is, then close every
/// window through the ordinary path.
///
/// The status is deliberately left on `Ready`. Nothing has happened yet that a cancelled quit
/// would have to undo — [`abandon_install`] only has to forget the intent — and a status that
/// said "installing" would be a claim about work that has not started and may never.
pub fn install(status: UpdateStatus) {
    let mut status = status;
    let staged = match &*status.peek() {
        Update::Ready { staged, .. } => staged.clone(),
        _ => return,
    };
    if update::is_local() {
        tracing::warn!("not installing {}: it came from a local origin", staged.display());
        return;
    }
    let Site::Writable(target) = install_site() else {
        status.set(failed(
            "Strata cannot be replaced where it is installed. Open the release page to install \
             the update by hand.",
        ));
        return;
    };
    set_pending(Some(Pending {
        staged,
        target: target.clone(),
    }));
    quit();
}

/// **Forget the install a quit was for.** Called by
/// [`end_quit`](crate::platform::end_quit) — which every path that dismisses a close confirm
/// already has to call, so the obligation is one line in one place rather than a rule each
/// dialog has to remember.
///
/// Nothing is lost: the staged bundle is untouched and the status is still `Ready`, so the
/// press can simply be made again.
pub fn abandon_install() {
    set_pending(None);
}

/// **Perform the swap.** Call from `main`, after `launch` has returned: no window exists, the
/// renderer is gone, and the bundle being replaced is not being read.
///
/// A failure is a log line rather than a report, because there is nobody left to report to —
/// and [`update::install`] has already put the old app back, so the relaunch below still finds
/// a working Strata either way. That is also why the relaunch is unconditional: the user asked
/// for a restart.
pub fn install_pending() {
    tracing::debug!("the event loop has ended");
    let Some(pending) = take_pending() else {
        return;
    };
    match update::install(&pending.staged, &pending.target) {
        Ok(()) => tracing::info!("installed the update at {}", pending.target.display()),
        Err(why) => tracing::error!("{why}"),
    }
    update::discard(&pending.staged);
    if let Err(e) = update::relaunch(&pending.target) {
        tracing::error!("{e}");
    }
}

/// **Keep this window in step with the update status, and run the app's one startup check.**
///
/// Call once in a **workspace** window's root — both kinds, for
/// [`use_agent_server`](crate::agent::use_agent_server)'s reason: there is always at least one
/// of them alive, because the launcher takes the last project's place.
///
/// Two jobs, and the first is why this is mounted rather than called from `main`. A window
/// mounting is the second chance a worker's answer gets (see the module doc), so every mount
/// takes whatever is parked before anything else happens.
pub fn use_updates(status: UpdateStatus, config: ConfigStation) {
    use_side_effect(move || {
        reconcile(status);
        if install_site().bundle().is_none() {
            return;
        }
        if !config.peek().settings.check_updates {
            return;
        }
        if CHECKED.swap(true, Ordering::SeqCst) {
            return;
        }
        check(status);
    });
}

/// **Whatever went wrong, recorded where it was learned.** The one way to build
/// [`Update::Failed`], because the rail deliberately draws *nothing* for it (UP-03: the rail
/// never nags and a failed check is not chrome) and the report card is only up if somebody
/// asked — so if this did not log, a refused signature and a finished download would be
/// indistinguishable and there would be nothing to diagnose from. One funnel rather than a `tracing` call remembered at five sites, on the log's own
/// rule: the fact is recorded by whoever observed it.
fn failed(why: impl Into<String>) -> Update {
    let why = why.into();
    tracing::warn!("{why}");
    Update::Failed { why }
}

/// The swap a quit is for.
struct Pending {
    /// The verified bundle a download left in its staging folder.
    staged: PathBuf,
    /// The installed bundle it replaces.
    target: PathBuf,
}

/// Set while a worker thread is running one of the blocking calls. Taken on the render thread
/// *before* the thread exists, so two presses in one frame cannot both get past it, and
/// released on the worker once its answer is parked.
static WORKING: AtomicBool = AtomicBool::new(false);

/// The settled status a worker produced, waiting for a window to take it — see the module doc.
static SETTLED: Mutex<Option<Update>> = Mutex::new(None);

/// The install this quit is for, or none. Process-global because it outlives every window: the
/// swap happens after the event loop has returned.
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// Whether the one startup check has been run in this process.
static CHECKED: AtomicBool = AtomicBool::new(false);

/// Claim [`WORKING`] for one job. `None` means a job is already running, and the caller stands
/// down rather than racing it.
struct Working;

impl Working {
    fn start() -> Option<Working> {
        (!WORKING.swap(true, Ordering::SeqCst)).then_some(Working)
    }
}

impl Drop for Working {
    fn drop(&mut self) {
        WORKING.store(false, Ordering::SeqCst);
    }
}

/// Leave a settled status where whoever is still around can find it. **Before** the [`Working`]
/// guard drops, or a reconcile in between would see no job running and no answer, and call the
/// job lost.
fn park(settled: Update) {
    match SETTLED.lock() {
        Ok(mut slot) => *slot = Some(settled),
        Err(e) => tracing::error!("the update slot is poisoned: {e}"),
    }
}

/// **What a finished job leaves in the slot.** `ran` is [`offload`]'s answer: `None` means the
/// thread never started or it panicked, so nothing was parked and nothing ever will be — and a
/// status left on `Checking` or `Downloading` would sit there for the rest of this window's
/// life, showing a request nobody is making. Neither is a fact about GitHub, so `lost` must not
/// claim to have reached it.
fn settle(status: UpdateStatus, ran: Option<()>, lost: &str) {
    if ran.is_some() {
        adopt(status);
        return;
    }
    let mut status = status;
    status.set(failed(lost));
}

/// Take a parked status into the slot, if there is one.
fn adopt(status: UpdateStatus) {
    let settled = match SETTLED.lock() {
        Ok(mut slot) => slot.take(),
        Err(e) => {
            tracing::error!("the update slot is poisoned: {e}");
            None
        }
    };
    if let Some(settled) = settled {
        let mut status = status;
        status.set(settled);
    }
}

/// Take whatever a worker left, and — if the status still claims a job is in flight when none
/// is — say so rather than leaving it there forever.
///
/// **`WORKING` is read before the adopt, not after.** [`park`] happens before the guard clears
/// it, so a `false` read means whatever the worker had to say is already in [`SETTLED`] and the
/// adopt below will find it. The other order has a window: a worker that finishes between the
/// adopt and the read is seen as "nothing running, nothing parked", and a download that in fact
/// succeeded is reported as lost while its verified bundle sits unreachable.
fn reconcile(status: UpdateStatus) {
    let running = WORKING.load(Ordering::SeqCst);
    adopt(status);
    if running {
        return;
    }
    let mut status = status;
    let lost = match &*status.peek() {
        Update::Checking => "The update check did not finish.",
        Update::Downloading { .. } => "The download did not finish.",
        _ => return,
    };
    status.set(failed(lost));
}

fn set_pending(pending: Option<Pending>) {
    match PENDING.lock() {
        Ok(mut slot) => *slot = pending,
        Err(e) => tracing::error!("the install slot is poisoned: {e}"),
    }
}

fn take_pending() -> Option<Pending> {
    match PENDING.lock() {
        Ok(mut slot) => slot.take(),
        Err(e) => {
            tracing::error!("the install slot is poisoned: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard is what stops two presses in one frame starting two workers, and it has to
    /// release on the worker's own thread — so it is an RAII value rather than a flag somebody
    /// remembers to clear.
    #[test]
    fn only_one_job_runs_at_a_time() {
        let first = Working::start().expect("the first job starts");
        assert!(
            Working::start().is_none(),
            "a second job got past the guard"
        );
        drop(first);
        assert!(Working::start().is_some(), "the guard did not release");
    }

    /// **A parked answer survives the window that asked for it.** Nothing here holds a `State`,
    /// which is the whole point: the worker writes to a process-global and the next window to
    /// mount reads it.
    #[test]
    fn a_settled_status_waits_to_be_taken() {
        park(Update::UpToDate);
        let taken = SETTLED.lock().unwrap().take();
        assert_eq!(taken, Some(Update::UpToDate));
        assert_eq!(SETTLED.lock().unwrap().take(), None, "taken twice");
    }

    /// The intent names **both** ends of the swap, is taken exactly once, and a cancelled quit
    /// clears it — which is the whole of what a cancelled quit has to undo.
    ///
    /// One test rather than three because [`PENDING`] is process-global and the harness runs
    /// tests in parallel: three would race each other rather than assert anything.
    #[test]
    fn the_install_intent_is_set_taken_once_and_abandoned() {
        let staged = PathBuf::from("/tmp/strata-update-x/Strata.app");
        let target = PathBuf::from("/Applications/Strata.app");
        let intent = || {
            Some(Pending {
                staged: staged.clone(),
                target: target.clone(),
            })
        };

        set_pending(intent());
        let taken = take_pending().expect("the intent");
        assert_eq!(taken.staged, staged);
        assert_eq!(taken.target, target);
        assert!(take_pending().is_none(), "the intent survived being taken");

        set_pending(intent());
        abandon_install();
        assert!(take_pending().is_none(), "a cancelled quit kept the intent");
    }
}
