//! Workspace-tab action handlers: new/close (+ variants)/reopen, inline rename,
//! and the tab context menu. Called from `action::dispatch`.

use dioxus::prelude::*;

use crate::engine::Command;
use crate::state::{AppState, ClosedTab, Origin, Workspace};

/// Open a new blank query tab and focus it.
pub fn add(mut state: Signal<AppState>) {
    let mut s = state.write();
    let id = s.project.next_ws_id;
    s.project.next_ws_id += 1;
    let n = s.project.workspaces.len() + 1;
    s.project
        .workspaces
        .push(Workspace::new(id, format!("query {n}"), String::new(), Origin::Scratch));
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
    s.project
        .workspaces
        .push(Workspace::new(id, c.name, c.sql, Origin::Scratch));
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
        crate::runs::drop_ids(&remove_ids);
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

/// Commit an inline tab rename: set the tab's name (an empty draft is a no-op).
/// Start / draft / cancel are transient UI owned by the `Tabs` component; only
/// this durable commit is an action, so it autosaves via `dispatch`.
pub fn rename_tab(mut state: Signal<AppState>, idx: usize, name: String) {
    let v = name.trim().to_string();
    if v.is_empty() {
        return;
    }
    if let Some(w) = state.write().project.workspaces.get_mut(idx) {
        w.name = v;
    }
}
