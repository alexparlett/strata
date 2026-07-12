//! Results / plan view state — clear, find popover, error dismiss, plan-tab/raw toggles,
//! result-search text, grid↔chart view. Split out of the action::query module.

use dioxus::prelude::*;

use crate::state::{AppState, LogKind};

/// Clear the active tab's results back to the empty state (Rz8).
pub fn clear_results(mut state: Signal<AppState>) {
    let id = crate::session::active_id();
    if id == 0 {
        return;
    }
    let mut cleared = false;
    crate::runs::edit_existing(id, |run| {
        if run.running || (run.result.is_none() && run.query_error.is_none() && run.plan.is_none())
        {
            return;
        }
        run.result = None;
        run.page_batch = None;
        run.query_error = None;
        run.plan = None;
        run.result_search = String::new();
        run.sel = None;
        run.sel_anchor = None;
        run.sort = None;
        run.col_widths.clear();
        cleared = true;
    });
    if cleared {
        state.write().set_status(LogKind::Info, "Cleared results");
    }
}
/// Open/close a tab's results find popover (U6). Closing clears its find query so a
/// stale filter never lingers. No-op if the tab has no run yet.
pub fn set_results_find(ws: crate::session::WorkspaceId, open: bool) {
    crate::runs::edit_existing(ws, |r| {
        r.find_open = open;
        if !open {
            r.result_search.clear();
        }
    });
}
/// Dismiss the results-pane error view (falls back to the grid if a prior
/// result is still loaded, otherwise the "no results yet" empty state).
pub fn dismiss_error(_state: Signal<AppState>) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit_existing(id, |run| run.query_error = None);
    }
}

/// Switch the EXPLAIN plan view between the physical and logical trees.
pub fn set_plan_tab(_state: Signal<AppState>, tab: crate::plan::PlanTab) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit_existing(id, |run| run.plan_tab = tab);
    }
}

/// Toggle the EXPLAIN plan view between the operator-card tree and raw text.
pub fn toggle_plan_raw(_state: Signal<AppState>) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit_existing(id, |run| run.plan_raw = !run.plan_raw);
    }
}
/// Update the find-in-results query.
pub fn set_result_search(_state: Signal<AppState>, q: String) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit(id, |run| run.result_search = q);
    }
}

/// Switch the active tab's result view (grid ↔ chart). Per result-set.
pub fn set_results_view(v: crate::runs::ResultsView) {
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit(id, |run| run.view = v);
    }
}
