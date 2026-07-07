//! Workspace-tab action handlers: new/close (+ variants)/reopen, inline rename,
//! and the tab context menu. Called from `action::dispatch`. Tabs are addressed
//! by their stable [`crate::session::WorkspaceId`]; the durable model lives in the
//! reactive [`crate::session`] store, so these are thin wrappers over its free
//! functions plus the runtime bookkeeping (closed-tab stack, runs, engine scopes).

use std::collections::HashSet;

use dioxus::prelude::*;

use crate::engine::Command;
use crate::session::WorkspaceId;
use crate::state::{AppState, ClosedTab};

/// Open a new blank query tab and focus it.
pub fn add(_state: Signal<AppState>) {
    crate::session::new_blank();
}

/// Duplicate workspace `id` into a new "<name> copy" tab to its right, and focus it.
pub fn duplicate(_state: Signal<AppState>, id: WorkspaceId) {
    crate::session::duplicate(id);
}

/// Focus workspace `id`.
pub fn switch(_state: Signal<AppState>, id: WorkspaceId) {
    crate::session::switch(id);
}

/// Close workspace `id`. Unsaved edits confirm first (A6, discard dialog); else a
/// tab with a query **in flight** confirms (S14 running dialog, no threshold — a
/// finished query has `running == false`, so quick queries never prompt); otherwise
/// it closes straight away.
pub fn close(state: Signal<AppState>, id: WorkspaceId) {
    if crate::session::is_dirty(id) {
        crate::overlays::open_close_confirm(id);
        return;
    }
    if crate::runs::is_running(id) && crate::settings::confirm_close_running() {
        crate::overlays::open_running_close(crate::overlays::RunningCloseTarget::Tab(id));
        return;
    }
    close_ids(state, &HashSet::from([id]));
}

/// Close workspace `id` unconditionally — from the discard-confirm dialog (A6).
pub fn close_force(state: Signal<AppState>, id: WorkspaceId) {
    close_ids(state, &HashSet::from([id]));
}

/// Close every tab except `id`.
pub fn close_others(state: Signal<AppState>, id: WorkspaceId) {
    let ids: HashSet<WorkspaceId> = crate::session::snapshot()
        .workspaces
        .iter()
        .map(|w| w.id)
        .filter(|&i| i != id)
        .collect();
    close_ids(state, &ids);
}

/// Close every tab to the right of `id` (in strip order).
pub fn close_right(state: Signal<AppState>, id: WorkspaceId) {
    let workspaces = crate::session::snapshot().workspaces;
    let pos = workspaces.iter().position(|w| w.id == id);
    let ids: HashSet<WorkspaceId> = match pos {
        Some(p) => workspaces.iter().skip(p + 1).map(|w| w.id).collect(),
        None => HashSet::new(),
    };
    close_ids(state, &ids);
}

/// Close every tab, leaving the workspace empty (center pane shows the empty state).
pub fn close_all(state: Signal<AppState>) {
    let ids: HashSet<WorkspaceId> = crate::session::snapshot()
        .workspaces
        .iter()
        .map(|w| w.id)
        .collect();
    close_ids(state, &ids);
}

/// Reopen the most recently closed tab (⇧⌘T).
pub fn reopen(mut state: Signal<AppState>) {
    let closed = state.write().closed_tabs.pop();
    let Some(c) = closed else {
        return;
    };
    crate::session::reopen(c.name, c.sql);
}

/// Core tab-removal: records the removed workspaces on the closed-tab stack
/// (capped 20), removes them from the session (which re-picks a neighbour focus),
/// reaps their runs, and tells the engine to drop their scopes. A no-op on an
/// empty id set.
fn close_ids(mut state: Signal<AppState>, ids: &HashSet<WorkspaceId>) {
    if ids.is_empty() {
        return;
    }
    // Collect the removed workspaces (in strip order) for the closed-tab stack.
    let removed: Vec<(WorkspaceId, String, String)> = crate::session::snapshot()
        .workspaces
        .iter()
        .filter(|w| ids.contains(&w.id))
        .map(|w| (w.id, w.name.clone(), w.sql.clone()))
        .collect();
    if removed.is_empty() {
        return;
    }
    let tx = {
        let mut s = state.write();
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
        s.cmd_tx.clone()
    };
    crate::session::remove_ids(ids);
    crate::runs::drop_ids(ids);
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
pub fn rename_tab(_state: Signal<AppState>, id: WorkspaceId, name: String) {
    let v = name.trim().to_string();
    if v.is_empty() {
        return;
    }
    crate::session::rename(id, v);
}
