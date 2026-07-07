//! The reactive workspace model — the ground-up Dioxus-0.7 Store rewrite of the
//! editor/results/tabs (fixes A5/6/7).
//!
//! Terminology (unchanged): a **`Workspace`** is a tab's data (name + SQL +
//! origin); the `Tabs` strip and the `Workspace` *view* render it.
//!
//! **Single source of truth.** Every open workspace lives in the per-window
//! [`SESSION`] store, addressed by a stable [`WorkspaceId`] — no more `Vec`-index
//! vs id-`HashMap` mismatch, and no `active_ws` indirection. `Session` derives
//! `Store`, so the workbench iterates `SESSION.resolve().workspaces().iter()` and
//! hands each `Workspace` *view* a sub-store scoped to *its* workspace: per-entry
//! reactivity, and the controlled `CodeEditor` binds straight to `ws.sql()` — no
//! key/remount, no cross-tab write, which is what kills the editing bug.
//!
//! **Durable vs runtime.** `Session` is pure serde (persisted in `session.json`);
//! a workspace's live query output is the runtime half and lives, keyed by the
//! same id, in [`crate::runs`] (never serialized). The action layer mutates the
//! durable side through the free functions here; the live editor writes its own
//! workspace's `sql()` lens directly.

use std::collections::HashSet;

use dioxus::prelude::*;
use dioxus_stores::{GlobalStore, Store};
use serde::{Deserialize, Serialize};

use crate::project::Origin;
use crate::util::sql_hash;

/// Stable per-workspace identity — the key everything is addressed by.
pub type WorkspaceId = u64;

/// A query tab's durable data. Pure serde — persisted verbatim in `session.json`,
/// no skipped fields (the runtime output lives separately, in [`crate::runs`]).
#[derive(Store, Clone, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub sql: String,
    #[serde(default)]
    pub origin: Origin,
    #[serde(default)]
    pub origin_hash: u64,
}

impl Workspace {
    /// A workspace bound to `origin`, its dirty baseline snapshotted from `sql`.
    pub fn new(id: WorkspaceId, name: String, sql: String, origin: Origin) -> Self {
        let mut w = Workspace {
            id,
            name,
            sql,
            origin: Origin::Scratch,
            origin_hash: 0,
        };
        w.set_origin(origin);
        w
    }

    /// Backed-only dirtiness: a view / saved-query workspace that has diverged from
    /// its bound baseline. Scratch workspaces are Tier-2 session buffers → never dirty.
    pub fn is_dirty(&self) -> bool {
        match self.origin {
            Origin::Scratch => false,
            _ => sql_hash(&self.sql) != self.origin_hash,
        }
    }

    /// Bind to `origin`, snapshotting the current SQL as the in-sync baseline.
    /// Used on open-into-workspace and after ⌘S / save-as-view.
    pub fn set_origin(&mut self, origin: Origin) {
        self.origin_hash = sql_hash(&self.sql);
        self.origin = origin;
    }
}

/// The durable working session — the workspace portion of `session.json`. (Catalog
/// definitions stay in `project.json`; history + window geometry stay on
/// [`crate::project::Project`] and are merged into the file at save time.)
#[derive(Store, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Session {
    /// Workspaces in strip order.
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// The focused workspace's id (`0` = none).
    #[serde(default)]
    pub active: WorkspaceId,
    /// Monotonic id allocator, persisted so a reopened workspace keeps its identity.
    #[serde(default)]
    pub next_id: WorkspaceId,
}

/// This window's durable session. Per-window: a `GlobalStore` is per-app, and each
/// project window is its own Dioxus app (like [`crate::runs::RUNS`]).
pub static SESSION: GlobalStore<Session> = Global::new(|| Session::default());

// --- reads (action layer / status / persistence) --------------------------

/// This window's durable session store (for components that iterate it).
pub fn store() -> Store<Session> {
    SESSION.resolve()
}

/// The active workspace's id (`0` when there are none).
pub fn active_id() -> WorkspaceId {
    SESSION.resolve().read().active
}

/// A clone of the active workspace, if any.
pub fn active() -> Option<Workspace> {
    let s = SESSION.resolve();
    let s = s.read();
    s.workspaces.iter().find(|w| w.id == s.active).cloned()
}

/// The active workspace's SQL (empty when there are no workspaces).
pub fn active_sql() -> String {
    active().map(|w| w.sql).unwrap_or_default()
}

/// A clone of every workspace, in strip order (for action-layer reads).
pub fn snapshot() -> Session {
    SESSION.resolve().read().clone()
}

/// Whether workspace `id` is dirty (diverged from a backing view / saved query).
pub fn is_dirty(id: WorkspaceId) -> bool {
    SESSION
        .resolve()
        .read()
        .workspaces
        .iter()
        .find(|w| w.id == id)
        .map(|w| w.is_dirty())
        .unwrap_or(false)
}

// --- durable mutations -----------------------------------------------------
//
// Structural edits go through `.write()` on the whole `Session` (coarse, but
// correct — add/remove/reorder change the strip anyway). The *live editor* writes
// its own workspace's `sql()` sub-store lens instead, so a keystroke re-renders
// only that workspace.

/// Allocate the next workspace id (and bump the counter).
fn alloc_id(s: &mut Session) -> WorkspaceId {
    let id = s.next_id.max(1);
    s.next_id = id + 1;
    id
}

/// A name that doesn't collide with an existing workspace (`base`, then `base 2`, …).
fn unique_name(s: &Session, base: &str) -> String {
    if !s.workspaces.iter().any(|w| w.name == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|c| !s.workspaces.iter().any(|w| &w.name == c))
        .unwrap_or_else(|| base.to_string())
}

/// Open `sql` in a workspace named `name`, bound to `origin`, and focus it. Reuses
/// an existing workspace of that name **only if it still holds exactly this SQL**
/// (unedited), so repeated opens of an unchanged item don't pile up; an edited one
/// is never clobbered — a fresh, uniquely-named workspace is appended. Used by
/// SELECT *, edit-view, and open-saved-query.
pub fn open_named(name: &str, sql: String, origin: Origin) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    if let Some(w) = s.workspaces.iter_mut().find(|w| w.name == name && w.sql == sql) {
        w.set_origin(origin);
        s.active = { let id = w.id; id };
        return;
    }
    let name = unique_name(&s, name);
    let id = alloc_id(&mut s);
    s.workspaces.push(Workspace::new(id, name, sql, origin));
    s.active = id;
}

/// Append a **new** blank workspace (`query N`) and focus it (⌘T).
pub fn new_blank() -> WorkspaceId {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    let base = format!("query {}", s.workspaces.len() + 1);
    let name = unique_name(&s, &base);
    let id = alloc_id(&mut s);
    s.workspaces.push(Workspace::new(id, name, String::new(), Origin::Scratch));
    s.active = id;
    id
}

/// Append a workspace holding `sql` (uniquely named `query N`) and focus it.
pub fn open_new(sql: String) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    let base = format!("query {}", s.workspaces.len() + 1);
    let name = unique_name(&s, &base);
    let id = alloc_id(&mut s);
    s.workspaces.push(Workspace::new(id, name, sql, Origin::Scratch));
    s.active = id;
}

/// Focus a workspace that already holds exactly `sql`, else append a new one
/// (idempotent — history double-clicks can't spawn duplicates).
pub fn open_or_focus_sql(sql: String) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    if let Some(w) = s.workspaces.iter().find(|w| w.sql == sql) {
        s.active = w.id;
        return;
    }
    drop(s);
    open_new(sql);
}

/// Re-open a previously closed workspace (⇧⌘T) with its saved name + SQL.
pub fn reopen(name: String, sql: String) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    let id = alloc_id(&mut s);
    s.workspaces.push(Workspace::new(id, name, sql, Origin::Scratch));
    s.active = id;
}

/// Replace workspace `id`'s SQL (Format / Clear / other programmatic edits). The
/// live `CodeEditor` writes its own `sql()` lens directly, not through here.
pub fn set_sql(id: WorkspaceId, sql: String) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    if let Some(w) = s.workspaces.iter_mut().find(|w| w.id == id) {
        w.sql = sql;
    }
}

/// Rebind workspace `id` to `origin` (after ⌘S / save-as-view).
pub fn set_origin(id: WorkspaceId, origin: Origin) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    if let Some(w) = s.workspaces.iter_mut().find(|w| w.id == id) {
        w.set_origin(origin);
    }
}

/// Focus workspace `id`.
pub fn switch(id: WorkspaceId) {
    SESSION.resolve().write().active = id;
}

/// Rename workspace `id` (an empty name is ignored by the caller).
pub fn rename(id: WorkspaceId, name: String) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    if let Some(w) = s.workspaces.iter_mut().find(|w| w.id == id) {
        w.name = name;
    }
}

/// Remove `ids`, moving focus to a sensible neighbour if the active one went (or
/// leaving the session empty). The caller records the closed workspaces + reaps
/// their runs / engine scopes.
pub fn remove_ids(ids: &HashSet<WorkspaceId>) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    let active_gone = ids.contains(&s.active);
    // Index of the active workspace *before* removal, to pick a neighbour.
    let active_pos = s.workspaces.iter().position(|w| w.id == s.active);
    s.workspaces.retain(|w| !ids.contains(&w.id));
    if active_gone {
        let n = s.workspaces.len();
        s.active = if n == 0 {
            0
        } else {
            let i = active_pos.unwrap_or(0).min(n - 1);
            s.workspaces[i].id
        };
    }
}

// --- persistence bridge ----------------------------------------------------

/// Replace the session from a loaded [`Session`] (project open), repairing legacy /
/// duplicate ids and guaranteeing at least one workspace + a valid active id.
pub fn load(mut loaded: Session) {
    normalize(&mut loaded);
    *SESSION.resolve().write() = loaded;
}

/// Reset to a single blank workspace (a brand-new project).
pub fn reset_blank() {
    let mut s = Session::default();
    s.workspaces.push(Workspace::new(1, "query 1".into(), String::new(), Origin::Scratch));
    s.active = 1;
    s.next_id = 2;
    *SESSION.resolve().write() = s;
}

/// Repair a loaded session: keep persisted ids, but fix legacy (`0`) / duplicate
/// ones, ensure ≥1 workspace, rebuild `next_id`, and guarantee a valid `active`.
fn normalize(s: &mut Session) {
    if s.workspaces.is_empty() {
        s.workspaces
            .push(Workspace::new(1, "query 1".into(), String::new(), Origin::Scratch));
    }
    let ids_ok = {
        let mut seen = HashSet::new();
        s.workspaces.iter().all(|w| w.id != 0 && seen.insert(w.id))
    };
    if !ids_ok {
        for (i, w) in s.workspaces.iter_mut().enumerate() {
            w.id = i as u64 + 1;
        }
    }
    s.next_id = s.workspaces.iter().map(|w| w.id).max().unwrap_or(0) + 1;
    if !s.workspaces.iter().any(|w| w.id == s.active) {
        s.active = s.workspaces[0].id;
    }
}
