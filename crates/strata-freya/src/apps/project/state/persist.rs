//! **The one place a `.strata` write is reported when it fails** (P4-15).
//!
//! [`write_atomic`](strata_core::util::write_atomic) already guarantees the good half: a failed
//! write leaves the previous file intact and strands no temp, so a failure is never "your project
//! file is corrupt" — always "the file on disk is one revision behind the screen". What was
//! missing is anyone saying so. Every writer's local idiom was
//! `if let Err(e) = … { tracing::error!(…) }`, which is a line in a terminal the user does not
//! have open.
//!
//! [`persisted`] is that line plus the two things it was missing: an **event row**, so the failure
//! is visible where the user looks, and a **`bool`**, so the caller can decline to claim a success
//! that didn't happen. Both matter — P3-13's original version of this was written because a drop
//! whose write failed still logged "Dropped view 'x'", which is the app asserting something it
//! had just failed to make true.
//!
//! **Why it lives in `state/` rather than beside a caller.** It began in
//! `views/workbench/editor/actions.rs` (P3-13, where the save paths needed it), and every writer
//! added since either reached across the tree for it — the drop confirm, and the Configure window
//! from another `apps/` subtree entirely — or, more often, didn't find it and grew its own
//! `tracing::error!` instead. That is how `rename_saved_query` ended up holding this function's
//! exact body **minus the reporting line**, and how the history *append* stayed silent four lines
//! from a Clear that reports. A funnel is only adopted if it sits where every mutation site
//! already looks, which for these is the stores.
//!
//! Deliberately **not** here: writes to a destination the *user* chose. The export window
//! (P4-10) writes where a file dialog pointed it, so "the project is behind" is not what its
//! failure means — see its own footer, and P4-15 build item 5.

use std::collections::BTreeMap;
use std::path::Path;

use freya::prelude::{use_consume, use_provide_context, State};
use strata_core::project as project_io;
use strata_model::{HistoryEntry, SessionSnapshot};

use super::log::{log_event, LogCtx, LogLevel};
use super::project::ProjectState;

/// Which of the project's files a write was aimed at — and **the only place its wording lives**,
/// so the terminal tag and the sentence the user reads cannot drift apart (P4-15 build item 7:
/// the same failure used to read two ways depending on which arm produced it).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ProjectFile {
    /// `.strata/project.json` — the shareable catalog defs.
    Defs,
    /// `.strata/session.json` — the local session (tabs, layout, geometry).
    Session,
    /// `.strata/history.jsonl` — the persisted run history.
    History,
}

impl ProjectFile {
    /// The terse terminal tag.
    fn tag(self) -> &'static str {
        match self {
            Self::Defs => "save project defs",
            Self::Session => "save session",
            Self::History => "save query history",
        }
    }

    /// The noun the user's sentence is built around.
    fn noun(self) -> &'static str {
        match self {
            Self::Defs => "project file",
            Self::Session => "session file",
            Self::History => "query history",
        }
    }

    /// What the Problems row calls it — the file itself, since that row's subject is the file
    /// rather than the mutation that happened to be writing it.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Defs => "project.json",
            Self::Session => "session.json",
            Self::History => "history.jsonl",
        }
    }
}

/// **Which of the project's files are currently behind the screen**, and why (P4-15 item 3).
///
/// The satellite behind the Problems drawer's *Project* tab, and the reason a failed write is a
/// **condition** rather than only an event. An event describes a finished moment and scrolls
/// away; this is the standing fact — the file is behind and stays behind until some later write
/// to it succeeds — so it is what a surface can render for as long as it holds and retract by
/// itself when it stops.
///
/// It is a **third kind** of state, which is worth naming because the two it sits between are
/// already rules of their own. A diagnostic is a *reconciliation*: a pure function of the buffer
/// revision and the catalog epoch, so one driver re-derives it and no entry point needs
/// enumerating. An event is *observed*: it cannot be re-derived at all, because it describes
/// something already finished. A write failure is a **remembered condition** — it takes "clears
/// when it stops holding" from the first and "an observer has to record it" from the second, and
/// that is exactly why it earns its own tab beside the SQL rows rather than being mixed in with
/// them.
#[derive(Default)]
pub struct PersistFaults {
    behind: BTreeMap<ProjectFile, String>,
}

impl PersistFaults {
    /// Record `file` as behind. `true` when this is the **transition** into failure rather than
    /// another instance of one already held.
    ///
    /// The distinction is what keeps the event log usable: the session autosave retries every
    /// 500ms of activity, so a project on a read-only volume would otherwise append an identical
    /// row per debounce and evict the log's whole 200-entry contents within a couple of minutes
    /// of ordinary typing. One condition is one row here, and one event when it starts.
    pub(crate) fn fault(&mut self, file: ProjectFile, why: String) -> bool {
        self.behind.insert(file, why).is_none()
    }

    /// Forget `file`'s fault — a later write to it landed. `true` when one was actually held.
    pub(crate) fn cleared(&mut self, file: ProjectFile) -> bool {
        self.behind.remove(&file).is_some()
    }

    /// Whether `file` is currently behind — peeked before taking a write lock, so the common case
    /// (a write that lands with nothing wrong) never wakes this store's subscribers.
    fn holds(&self, file: ProjectFile) -> bool {
        self.behind.contains_key(&file)
    }

    /// Whether `file` is already recorded as behind **for this same reason**.
    ///
    /// The failure arm's equivalent of [`holds`](Self::holds), and needed for the same reason: a
    /// writer that keeps failing reaches it every 500ms, and re-recording an identical fault
    /// would notify every subscriber — the Problems body, the header strip, the rail badge —
    /// twice a second for as long as the condition lasts. That is the flood the transition-only
    /// event logging exists to prevent, just moved into the render loop.
    ///
    /// Compares the **reason**, not just the key, so a cause that genuinely changes
    /// (`Permission denied` → `No space left on device`) still lands as a fresh fault with its
    /// own event, rather than the row quietly swapping text with no record of when.
    fn unchanged(&self, file: ProjectFile, why: &str) -> bool {
        self.behind.get(&file).is_some_and(|held| held == why)
    }

    /// The files currently behind, in [`ProjectFile`] declaration order, with the error that put
    /// them there — the map is a `BTreeMap` over a derived `Ord`, so its own iteration order is
    /// already the one this wants.
    pub fn rows(&self) -> Vec<(ProjectFile, String)> {
        self.behind
            .iter()
            .map(|(f, why)| (*f, why.clone()))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.behind.len()
    }
}

/// The write-fault satellite in context.
pub type FaultsCtx = State<PersistFaults>;

/// Stand this project's write-fault satellite up and provide it. Call once in the window root,
/// beside [`use_init_log`](super::log::use_init_log) and for the same reason — the scan pass and
/// the autosave both report through it.
pub fn use_init_faults() -> FaultsCtx {
    use_provide_context(|| State::create(PersistFaults::default()))
}

/// **Where a `.strata` write reports**, both halves of it: the event log (when it happened) and
/// the fault satellite (that it is still true).
///
/// One handle rather than two parameters because every writer needs both and neither is useful
/// alone — an event with no condition scrolls away while the file is still behind, and a
/// condition with no event has no record of when it started.
#[derive(Clone, Copy)]
pub struct ReportCtx {
    pub log: LogCtx,
    pub faults: FaultsCtx,
}

/// Gather the window's two reporting handles. A hook, so call it at render time and pass the
/// value into handlers and tasks — the same shape [`LogCtx`] itself is used with, and for the
/// same reason: by the time a writer knows what happened it is inside a spawned task with no
/// scope to read context from.
pub fn use_report() -> ReportCtx {
    ReportCtx {
        log: use_consume::<LogCtx>(),
        faults: use_consume::<FaultsCtx>(),
    }
}

/// Run a `.strata` write, and **record it if it fails**. `true` when it really reached disk.
///
/// The write is passed in rather than performed here because the four families spell it
/// differently (a store projection, a snapshot, an append, a rewrite) while the *reporting* is
/// identical — which is the whole reason this is one function and not four.
///
/// The message deliberately does not name the subject of the mutation ("the view", "this run"):
/// the event that precedes or follows it already does, and a second copy is one that can disagree.
pub fn persisted(
    report: ReportCtx,
    file: ProjectFile,
    write: impl FnOnce() -> Result<(), String>,
) -> bool {
    let mut faults = report.faults;
    // Both arms **peek before writing**, and both bind the answer to a `let` before touching the
    // store. Peeking first is what keeps a repeating writer from waking every subscriber on every
    // attempt (this is the hot path — the autosave lands here every 500ms of activity). Binding
    // first is what keeps the read guard from living across the write on the same
    // `GenerationalBox`, which CLAUDE.md records as a runtime borrow panic.
    match write() {
        Ok(()) => {
            let was_behind = faults.peek().holds(file);
            if was_behind {
                faults.write().cleared(file);
                log_event(
                    report.log,
                    LogLevel::Ok,
                    format!("The {} is being written again", file.noun()),
                );
            }
            true
        }
        Err(e) => {
            tracing::error!("{}: {e}", file.tag());
            // The **transition** is the event; the condition is the row. A repeating writer that
            // logged every attempt would bury every other event in the log, and one that
            // re-recorded an identical fault would re-render the drawer just as often (see
            // `PersistFaults::unchanged`).
            let same_as_held = faults.peek().unchanged(file, &e);
            if !same_as_held {
                faults.write().fault(file, e.clone());
                log_event(
                    report.log,
                    LogLevel::Error,
                    format!("Could not write the {}: {e}", file.noun()),
                );
            }
            false
        }
    }
}

/// The defs write, which every catalog mutation shares — [`persisted`] with the one write that
/// has six call sites folded in, so a mutation site names its channel and its store and not the
/// path to a file.
pub fn persisted_defs(project: &ProjectState, report: ReportCtx) -> bool {
    persisted(report, ProjectFile::Defs, || project.save_defs())
}

/// The session write, shared by the debounced autosave and the final save on close / re-root.
///
/// **A caveat the final save carries and the autosave doesn't:** on the way down, the event row
/// this records lands in a log that is about to be dropped with its window, so it is a real
/// record only in the terminal and only until then. That is not a reason to leave the write
/// silent — a re-root keeps the *window* and only remounts the project, and either way the
/// `tracing` line stops being the only trace of it — but making a dying window's failure
/// genuinely visible needs the standing condition (P4-15 build item 3), not another event.
pub fn persisted_session(root: &Path, snapshot: &SessionSnapshot, report: ReportCtx) -> bool {
    persisted(report, ProjectFile::Session, || {
        project_io::save_session(root, snapshot)
    })
}

/// The history write — an **append** for a query the log doesn't hold, a whole-file **rewrite**
/// for a re-run that moved an existing entry (an append can add a line but not move one).
///
/// Which of the two it is comes from the caller's push; both are the same failure to report.
pub fn persisted_history(
    root: &Path,
    rewrite: Option<&[HistoryEntry]>,
    entry: &HistoryEntry,
    report: ReportCtx,
) -> bool {
    persisted(report, ProjectFile::History, || match rewrite {
        Some(entries) => project_io::save_history(root, entries),
        None => project_io::append_history(root, entry),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::project::state::log::Log;
    use freya::prelude::{rect, IntoElement, State};
    use freya_testing::TestingRunner;

    /// A scratch project root of this test's own.
    ///
    /// `env::temp_dir()` + **pid**, matching `strata_core::project`'s convention and
    /// `drop_confirm`'s, for the reason they give: the OS temp dir is machine-shared and this
    /// repo builds in several worktrees at once, so a hardcoded path collides between parallel
    /// test binaries. It matters more here than usual — the failing half works by chmod'ing the
    /// directory, so a collision is two tests fighting over one directory's mode bits.
    fn temp_root(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("strata-persist-{tag}-{}", std::process::id()))
    }

    /// Run one write through the funnel and hand back what it answered and what it recorded.
    ///
    /// A `LogCtx` is a `State`, so it can only be *created* inside a Freya scope — hence a
    /// runner, even though nothing here renders. The setup hook is that scope, and it returns
    /// its value, so the whole probe fits in it and no component has to exist to host the write.
    fn probe(write: impl FnOnce(ReportCtx) -> bool) -> Probe {
        fn nothing() -> impl IntoElement {
            rect()
        }
        let (_runner, out) = TestingRunner::new(
            nothing,
            (10., 10.).into(),
            move |r| {
                let report = ReportCtx {
                    log: r.provide_root_context(|| State::create(Log::default())),
                    faults: r.provide_root_context(|| State::create(PersistFaults::default())),
                };
                let landed = write(report);
                Probe {
                    landed,
                    events: report
                        .log
                        .peek()
                        .events()
                        .map(|e| (e.level, e.message.clone()))
                        .collect(),
                    behind: report.faults.peek().rows(),
                }
            },
            1.0,
        );
        out
    }

    /// Just the files a probe left behind, which is what most assertions here are about.
    fn files(p: &Probe) -> Vec<ProjectFile> {
        p.behind.iter().map(|(f, _)| *f).collect()
    }

    /// What one run through the funnel produced: its answer, the events it appended, and the
    /// files it left standing as behind.
    struct Probe {
        landed: bool,
        events: Vec<(LogLevel, String)>,
        behind: Vec<(ProjectFile, String)>,
    }

    /// Make `.strata/` unwritable, run `f`, put the mode back — the portable way to fail a write
    /// (as `strata_core::util`'s own `write_atomic` tests do). Unix-only, because that is where
    /// the mode bits mean this.
    #[cfg(unix)]
    fn while_read_only<T>(root: &std::path::Path, f: impl FnOnce() -> T) -> T {
        use std::os::unix::fs::PermissionsExt;
        let strata = project_io::strata_dir(root);
        std::fs::create_dir_all(&strata).unwrap();
        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o500)).unwrap();
        let out = f();
        std::fs::set_permissions(&strata, std::fs::Permissions::from_mode(0o700)).unwrap();
        out
    }

    fn entry(sql: &str) -> HistoryEntry {
        HistoryEntry {
            sql: sql.into(),
            ts_ms: 1,
            elapsed_ms: 2,
            rows: 3,
        }
    }

    /// The session write P4-14 left reporting through `tracing` alone: a failure is now an
    /// **event**, and answers `false` — which is what makes the autosave decline to record the
    /// The session write P4-14 left reporting through `tracing` alone: a failure is now an
    /// **event** *and* a held condition, and answers `false` — which is what makes the autosave
    /// decline to record the snapshot as written, so the next change tries again instead of
    /// believing the file current.
    #[cfg(unix)]
    #[test]
    fn a_session_write_that_fails_is_an_event_and_says_so() {
        let root = temp_root("session");
        let p = probe(|report| {
            while_read_only(&root, || {
                persisted_session(&root, &Default::default(), report)
            })
        });

        assert!(!p.landed, "a write into a read-only dir did not fail");
        assert_eq!(p.events.len(), 1, "one event: {:?}", p.events);
        assert_eq!(p.events[0].0, LogLevel::Error);
        assert!(
            p.events[0]
                .1
                .starts_with("Could not write the session file: "),
            "unexpected message: {}",
            p.events[0].1
        );
        // And the condition is *held* — the half an event can't carry, and the whole reason the
        // Problems drawer's Project tab can show it for as long as it lasts.
        assert_eq!(files(&p), [ProjectFile::Session]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The history **append** — the writer that sat four lines from a Clear which already
    /// reported, and stayed silent because it predated the funnel.
    #[cfg(unix)]
    #[test]
    fn a_history_append_that_fails_is_an_event_and_says_so() {
        let root = temp_root("history");
        let e = entry("select 1");
        let p =
            probe(|report| while_read_only(&root, || persisted_history(&root, None, &e, report)));

        assert!(!p.landed);
        assert_eq!(p.events.len(), 1, "one event: {:?}", p.events);
        assert_eq!(p.events[0].0, LogLevel::Error);
        assert!(
            p.events[0]
                .1
                .starts_with("Could not write the query history: "),
            "unexpected message: {}",
            p.events[0].1
        );
        assert_eq!(files(&p), [ProjectFile::History]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The **rewrite** arm (a re-run that moved an entry) fails and reads identically — the two
    /// arms are one failure to the user, which is why they share a [`ProjectFile`] rather than
    /// each naming the call that produced them.
    #[cfg(unix)]
    #[test]
    fn a_history_rewrite_that_fails_reports_the_same_way() {
        let root = temp_root("history-rewrite");
        let e = entry("select 1");
        let all = [entry("select 1"), entry("select 2")];
        let p = probe(|report| {
            while_read_only(&root, || persisted_history(&root, Some(&all), &e, report))
        });

        assert!(!p.landed);
        assert!(
            p.events[0]
                .1
                .starts_with("Could not write the query history: "),
            "unexpected message: {}",
            p.events[0].1
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The flood guard.** A repeating writer fails over and over — the session autosave retries
    /// every 500ms of activity — and that must be *one* event and *one* condition, not one event
    /// per attempt. Without this the 200-entry log fills with identical rows within a couple of
    /// minutes of typing on a read-only volume, evicting every registration failure, drop record
    /// and run settle it held.
    #[cfg(unix)]
    #[test]
    fn a_writer_that_keeps_failing_logs_once_and_holds_the_condition() {
        let root = temp_root("flood");
        let p = probe(|report| {
            while_read_only(&root, || {
                let mut last = true;
                for _ in 0..20 {
                    last = persisted_session(&root, &Default::default(), report);
                }
                last
            })
        });

        assert!(!p.landed);
        assert_eq!(
            p.events.len(),
            1,
            "twenty failures, one event — got {:?}",
            p.events
        );
        assert_eq!(files(&p), [ProjectFile::Session], "and one condition");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Recovery is the other half: a later write that lands **retracts** the condition and says
    /// so once. Without the retraction the row would sit there claiming the project is behind
    /// long after it stopped being true — a surface disagreeing with its store, which is the
    /// defect this whole task exists to remove.
    #[cfg(unix)]
    #[test]
    fn a_write_that_lands_afterwards_retracts_the_condition() {
        let root = temp_root("recover");
        project_io::scaffold(&root).unwrap();
        let p = probe(|report| {
            while_read_only(&root, || {
                persisted_session(&root, &Default::default(), report)
            });
            // The directory is writable again by here.
            persisted_session(&root, &Default::default(), report)
        });

        assert!(p.landed, "the second write should have landed");
        assert!(p.behind.is_empty(), "the condition should be retracted");
        assert_eq!(
            p.events.len(),
            2,
            "the failure and the recovery, once each: {:?}",
            p.events
        );
        assert_eq!(p.events[1].0, LogLevel::Error, "newest first: the failure");
        assert_eq!(p.events[0].0, LogLevel::Ok, "and the recovery on top");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A write that lands records **nothing**. The funnel is a failure reporter, not an audit
    /// trail: every caller already logs its success in the mutation's own words ("Saved view
    /// 'x'"), and a row here saying a file was written would be the stacked near-duplicate
    /// AGENTS.md §3 rules out.
    #[test]
    fn a_write_that_lands_is_not_an_event() {
        let root = temp_root("ok");
        project_io::scaffold(&root).unwrap();
        let p = probe(|report| persisted_session(&root, &Default::default(), report));

        assert!(p.landed);
        assert!(p.events.is_empty(), "a successful write is not an event");
        assert!(p.behind.is_empty(), "and leaves nothing behind");
        let _ = std::fs::remove_dir_all(&root);
    }
}
