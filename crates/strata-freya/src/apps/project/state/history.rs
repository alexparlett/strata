//! The per-window **query-history** satellite (P4-14 · state-arch §8).
//!
//! History is its own small store — **not** on `ProjectState` (shared/committed) and not on
//! `SessionState` (the tabs) — persisted locally to `.strata/history.jsonl` (gitignored; IO in
//! `strata-core::project`). The Run path publishes into it; the drawer's History tab (P3-14,
//! `views::drawer::history`) reads it, and its **Clear** ([`clear_history`]) is the one thing
//! that unwrites it.
//!
//! **A list of queries, not of presses.** Re-running a query moves its entry to the top with the
//! newest figures rather than stacking a second row, keyed by
//! [`collapse_sql`](strata_core::util::collapse_sql) — the same normalization the drawer's
//! preview uses, so no two rows can render identically. Dedupe happens **before** the cap, here
//! and in `load_history`, which is the load-bearing part: one query pressed a hundred times has
//! to cost one slot of `max_history`, not all of them. The log stays append-mostly — a new query
//! is one `O_APPEND` line, and only a run that *replaced* an entry rewrites the file, since an
//! append can add a line but not move one (see [`record_run`]).
//!
//! **Only successful runs are recorded** — a run that settled `Ok`, whether it returned rows or
//! performed a statement (ED-02: a typed `CREATE TABLE` is as much a query you may want back as
//! the `SELECT` inside it). A failed / cancelled run (settles `Err`) and an Explain (settles
//! `Plan`) never reach here, so history stays a log of queries that actually did something. The
//! success-only rule is what the drawer's *absence* of a status mark rests on, and it is
//! unchanged. The recorder is the
//! tab's request keeper (`views::keeper`), not the results pane: the keeper stays
//! mounted while the tab is backgrounded, so the settle is observed — and timestamped — at
//! real completion time, and a run whose tab is never revisited still records. (A settle
//! landing in the same update pass that unmounts its pin — a supersede at the instant of
//! completion — goes unrecorded; the pin's side effect never gets to run.) Recording is
//! deduped by the run's [`RunId`], so a second observer of the same settled run (or a
//! re-mounted one re-serving the *cached* result) can never re-log it.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use freya::prelude::{spawn, use_consume, use_side_effect, use_state, State, WritableUtils};
use freya::query::{QueryStateData, UseQuery};
use freya::radio::{use_radio_station, RadioStation};
use strata_core::config::HISTORY_MIN;
use strata_core::project as project_io;
use strata_core::util::{collapse_sql, now_ms};
use strata_model::HistoryEntry;

use crate::apps::project::query::{QueryOutcome, RunId, RunQuery};
use crate::state::{use_config_station, ConfigStation};

use super::log::{log_event, LogCtx, LogLevel};
use super::persist::{persisted_history, use_report, ReportCtx};
use super::{ProjChan, ProjectState};

/// How many recent runs are kept, in memory and on disk: the user's `max_history` setting
/// (design24 System ▸ History), which is the *only* source — a second constant here would
/// silently outrank the control Settings ▸ System puts on it.
///
/// Floored at [`HISTORY_MIN`], the same floor that field offers, because a `0` (only
/// reachable by hand-editing the config) would make the next load rotate `history.jsonl`
/// down to nothing — a setting shouldn't delete the log.
pub fn history_cap(config: ConfigStation) -> usize {
    config.peek().settings.max_history.max(HISTORY_MIN)
}

/// The window's query-history satellite: recent runs newest-first, plus a dedup guard so a
/// results-pane re-mount can't re-log a run.
pub struct History {
    pub entries: VecDeque<HistoryEntry>,
    seen: HashSet<RunId>,
}

impl History {
    /// Load the persisted history for `root` (newest `cap` entries — see
    /// [`history_cap`]) into a fresh satellite. A load error logs and yields an empty
    /// history rather than blocking the window — history is regenerable, unlike the project.
    pub fn load(root: &Path, cap: usize) -> Self {
        let loaded = project_io::load_history(root, cap).unwrap_or_else(|e| {
            tracing::error!("load history: {e}");
            Vec::new()
        });
        // File order is oldest → newest; the satellite holds newest-first.
        Self {
            entries: loaded.into_iter().rev().collect(),
            seen: HashSet::new(),
        }
    }

    /// Record `run` newest-first, trimming the in-memory window to `cap`.
    ///
    /// **Two different repeats, deduped for two different reasons.** The same *run* re-served
    /// after a re-mount never gets here at all — the caller peeks [`seen`](Self::seen) first, so
    /// reaching this point means the run is genuinely new. The same *query* run again does get
    /// here, and replaces its earlier entry rather than stacking a second row: history is a list
    /// of the queries you have run, not of the times you pressed Run, and a list that reads
    /// `select * from events` seven times has lost the seven other queries it could have been
    /// showing. The survivor is this one — newest figures, newest timestamp, at the top.
    ///
    /// The key is [`collapse_sql`], the same normalization the drawer's preview uses, so no two
    /// rows can render identically and no two rows a reader can tell apart are merged.
    ///
    /// `cap` is passed per call rather than stored: it is a live setting, so lowering it
    /// takes effect on the next recorded run instead of waiting for the window to reopen.
    ///
    /// Returns whether an earlier entry was **replaced**, which is what decides how the log is
    /// written: an append can add a line but not move one, so a replacement leaves a stale line
    /// behind and the caller has to rewrite instead ([`record_run`]).
    fn push(&mut self, run: RunId, entry: HistoryEntry, cap: usize) -> bool {
        self.seen.insert(run);
        let key = collapse_sql(&entry.sql);
        let before = self.entries.len();
        self.entries.retain(|e| collapse_sql(&e.sql) != key);
        let replaced = self.entries.len() != before;
        self.entries.push_front(entry);
        while self.entries.len() > cap {
            self.entries.pop_back();
        }
        replaced
    }

    /// The satellite in **file order** (oldest → newest) — what [`save_history`] persists.
    ///
    /// [`save_history`]: strata_core::project::save_history
    fn file_order(&self) -> Vec<HistoryEntry> {
        self.entries.iter().rev().cloned().collect()
    }

    /// Drop every recorded run — the History drawer's **Clear** (P3-14), the in-memory half of
    /// [`clear_history`].
    ///
    /// [`seen`](Self::seen) is deliberately **kept**: it is the dedup guard for runs, not a copy
    /// of what is on screen. Forgetting a cleared run would let the pin that is still holding it
    /// re-record it on its next render, putting back an entry the user just cleared.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// The history satellite in context.
pub type HistoryCtx = State<History>;

/// Drive history recording for a press: record it once when it settles `Ok` — rows, or an
/// intercepted statement. A failed / cancelled `Err` or an Explain `Plan` never records,
/// so bad queries stay out of history; the local `recorded` flag stops a re-record on
/// later re-renders of the same mount, and cross-mount dedup lives in the store, keyed by
/// `run`. Call once per `RequestPin` (`views::keeper` — keyed by the press's nonce, and
/// mounted for the press's whole life, so a background tab's settle still records; the one
/// unrecorded edge is a settle landing in the same update pass that unmounts the pin).
pub fn use_history_recording(query: UseQuery<RunQuery>, run: RunId, sql: String) {
    let history = use_consume::<HistoryCtx>();
    let project = use_radio_station::<ProjectState, ProjChan>();
    // Peeked at record time, never subscribed: the cap decides how much to keep, and a
    // settings change has no business re-rendering a results pane.
    let config = use_config_station();
    // A failed append is reported rather than only `tracing`d (P4-15): the row is on screen
    // either way, so a silent failure is the drawer disagreeing with the file until the next open.
    let report = use_report();
    let mut recorded = use_state(|| false);
    use_side_effect(move || {
        if *recorded.peek() {
            return;
        }
        // Pull the primitives out while the query borrow is held (both `Copy`), so the
        // borrow is released before `record_run` runs.
        let settled = match &*query.read().state() {
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Rows(rows)),
                ..
            } => Some((rows.output.elapsed_ms as u64, rows.output.total as u64)),
            // A statement that ran is a query the user may want back (ED-02) — a typed
            // `CREATE TABLE` no less than the `SELECT` it wraps. Its `count` is the rows it
            // moved; a statement that counts nothing records `0`, which is what the drawer's
            // "N rows" then honestly says about it.
            QueryStateData::Settled {
                res: Ok(QueryOutcome::Statement(report)),
                ..
            } => Some((report.elapsed_ms as u64, report.count.unwrap_or(0))),
            _ => None,
        };
        if let Some((elapsed_ms, rows)) = settled {
            recorded.set(true);
            let entry = HistoryEntry {
                sql: sql.clone(),
                ts_ms: now_ms(),
                elapsed_ms,
                rows,
            };
            record_run(
                history,
                project.peek().root.clone(),
                run,
                entry,
                history_cap(config),
                report,
            );
        }
    });
}

/// Record a completed successful run: prepend it to the in-memory satellite (deduped by
/// `run`, so a re-mount can't double-log; by SQL, so a re-run moves its entry rather than
/// stacking a second; trimmed to `cap`) and write it to `history.jsonl`.
///
/// **Which write depends on what the push did.** A query the log doesn't hold is one cheap
/// `O_APPEND` line, which is the shape `history.jsonl` was chosen for. A *re-run* moved an entry,
/// and an append can't move one — it would leave the superseded line in the file, so the whole
/// (already capped) list is rewritten instead. The file therefore never holds a duplicate, and
/// the rewrite costs nothing on the path that doesn't need it.
fn record_run(
    mut history: HistoryCtx,
    root: PathBuf,
    run: RunId,
    entry: HistoryEntry,
    cap: usize,
    report: ReportCtx,
) {
    // Peek first: an already-seen run must not take a write lock (which would wake the
    // history subscribers for nothing).
    if history.peek().seen.contains(&run) {
        return;
    }
    let replaced = history.write().push(run, entry.clone(), cap);
    // Snapshotted out here, after the write guard is dropped and before the task runs, so the
    // list that reaches disk is the one this run produced.
    let rewrite = replaced.then(|| history.peek().file_order());
    spawn(async move {
        persisted_history(&root, rewrite.as_deref(), &entry, report);
    });
}

/// The History drawer's **Clear**: empty the satellite and remove `history.jsonl`, so the
/// project reopens with no history rather than the rows coming straight back.
///
/// The disk half is spawned (like [`record_run`]'s append) and its failure is an **event**, not
/// just a `tracing` line: the list on screen is already empty, so a silent failure would be a
/// surface disagreeing with the file behind it until the next open — the same thing
/// [`persisted`](super::persist::persisted) records for every other `.strata` write.
///
/// It reports **directly** rather than through that funnel, and deliberately (P4-15 item 5): this
/// *removes* a file, so the funnel's "Could not write the …" would be less accurate rather than
/// more consistent, and there is nothing to gate — the satellite is emptied before the file is
/// touched.
pub fn clear_history(
    mut history: HistoryCtx,
    project: RadioStation<ProjectState, ProjChan>,
    log: LogCtx,
) {
    history.write().clear();
    let root = project.peek().root.clone();
    spawn(async move {
        if let Err(e) = project_io::clear_history(&root) {
            tracing::error!("clear history: {e}");
            log_event(
                log,
                LogLevel::Error,
                format!("Could not clear the query history: {e}"),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::process;

    use super::*;

    fn entry(sql: &str) -> HistoryEntry {
        HistoryEntry {
            sql: sql.into(),
            ts_ms: 0,
            elapsed_ms: 1,
            rows: 1,
        }
    }

    /// The same entry with distinguishable figures, for the tests that check *which* run of a
    /// repeated query survived.
    fn run_of(sql: &str, elapsed_ms: u64) -> HistoryEntry {
        HistoryEntry {
            sql: sql.into(),
            ts_ms: elapsed_ms,
            elapsed_ms,
            rows: 1,
        }
    }

    fn sqls(h: &History) -> Vec<&str> {
        h.entries.iter().map(|x| x.sql.as_str()).collect()
    }

    /// Dedup by run id: the same run recorded twice (a results-pane re-mount) lands once.
    #[test]
    fn push_is_deduped_by_run_id() {
        let mut h = History::load(Path::new("/nonexistent"), 100); // absent → empty
        let run = RunId::new();
        h.push(run, entry("SELECT 1"), 100);
        // `record_run` guards on `seen` before pushing; simulate its check.
        assert!(h.seen.contains(&run));
        assert_eq!(h.entries.len(), 1);
    }

    /// **Re-running a query moves its entry, it does not add one** — and the survivor is the
    /// newest run, with the newest figures.
    #[test]
    fn push_moves_a_repeated_query_instead_of_stacking_it() {
        let mut h = History::load(Path::new("/nonexistent"), 100);
        h.push(RunId::new(), run_of("SELECT 1", 10), 100);
        h.push(RunId::new(), run_of("SELECT 2", 20), 100);
        let replaced = h.push(RunId::new(), run_of("SELECT 1", 30), 100);

        assert!(
            replaced,
            "the caller has to know, so it can rewrite the log"
        );
        assert_eq!(
            sqls(&h),
            ["SELECT 1", "SELECT 2"],
            "the repeat moved to the top"
        );
        assert_eq!(
            h.entries[0].elapsed_ms, 30,
            "and carries the newest figures"
        );
    }

    /// The reason dedupe has to come **before** the cap: one query pressed over and over must
    /// not evict everything else. With a cap of 3, fifty runs of one query leave two free slots,
    /// not none.
    #[test]
    fn a_hammered_query_occupies_exactly_one_slot() {
        let mut h = History::load(Path::new("/nonexistent"), 3);
        h.push(RunId::new(), entry("SELECT a"), 3);
        h.push(RunId::new(), entry("SELECT b"), 3);
        for _ in 0..50 {
            h.push(RunId::new(), entry("SELECT * FROM events"), 3);
        }
        assert_eq!(sqls(&h), ["SELECT * FROM events", "SELECT b", "SELECT a"]);
    }

    /// Only whitespace is normalized away: re-indenting a query is the same query, changing its
    /// case is not (a quoted identifier is case-sensitive).
    #[test]
    fn dedupe_ignores_layout_but_not_case() {
        let mut h = History::load(Path::new("/nonexistent"), 100);
        h.push(RunId::new(), entry("SELECT a\n  FROM t"), 100);
        h.push(RunId::new(), entry("SELECT a FROM t"), 100);
        assert_eq!(h.entries.len(), 1, "same query, laid out differently");

        h.push(RunId::new(), entry("select a from t"), 100);
        assert_eq!(h.entries.len(), 2, "different case is a different query");
    }

    /// The in-memory window is trimmed to the cap the *caller* passed — the live
    /// `Settings::max_history`, so lowering it takes effect on the next run.
    #[test]
    fn push_trims_to_the_cap_it_is_given() {
        let mut h = History::load(Path::new("/nonexistent"), 2);
        for sql in ["a", "b", "c"] {
            h.push(RunId::new(), entry(sql), 2);
        }
        let sqls: Vec<&str> = h.entries.iter().map(|x| x.sql.as_str()).collect();
        assert_eq!(sqls, ["c", "b"], "newest kept, oldest dropped");
    }

    /// Clear empties the list but **keeps** the dedup guard: the pin holding a cleared run is
    /// still mounted, and forgetting it would let the run re-record itself on the next render.
    #[test]
    fn clear_empties_the_list_but_not_the_dedup_guard() {
        let mut h = History::load(Path::new("/nonexistent"), 100);
        let run = RunId::new();
        h.push(run, entry("SELECT 1"), 100);

        h.clear();
        assert!(h.entries.is_empty());
        assert!(
            h.seen.contains(&run),
            "a cleared run must not be re-recordable"
        );
    }

    /// Load flips file-order (oldest → newest) to newest-first for display.
    #[test]
    fn load_is_newest_first() {
        let root = env::temp_dir().join(format!("strata-history-test-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        for s in ["old", "mid", "new"] {
            project_io::append_history(&root, &entry(s)).unwrap();
        }
        let h = History::load(&root, 100);
        let sqls: Vec<&str> = h.entries.iter().map(|x| x.sql.as_str()).collect();
        assert_eq!(sqls, ["new", "mid", "old"]);
        let _ = fs::remove_dir_all(&root);
    }
}
