//! Workspace-tab action handlers: new/close (+ variants)/reopen, inline rename,
//! and the tab context menu. Called from `action::dispatch`.

use dioxus::prelude::*;

use crate::engine::Command;
use crate::state::{AppState, ClosedTab, TabRun, Workspace};

/// Open a new blank query tab and focus it.
pub fn add(mut state: Signal<AppState>) {
    let mut s = state.write();
    let id = s.project.next_ws_id;
    s.project.next_ws_id += 1;
    let n = s.project.workspaces.len() + 1;
    s.project.workspaces.push(Workspace {
        id,
        name: format!("query {n}"),
        sql: String::new(),
        run: TabRun::default(),
    });
    s.project.active_ws = s.project.workspaces.len() - 1;
}

/// Focus tab `idx`.
pub fn switch(mut state: Signal<AppState>, idx: usize) {
    state.write().project.active_ws = idx;
}

pub fn close(state: Signal<AppState>, idx: usize) {
    close_where(state, |i| i == idx);
}

/// Close every tab except `idx`.
pub fn close_others(state: Signal<AppState>, idx: usize) {
    close_where(state, move |i| i != idx);
}

/// Close every tab to the right of `idx`.
pub fn close_right(state: Signal<AppState>, idx: usize) {
    close_where(state, move |i| i > idx);
}

/// Close every tab, leaving the workspace empty (center pane shows the empty state).
pub fn close_all(state: Signal<AppState>) {
    close_where(state, |_| true);
}

/// Reopen the most recently closed tab (⇧⌘T).
pub fn reopen(mut state: Signal<AppState>) {
    let closed = state.write().closed_tabs.pop();
    let Some(c) = closed else {
        return;
    };
    let mut s = state.write();
    let id = s.project.next_ws_id;
    s.project.next_ws_id += 1;
    s.project.workspaces.push(Workspace {
        id,
        name: c.name,
        sql: c.sql,
        run: TabRun::default(),
    });
    s.project.active_ws = s.project.workspaces.len() - 1;
}

/// Core tab-removal: drops every tab whose index satisfies `remove`, records
/// them on the closed-tab stack (capped 20), reaps their snapshots, and keeps
/// the previously-active tab selected (or leaves the workspace empty).
fn close_where(mut state: Signal<AppState>, remove: impl Fn(usize) -> bool) {
    let (removed, active_id, tx) = {
        let s = state.read();
        let removed: Vec<(u64, String, String)> = s
            .project
            .workspaces
            .iter()
            .enumerate()
            .filter(|(i, _)| remove(*i))
            .map(|(_, w)| (w.id, w.name.clone(), w.sql.clone()))
            .collect();
        let active_id = s.project.workspaces.get(s.project.active_ws).map(|w| w.id);
        (removed, active_id, s.cmd_tx.clone())
    };
    if removed.is_empty() {
        return;
    }
    let remove_ids: std::collections::HashSet<u64> = removed.iter().map(|(id, ..)| *id).collect();
    {
        let mut s = state.write();
        s.project.workspaces.retain(|w| !remove_ids.contains(&w.id));
        for (_, name, sql) in &removed {
            s.closed_tabs.push(ClosedTab {
                name: name.clone(),
                sql: sql.clone(),
            });
        }
        let overflow = s.closed_tabs.len().saturating_sub(20);
        if overflow > 0 {
            s.closed_tabs.drain(0..overflow);
        }
        if s.project.workspaces.is_empty() {
            // No tabs left → the center pane renders the empty state.
            s.project.active_ws = 0;
        } else {
            let keep = active_id
                .filter(|id| !remove_ids.contains(id))
                .and_then(|id| s.project.workspaces.iter().position(|w| w.id == id))
                .unwrap_or(0);
            s.project.active_ws = keep.min(s.project.workspaces.len() - 1);
        }
    }
    if let Some(tx) = tx {
        for (id, ..) in &removed {
            let _ = tx.send(Command::CleanupWorkspace { ws_id: *id });
        }
    }
}

// ---- inline rename ----

pub fn start_rename(mut state: Signal<AppState>, idx: usize) {
    let name = state
        .read()
        .project
        .workspaces
        .get(idx)
        .map(|w| w.name.clone())
        .unwrap_or_default();
    let mut s = state.write();
    s.renaming_ws = Some(idx);
    s.rename_val = name;
}

pub fn rename_input(mut state: Signal<AppState>, val: String) {
    state.write().rename_val = val;
}

pub fn commit_rename(mut state: Signal<AppState>) {
    let mut s = state.write();
    if let Some(idx) = s.renaming_ws {
        let v = s.rename_val.trim().to_string();
        if !v.is_empty() {
            if let Some(w) = s.project.workspaces.get_mut(idx) {
                w.name = v;
            }
        }
    }
    s.renaming_ws = None;
}

pub fn cancel_rename(mut state: Signal<AppState>) {
    state.write().renaming_ws = None;
}
