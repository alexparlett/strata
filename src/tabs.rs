//! The reactive tab model — the ground-up Dioxus-0.7 Store rewrite of the
//! editor/results/tabs (fixes A5/6/7).
//!
//! Durable and runtime state are **physically separated**, so persistence is a
//! pure serde type with no `serde(skip)`:
//!   - [`Session`] (durable): the ordered [`TabDef`]s + the active id, held in the
//!     per-window [`SESSION`] store. A `use_persist` effect (next stage) syncs it
//!     to `session.json`.
//!   - [`TabRun`] (runtime): one tab's live query output, never serialized. Stored
//!     per-tab-id in the runtime store (added with the results panel).
//!
//! Every tab is addressed by a stable [`TabId`] — no more `Vec`-index vs
//! id-`HashMap` mismatch. `Session` derives `Store`, so the workbench iterates
//! `SESSION.resolve().tabs().iter()` and hands each `TabView` a sub-store scoped
//! to *its* tab: per-entry reactivity, and the controlled `CodeEditor` binds
//! straight to `tab.sql()` — no key/remount, no active-tab indirection.

use dioxus::prelude::*;
use dioxus_stores::{GlobalStore, Store};
use serde::{Deserialize, Serialize};

use crate::engine::QueryOutput;
use crate::plan::{PlanTab, QueryPlan};
use crate::project::Origin;
use crate::query_error::QueryError;
use crate::util::sql_hash;

/// Stable per-tab identity — the key everything is addressed by.
pub type TabId = u64;

/// Durable per-tab data. Pure serde — persisted verbatim in `session.json`, no
/// skipped fields (the runtime output lives separately, in [`TabRun`]).
#[derive(Store, Clone, Serialize, Deserialize, PartialEq)]
pub struct TabDef {
    pub id: TabId,
    pub name: String,
    pub sql: String,
    #[serde(default)]
    pub origin: Origin,
    #[serde(default)]
    pub origin_hash: u64,
}

impl TabDef {
    /// A tab bound to `origin`, its dirty baseline snapshotted from `sql`.
    pub fn new(id: TabId, name: String, sql: String, origin: Origin) -> Self {
        let mut t = TabDef {
            id,
            name,
            sql,
            origin: Origin::Scratch,
            origin_hash: 0,
        };
        t.set_origin(origin);
        t
    }

    /// Backed-only dirtiness: a view / saved-query tab that has diverged from its
    /// bound baseline. Scratch tabs are Tier-2 session buffers → never dirty.
    pub fn is_dirty(&self) -> bool {
        match self.origin {
            Origin::Scratch => false,
            _ => sql_hash(&self.sql) != self.origin_hash,
        }
    }

    /// Bind to `origin`, snapshotting the current SQL as the in-sync baseline.
    /// Used on open-into-tab and after ⌘S / save-as-view.
    pub fn set_origin(&mut self, origin: Origin) {
        self.origin_hash = sql_hash(&self.sql);
        self.origin = origin;
    }
}

/// The durable working session — the tab portion of `session.json`. (Catalog
/// definitions stay in `project.json`; see [`crate::project`].)
#[derive(Store, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    /// Tabs in strip order.
    #[serde(default)]
    pub tabs: Vec<TabDef>,
    /// The focused tab's id (`0` = none).
    #[serde(default)]
    pub active: TabId,
    /// Monotonic id allocator, persisted so a reopened tab keeps its identity.
    #[serde(default)]
    pub next_id: TabId,
}

/// This window's durable tab session. Per-window: a `GlobalStore` is per-app, and
/// each project window is its own Dioxus app (like [`crate::runs::RUNS`]).
pub static SESSION: GlobalStore<Session> = Global::new(|| Session::default());

/// One tab's live query output — the **runtime** half, never serialized. Keyed by
/// tab id in the runtime store (wired with the results panel). Mirrors the current
/// `crate::runs::WorkspaceRun`, which this replaces.
#[allow(dead_code)]
pub struct TabRun {
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

impl Default for TabRun {
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
// its own tab's `sql()` sub-store lens instead, so a keystroke re-renders only
// that tab.

/// This window's durable tab store.
pub fn session() -> Store<Session> {
    SESSION.resolve()
}

/// The active tab's id (`0` when there are no tabs).
pub fn active_id() -> TabId {
    SESSION.resolve().read().active
}

/// Open a fresh tab bound to `origin`, focus it, and return its id.
pub fn open_tab(name: String, sql: String, origin: Origin) -> TabId {
    let store = SESSION.resolve();
    let mut s = store.write();
    let id = s.next_id.max(1);
    s.next_id = id + 1;
    s.tabs.push(TabDef::new(id, name, sql, origin));
    s.active = id;
    id
}

/// Replace tab `id`'s SQL (Format / Clear / other programmatic edits). The live
/// `CodeEditor` writes its own `sql()` lens directly and does *not* go through here.
pub fn set_sql(id: TabId, sql: String) {
    let store = SESSION.resolve();
    let mut s = store.write();
    if let Some(t) = s.tabs.iter_mut().find(|t| t.id == id) {
        t.sql = sql;
    }
}

/// Focus tab `id`.
pub fn switch(id: TabId) {
    SESSION.resolve().write().active = id;
}

/// Close tab `id`, moving focus to a sensible neighbour if it was active.
pub fn close(id: TabId) {
    let store = SESSION.resolve();
    let mut s = store.write();
    if let Some(i) = s.tabs.iter().position(|t| t.id == id) {
        s.tabs.remove(i);
        if s.active == id {
            let n = s.tabs.len();
            s.active = if n == 0 { 0 } else { s.tabs[i.min(n - 1)].id };
        }
    }
}

/// Rename tab `id`.
pub fn rename(id: TabId, name: String) {
    let store = SESSION.resolve();
    let mut s = store.write();
    if let Some(t) = s.tabs.iter_mut().find(|t| t.id == id) {
        t.name = name;
    }
}
