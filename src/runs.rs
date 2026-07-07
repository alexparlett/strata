//! Per-window store of each tab's live query output (`WorkspaceRun`), keyed by tab id.
//!
//! Deliberately **off `AppState`**: `AppState` is one coarse `Signal`, so any
//! write re-renders every reader. Holding the runs in their own `GlobalSignal`
//! (like [`crate::settings`] / [`crate::overlays`]) means the frequent small
//! mutations — find-in-results, paging, the plan-tab toggle — re-render only the
//! components that read `RUNS` (the results panel), not the editor / tabs /
//! sidebar. Never persisted; results are rebuilt as queries run.
//!
//! Keyed by the tab's `id`; the *active* tab's id comes from the session store
//! ([`crate::session::active_id`]). Entries are created on first write
//! (`edit`), dropped when a tab closes (`drop_ids`), and cleared on project open
//! (`clear`) — a fresh project reassigns ids, so a new tab could otherwise inherit
//! a stale run.

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;

use crate::engine::QueryOutput;
use crate::plan::{PlanTab, QueryPlan};
use crate::query_error::QueryError;

/// One tab's live query output — never serialized. The results panel derives its
/// whole state (grid / plan / error / running / pager) from the active tab's run.
pub struct WorkspaceRun {
    pub result: Option<QueryOutput>,
    pub query_error: Option<QueryError>,
    pub plan: Option<QueryPlan>,
    pub plan_tab: PlanTab,
    pub plan_raw: bool,
    pub running: bool,
    pub pending_req: Option<u64>,
    /// 1-based page into the snapshot.
    pub page: usize,
    pub page_size: usize,
    pub result_search: String,
}

impl Default for WorkspaceRun {
    fn default() -> Self {
        Self {
            result: None,
            query_error: None,
            plan: None,
            plan_tab: PlanTab::default(),
            plan_raw: false,
            running: false,
            pending_req: None,
            page: 1,
            page_size: 100,
            result_search: String::new(),
        }
    }
}

/// This window's per-tab runs, keyed by tab id.
pub static RUNS: GlobalSignal<HashMap<u64, WorkspaceRun>> = Signal::global(HashMap::new);

/// Mutate tab `id`'s run, creating a default entry if absent. For the active tab
/// about to write (run start, paging, find, plan toggle).
pub fn edit(id: u64, f: impl FnOnce(&mut WorkspaceRun)) {
    f(RUNS.write().entry(id).or_default());
}

/// Mutate tab `id`'s run **only if it already exists** — for engine events, whose
/// tab may have been closed or superseded (no entry → the result is dropped).
pub fn edit_existing(id: u64, f: impl FnOnce(&mut WorkspaceRun)) {
    if let Some(run) = RUNS.write().get_mut(&id) {
        f(run);
    }
}

/// Whether tab `id`'s run has request `req_id` in flight (engine-event routing).
pub fn is_pending(id: u64, req_id: u64) -> bool {
    RUNS.peek()
        .get(&id)
        .map(|r| r.pending_req == Some(req_id))
        .unwrap_or(false)
}

/// Drop the runs for closed tabs.
pub fn drop_ids(ids: &HashSet<u64>) {
    RUNS.write().retain(|id, _| !ids.contains(id));
}

/// Clear every run (on project open — new tabs may reuse old ids).
pub fn clear() {
    RUNS.write().clear();
}
