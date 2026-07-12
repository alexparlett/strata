//! Paging + column sort + page-size (the snapshot LIMIT/OFFSET + ORDER BY controls). Split
//! out of the action::query module.

use dioxus::prelude::*;

use crate::engine::Command;
use crate::state::AppState;

/// Fetch a specific page from the active workspace's snapshot (bounded LIMIT/OFFSET).
pub fn fetch_page(state: Signal<AppState>, page: usize) {
    let ws_id = crate::session::active_id();
    // Resolve the active sort (if any) to `(column name, ascending)` so the engine can
    // ORDER BY it over the snapshot; the name comes from the result schema at that index.
    let (page_size, has_result, sort) = crate::runs::RUNS
        .resolve()
        .get(ws_id)
        .map(|e| {
            let run = e.peek();
            let sort = run.sort.and_then(|s| {
                run.result
                   .as_ref()
                   .and_then(|r| r.columns.get(s.col))
                   .map(|c| (c.name.clone(), s.asc))
            });
            (run.page_size, run.result.is_some(), sort)
        })
        .unwrap_or((100, false, None));
    if !has_result {
        return;
    }
    // Selection is page-local — dropping it avoids stale indices highlighting the new page.
    crate::runs::edit(ws_id, |run| {
        run.page = page;
        run.sel = None;
        run.sel_anchor = None;
    });
    let tx = state.read().cmd_tx.clone();
    if let Some(tx) = tx {
        let _ = tx.send(Command::FetchPage {
            ws_id,
            page,
            page_size,
            sort,
        });
    }
}
/// Cycle the active tab's column sort (Rz6): unsorted → `ci` asc → `ci` desc → clear, then
/// re-fetch page 1. Sort is applied over the whole snapshot at page-read time.
pub fn sort_column(state: Signal<AppState>, ci: usize) {
    let ws_id = crate::session::active_id();
    if ws_id == 0 {
        return;
    }
    crate::runs::edit(ws_id, |run| {
        run.sort = match run.sort {
            Some(crate::runs::ColSort { col, asc: true }) if col == ci => {
                Some(crate::runs::ColSort { col: ci, asc: false })
            }
            Some(crate::runs::ColSort { col, asc: false }) if col == ci => None,
            _ => Some(crate::runs::ColSort { col: ci, asc: true }),
        };
    });
    fetch_page(state, 1);
}
/// Toggle the page-size dropdown.
pub fn toggle_page_size_menu(mut state: Signal<AppState>) {
    let mut s = state.write();
    s.page_size_open = !s.page_size_open;
}
/// Set the page size and reload the first page.
pub fn set_page_size(mut state: Signal<AppState>, size: usize) {
    state.write().page_size_open = false;
    let id = crate::session::active_id();
    if id != 0 {
        crate::runs::edit(id, |run| run.page_size = size);
    }
    fetch_page(state, 1);
}
