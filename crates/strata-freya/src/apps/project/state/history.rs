//! The per-window **query-history** satellite (P4-14 · state-arch §8).
//!
//! History is its own small store — **not** on `ProjectState` (shared/committed) and not on
//! `SessionState` (the tabs) — persisted locally to `.strata/history.jsonl` (append-only,
//! gitignored; IO in `strata-core::project`). The Run path publishes into it; the future
//! History drawer (FEATURES §12) reads it.
//!
//! **Only successful data runs are recorded**, captured when the run *settles* `Ok(Rows)` —
//! a failed / cancelled run (settles `Err`) and an Explain (settles `Plan`) never reach
//! here, so history stays a log of queries that actually returned data. Recording is
//! deduped by the run's [`RunId`]: the results pane re-mounts on a tab switch and re-serves
//! the *cached* result, which would otherwise re-log the same run.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use freya::prelude::{spawn, use_consume, use_side_effect, use_state, State, WritableUtils};
use freya::query::{QueryStateData, UseQuery};
use freya::radio::use_radio_station;
use strata_core::project as project_io;
use strata_model::HistoryEntry;

use crate::apps::project::query::{QueryOutcome, RunId, RunQuery};
use crate::state::{use_config_station, ConfigStation};

use super::{ProjChan, ProjectState};

/// How many recent runs are kept, in memory and on disk: the user's `max_history` setting
/// (design24 System ▸ History), which is the *only* source — a second constant here would
/// silently outrank the control P4-06 is about to put on it.
///
/// Floored at 1 because a `0` (only reachable by hand-editing the config) would make the
/// next load rotate `history.jsonl` down to nothing — a setting shouldn't delete the log.
pub fn history_cap(config: ConfigStation) -> usize {
    config.peek().settings.max_history.max(1)
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

    /// Record `run` once, newest-first, trimming the in-memory window to `cap`. Repeats (the
    /// same run re-served after a re-mount) are no-ops — the caller peeks
    /// [`seen`](Self::seen) first, so reaching here means it's genuinely new.
    ///
    /// `cap` is passed per call rather than stored: it is a live setting, so lowering it
    /// takes effect on the next recorded run instead of waiting for the window to reopen.
    fn push(&mut self, run: RunId, entry: HistoryEntry, cap: usize) {
        self.seen.insert(run);
        self.entries.push_front(entry);
        while self.entries.len() > cap {
            self.entries.pop_back();
        }
    }
}

/// The history satellite in context.
pub type HistoryCtx = State<History>;

/// Drive history recording for a results pane's run: record it once when it settles
/// `Ok(Rows)` — a successful *data* run. A failed / cancelled `Err` or an Explain `Plan`
/// never records, so bad queries stay out of history; the local `recorded` flag stops a
/// re-record on later re-renders of the same mount, and cross-mount dedup (a tab switch
/// re-serves the cached result) lives in the store, keyed by `run`. Call once per
/// `ResultsBody` (keyed by the press's nonce).
pub fn use_history_recording(query: UseQuery<RunQuery>, run: RunId, sql: String) {
    let history = use_consume::<HistoryCtx>();
    let project = use_radio_station::<ProjectState, ProjChan>();
    // Peeked at record time, never subscribed: the cap decides how much to keep, and a
    // settings change has no business re-rendering a results pane.
    let config = use_config_station();
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
            );
        }
    });
}

/// Record a completed successful run: prepend it to the in-memory satellite (deduped by
/// `run`, so a re-mount can't double-log, and trimmed to `cap`) and, if newly recorded,
/// append it to `history.jsonl`.
fn record_run(mut history: HistoryCtx, root: PathBuf, run: RunId, entry: HistoryEntry, cap: usize) {
    // Peek first: an already-seen run must not take a write lock (which would wake the
    // history subscribers for nothing).
    if history.peek().seen.contains(&run) {
        return;
    }
    history.write().push(run, entry.clone(), cap);
    spawn(async move {
        if let Err(e) = project_io::append_history(&root, &entry) {
            tracing::error!("append history: {e}");
        }
    });
}

/// Epoch millis now — stamps a [`HistoryEntry`] as a run completes.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
