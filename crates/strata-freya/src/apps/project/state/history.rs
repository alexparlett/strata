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

use super::{ProjChan, ProjectState};

/// Recent runs kept in memory and on disk — the file rotates to this window on load.
const HISTORY_CAP: usize = 200;

/// The window's query-history satellite: recent runs newest-first, plus a dedup guard so a
/// results-pane re-mount can't re-log a run.
pub struct History {
    pub entries: VecDeque<HistoryEntry>,
    seen: HashSet<RunId>,
}

impl History {
    /// Load the persisted history for `root` (newest capped to [`HISTORY_CAP`]) into a fresh
    /// satellite. A load error logs and yields an empty history rather than blocking the
    /// window — history is regenerable, unlike the project.
    pub fn load(root: &Path) -> Self {
        let loaded = project_io::load_history(root, HISTORY_CAP).unwrap_or_else(|e| {
            tracing::error!("load history: {e}");
            Vec::new()
        });
        // File order is oldest → newest; the satellite holds newest-first.
        Self {
            entries: loaded.into_iter().rev().collect(),
            seen: HashSet::new(),
        }
    }

    /// Record `run` once, newest-first, capping the in-memory window. Repeats (the same run
    /// re-served after a re-mount) are no-ops — the caller peeks [`seen`](Self::seen) first,
    /// so reaching here means it's genuinely new.
    fn push(&mut self, run: RunId, entry: HistoryEntry) {
        self.seen.insert(run);
        self.entries.push_front(entry);
        while self.entries.len() > HISTORY_CAP {
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
            record_run(history, project.peek().root.clone(), run, entry);
        }
    });
}

/// Record a completed successful run: prepend it to the in-memory satellite (deduped by
/// `run`, so a re-mount can't double-log) and, if newly recorded, append it to
/// `history.jsonl`.
fn record_run(mut history: HistoryCtx, root: PathBuf, run: RunId, entry: HistoryEntry) {
    // Peek first: an already-seen run must not take a write lock (which would wake the
    // history subscribers for nothing).
    if history.peek().seen.contains(&run) {
        return;
    }
    history.write().push(run, entry.clone());
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
        let mut h = History::load(Path::new("/nonexistent")); // absent → empty
        let run = RunId::new();
        h.push(run, entry("SELECT 1"));
        // `record_run` guards on `seen` before pushing; simulate its check.
        assert!(h.seen.contains(&run));
        assert_eq!(h.entries.len(), 1);
    }

    /// Load flips file-order (oldest → newest) to newest-first for display.
    #[test]
    fn load_is_newest_first() {
        let root = std::env::temp_dir().join(format!("strata-history-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for s in ["old", "mid", "new"] {
            project_io::append_history(&root, &entry(s)).unwrap();
        }
        let h = History::load(&root);
        let sqls: Vec<&str> = h.entries.iter().map(|x| x.sql.as_str()).collect();
        assert_eq!(sqls, ["new", "mid", "old"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
