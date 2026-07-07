//! Per-window store of each tab's live query output (`WorkspaceRun`), keyed by
//! workspace id.
//!
//! Deliberately **separate from the durable session store** ([`crate::session`]):
//! a run is *heavy, ephemeral view-state* (a page of `QueryOutput` rows, running
//! flags, paging), whereas the session store is the *lightweight, cloned, persisted*
//! model. Keeping runs out means `session::snapshot()` / persistence stay cheap and
//! don't drag result pages around, and the persist effect (which watches `SESSION`)
//! isn't triggered by pure query activity. Keyed by the same `crate::session::
//! WorkspaceId` — one stable id addresses both a workspace and its run.
//!
//! A `dioxus-stores` `GlobalStore<HashMap<..>>`: `.get(id)` opens a scope that
//! tracks **just that key's** value, so the active tab's `Results` re-renders on its
//! own run, not on every tab's. Per-window (a `GlobalStore` is per-app). Never
//! persisted; entries are created on first write (`edit`), dropped when a tab closes
//! (`drop_ids`), and cleared on project open (`clear`) — a fresh project reassigns
//! ids, so a new tab could otherwise inherit a stale run.

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;
use dioxus_stores::*;

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

/// This window's per-tab runs, keyed by workspace id.
pub static RUNS: GlobalStore<HashMap<u64, WorkspaceRun>> = Global::new(|| HashMap::new());

/// Mutate tab `id`'s run, creating a default entry if absent. For the active tab
/// about to write (run start, paging, find, plan toggle).
pub fn edit(id: u64, f: impl FnOnce(&mut WorkspaceRun)) {
    let mut store = RUNS.resolve();
    if !store.contains_key(&id) {
        store.insert(id, WorkspaceRun::default());
    }
    if let Some(mut entry) = store.get(id) {
        let mut run = entry.write();
        f(&mut run);
    }
}

/// Mutate tab `id`'s run **only if it already exists** — for engine events, whose
/// tab may have been closed or superseded (no entry → the result is dropped).
pub fn edit_existing(id: u64, f: impl FnOnce(&mut WorkspaceRun)) {
    let store = RUNS.resolve();
    if let Some(mut entry) = store.get(id) {
        let mut run = entry.write();
        f(&mut run);
    }
}

/// Whether tab `id`'s run has request `req_id` in flight (engine-event routing).
pub fn is_pending(id: u64, req_id: u64) -> bool {
    RUNS.resolve()
        .get(id)
        .map(|e| e.peek().pending_req == Some(req_id))
        .unwrap_or(false)
}

/// Whether tab `id` currently has a query running (S14 — close confirms).
pub fn is_running(id: u64) -> bool {
    RUNS.resolve()
        .get(id)
        .map(|e| e.peek().running)
        .unwrap_or(false)
}

/// Drop the runs for closed tabs.
pub fn drop_ids(ids: &HashSet<u64>) {
    let mut store = RUNS.resolve();
    store.retain(|id, _| !ids.contains(id));
}

/// Clear every run (on project open — new tabs may reuse old ids).
pub fn clear() {
    let mut store = RUNS.resolve();
    store.clear();
}
