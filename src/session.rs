//! The reactive workspace model — the ground-up Dioxus-0.7 Store rewrite of the
//! editor/results/tabs (fixes A5/6/7).
//!
//! Terminology (unchanged): a **`Workspace`** is a tab's data (name + SQL +
//! origin); the `Tabs` strip and the `Workspace` *view* render it. Durable and
//! runtime state are **physically separated**, so persistence is a pure serde
//! type with no `serde(skip)`:
//!   - [`Session`] (durable): the ordered [`Workspace`]s + the active id, held in
//!     the per-window [`SESSION`] store. A `use_persist` effect (next stage) syncs
//!     it to `session.json`.
//!   - [`WorkspaceRun`] (runtime): one workspace's live query output, never
//!     serialized. Stored per-id in the runtime store (added with the results panel).
//!
//! Every workspace is addressed by a stable [`WorkspaceId`] — no more `Vec`-index
//! vs id-`HashMap` mismatch. `Session` derives `Store`, so the workbench iterates
//! `SESSION.resolve().workspaces().iter()` and hands each `Workspace` view a
//! sub-store scoped to *its* workspace: per-entry reactivity, and the controlled
//! `CodeEditor` binds straight to `ws.sql()` — no key/remount, no active-tab
//! indirection.

use dioxus::prelude::*;
use dioxus_stores::{GlobalStore, Store};
use serde::{Deserialize, Serialize};

use crate::engine::QueryOutput;
use crate::plan::{PlanTab, QueryPlan};
use crate::project::Origin;
use crate::query_error::QueryError;
use crate::util::sql_hash;

/// Stable per-workspace identity — the key everything is addressed by.
pub type WorkspaceId = u64;

/// A query tab's durable data. Pure serde — persisted verbatim in `session.json`,
/// no skipped fields (the runtime output lives separately, in [`WorkspaceRun`]).
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
/// definitions stay in `project.json`; see [`crate::project`].)
#[derive(Store, Clone, Serialize, Deserialize, Default)]
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

/// One workspace's live query output — the **runtime** half, never serialized.
/// Keyed by id in the runtime store (wired with the results panel). Replaces the
/// current `crate::runs::WorkspaceRun`.
#[allow(dead_code)]
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

// --- durable mutations, callable from the action layer --------------------
//
// Structural edits go through `.write()` on the whole `Session` (coarse, but
// correct — add/remove/reorder change the strip anyway). The *live editor* writes
// its own workspace's `sql()` sub-store lens instead, so a keystroke re-renders
// only that workspace.

/// This window's durable session store.
pub fn store() -> Store<Session> {
    SESSION.resolve()
}

/// The active workspace's id (`0` when there are none).
pub fn active_id() -> WorkspaceId {
    SESSION.resolve().read().active
}

/// Open a fresh workspace bound to `origin`, focus it, and return its id.
pub fn open(name: String, sql: String, origin: Origin) -> WorkspaceId {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    let id = s.next_id.max(1);
    s.next_id = id + 1;
    s.workspaces.push(Workspace::new(id, name, sql, origin));
    s.active = id;
    id
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

/// Focus workspace `id`.
pub fn switch(id: WorkspaceId) {
    SESSION.resolve().write().active = id;
}

/// Close workspace `id`, moving focus to a sensible neighbour if it was active.
pub fn close(id: WorkspaceId) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    if let Some(i) = s.workspaces.iter().position(|w| w.id == id) {
        s.workspaces.remove(i);
        if s.active == id {
            let n = s.workspaces.len();
            s.active = if n == 0 { 0 } else { s.workspaces[i.min(n - 1)].id };
        }
    }
}

/// Rename workspace `id`.
pub fn rename(id: WorkspaceId, name: String) {
    let mut store = SESSION.resolve();
    let mut s = store.write();
    if let Some(w) = s.workspaces.iter_mut().find(|w| w.id == id) {
        w.name = name;
    }
}
