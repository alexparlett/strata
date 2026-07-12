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
use std::time::Instant;

use dioxus::prelude::*;
use dioxus_stores::*;

use crate::engine::QueryOutput;
use crate::plan::{PlanTab, QueryPlan};
use crate::query_error::QueryError;

/// Which result view is active for a tab — the grid (default) or the chart (R2).
/// Toggled from the results toolbar; kept per result-set (per tab).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ResultsView {
    #[default]
    Grid,
    Chart,
}

/// An Excel-style grid selection (Rz3), scoped to the current result **page**. Rows and
/// columns are sets (disjoint multi-select); a cell selection is a single rectangle from
/// the anchor to the focus.
#[derive(Clone, PartialEq)]
pub enum Selection {
    /// Rectangle from anchor `(ar,ac)` to focus `(fr,fc)` — page-local row + column index.
    Cell {
        ar: usize,
        ac: usize,
        fr: usize,
        fc: usize,
    },
    /// Whole rows, by page-local index.
    Rows(Vec<usize>),
    /// Whole columns, by column index.
    Cols(Vec<usize>),
}

impl Selection {
    /// The inclusive `(min_row, max_row, min_col, max_col)` of a `Cell` rectangle.
    pub fn cell_bounds(&self) -> Option<(usize, usize, usize, usize)> {
        match self {
            Selection::Cell { ar, ac, fr, fc } => Some((
                (*ar).min(*fr),
                (*ar).max(*fr),
                (*ac).min(*fc),
                (*ac).max(*fc),
            )),
            _ => None,
        }
    }
}

/// A column sort over the snapshot (Rz6): the sorted column index + direction. Nulls always
/// sort last. `None` on the run = unsorted; a header click cycles asc → desc → clear. Applied
/// at page-read time (`ORDER BY` over the snapshot) so the snapshot itself is never re-spooled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ColSort {
    pub col: usize,
    pub asc: bool,
}

/// One tab's live query output — never serialized. The results panel derives its
/// whole state (grid / plan / error / running / pager) from the active tab's run.
pub struct WorkspaceRun {
    pub result: Option<QueryOutput>,
    /// The current page as an Arrow `RecordBatch` — the type-aware source for Copy / Export
    /// (Rz4). Parallel to `result.rows`; kept *off* `QueryOutput` so the grid's per-render
    /// `result.clone()` never touches it (it's Arc-cheap to hold).
    pub page_batch: Option<datafusion::arrow::array::RecordBatch>,
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
    /// Whether the find popover is open for this result-set (U6). Ephemeral UI toggle,
    /// opened by the toolbar button or the `Find` command; closing clears `result_search`.
    pub find_open: bool,
    /// Grid vs chart for this result-set (the results toolbar toggle).
    pub view: ResultsView,
    /// Excel-style grid selection (Rz3), page-local; `None` = nothing selected. Cleared
    /// on page change / new result / clear.
    pub sel: Option<Selection>,
    /// Anchor row/column index for shift-click contiguous range selection (Excel-style).
    /// Set by a plain / ⌘ click on a row or column header; a shift-click fills from here to
    /// the clicked index. `None` = no anchor. Reset whenever `sel` is cleared.
    pub sel_anchor: Option<usize>,
    /// Per-column width overrides in px, keyed by column index (V20 resizable columns).
    /// Absent ⇒ the default column width. Session-scoped view state (never persisted);
    /// survives paging + sort (per result set), reset only on an explicit results clear.
    pub col_widths: HashMap<usize, f64>,
    /// Active column sort over the snapshot (Rz6). `None` = unsorted. Survives paging (each
    /// page fetch re-applies it); reset on a new query result.
    pub sort: Option<ColSort>,
    /// When the current result-set landed — drives the "⏱ snapshot Xm ago" chip.
    /// `None` until the tab has actually produced a result. Monotonic (`Instant`),
    /// never serialized (like the rest of the run).
    pub ran_at: Option<Instant>,
}

impl Default for WorkspaceRun {
    fn default() -> Self {
        Self {
            result: None,
            page_batch: None,
            query_error: None,
            plan: None,
            plan_tab: PlanTab::default(),
            plan_raw: false,
            running: false,
            pending_req: None,
            page: 1,
            page_size: 100,
            result_search: String::new(),
            find_open: false,
            view: ResultsView::Grid,
            sel: None,
            sel_anchor: None,
            col_widths: HashMap::new(),
            sort: None,
            ran_at: None,
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
