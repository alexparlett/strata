//! The reactive workspace model — the ground-up Dioxus-0.7 Store rewrite of the
//! editor/results/tabs (fixes A5/6/7).
//!
//! Terminology (unchanged): a **`Workspace`** is a tab's data (name + SQL +
//! origin); the `Tabs` strip and the `Workspace` *view* render it.
//!
//! **Single source of truth.** Every open workspace lives in the per-window
//! [`SESSION`] store, addressed by a stable [`WorkspaceId`]. `Session`/`Workspace`
//! derive `Store`, so the UI reads through *lenses* (`store.active()`,
//! `store.workspaces().iter()`, `ws.sql()`) — per-entry reactivity, and the
//! controlled `CodeEditor` binds straight to `ws.sql()`.
//!
//! **All mutations write through lenses, never a coarse root `.write()`.** Only a
//! lens write notifies the matching lens subscribers: a coarse `SESSION.write()`
//! would leave `.active()` / `.workspaces()` readers (the `Workbench`) stale — that
//! was the tab-switch bug. So `switch` is `store.active().set(id)`, structural
//! edits go through `store.workspaces()`, and per-field edits through the entry's
//! own lens (`ws.sql()`, `ws.name()`).
//!
//! **Durable vs runtime.** `Session` is pure serde (persisted in `session.json`);
//! a workspace's live query output is the runtime half and lives, keyed by the
//! same id, in [`crate::runs`] (never serialized).

use std::collections::HashSet;

use dioxus::prelude::*;
use dioxus_stores::*;
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
    /// Monotonic stamp of the last time this workspace was focused — drives the
    /// "show all tabs" most-recently-viewed cap. `0` = not yet focused.
    #[serde(default)]
    pub last_viewed: u64,
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
            last_viewed: 0,
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
    /// Monotonic focus clock — bumped on every activation; a workspace snapshots it
    /// into `last_viewed`, giving a most-recently-viewed order for the tab list.
    #[serde(default)]
    pub view_clock: u64,
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
    SESSION.resolve().active().cloned()
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

/// A clone of every workspace + active/next_id, in strip order (action-layer reads).
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

// --- durable mutations (lens writes) ---------------------------------------

/// Allocate the next workspace id (persisted counter), via the `next_id` lens.
fn alloc_id(store: Store<Session>) -> WorkspaceId {
    let id = store.next_id().cloned().max(1);
    store.next_id().set(id + 1);
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

/// Focus `id` and stamp it as the most-recently-viewed workspace (drives the
/// "show all tabs" MRU cap). Every activation path routes through here so the
/// clock stays authoritative.
fn set_active(store: Store<Session>, id: WorkspaceId) {
    let clock = store.view_clock().cloned() + 1;
    store.view_clock().set(clock);
    for w in store.workspaces().iter() {
        if w.id().cloned() == id {
            w.last_viewed().set(clock);
            break;
        }
    }
    store.active().set(id);
}

// Entry lookups are inlined (capture the sub-store into an inferred `Option`) so
// the un-nameable per-entry lens generic never has to be written in a signature.

/// Open `sql` in a workspace named `name`, bound to `origin`, and focus it. Reuses
/// an existing workspace of that name **only if it still holds exactly this SQL**
/// (unedited); an edited one is never clobbered — a fresh, uniquely-named workspace
/// is appended. Used by SELECT *, edit-view, and open-saved-query.
pub fn open_named(name: &str, sql: String, origin: Origin) {
    let store = SESSION.resolve();
    // Reuse an unedited same-named workspace.
    let mut reuse = None;
    for w in store.workspaces().iter() {
        if w.name().cloned() == name && w.sql().cloned() == sql {
            reuse = Some(w);
            break;
        }
    }
    if let Some(mut w) = reuse {
        let id = w.id().cloned();
        w.write().set_origin(origin);
        set_active(store, id);
        return;
    }
    let name = {
        let s = store.read();
        unique_name(&s, name)
    };
    let id = alloc_id(store);
    store
        .workspaces()
        .push(Workspace::new(id, name, sql, origin));
    set_active(store, id);
}

/// Append a **new** blank workspace (`query N`) and focus it (⌘T).
pub fn new_blank() -> WorkspaceId {
    let store = SESSION.resolve();
    let name = {
        let s = store.read();
        unique_name(&s, &format!("query {}", s.workspaces.len() + 1))
    };
    let id = alloc_id(store);
    store
        .workspaces()
        .push(Workspace::new(id, name, String::new(), Origin::Scratch));
    set_active(store, id);
    id
}

/// Append a workspace holding `sql` (uniquely named `query N`) and focus it.
pub fn open_new(sql: String) {
    let store = SESSION.resolve();
    let name = {
        let s = store.read();
        unique_name(&s, &format!("query {}", s.workspaces.len() + 1))
    };
    let id = alloc_id(store);
    store
        .workspaces()
        .push(Workspace::new(id, name, sql, Origin::Scratch));
    set_active(store, id);
}

/// Focus a workspace that already holds exactly `sql`, else append a new one
/// (idempotent — history double-clicks can't spawn duplicates).
pub fn open_or_focus_sql(sql: String) {
    let store = SESSION.resolve();
    let mut hit = None;
    for w in store.workspaces().iter() {
        if w.sql().cloned() == sql {
            hit = Some(w);
            break;
        }
    }
    if let Some(w) = hit {
        set_active(store, w.id().cloned());
    } else {
        open_new(sql);
    }
}

/// Re-open a previously closed workspace (⇧⌘T) with its saved name + SQL.
pub fn reopen(name: String, sql: String) {
    let store = SESSION.resolve();
    let id = alloc_id(store);
    store
        .workspaces()
        .push(Workspace::new(id, name, sql, Origin::Scratch));
    set_active(store, id);
}

/// Duplicate workspace `id`: clone its SQL into a new **scratch** workspace named
/// "<name> copy", inserted immediately to its right, and focus it. A no-op when
/// `id` isn't present. The copy is unbound (`Origin::Scratch`) — a working buffer,
/// not a second binding to the source's view / saved query.
pub fn duplicate(id: WorkspaceId) {
    let store = SESSION.resolve();
    let (src_sql, name, pos) = {
        let s = store.read();
        let Some(p) = s.workspaces.iter().position(|w| w.id == id) else {
            return;
        };
        let base = format!("{} copy", s.workspaces[p].name);
        (s.workspaces[p].sql.clone(), unique_name(&s, &base), p)
    };
    let new_id = alloc_id(store);
    store.workspaces().write().insert(
        pos + 1,
        Workspace::new(new_id, name, src_sql, Origin::Scratch),
    );
    set_active(store, new_id);
}

/// Replace workspace `id`'s SQL (Format / Clear / other programmatic edits). The
/// live `CodeEditor` writes its own `sql()` lens directly, not through here.
pub fn set_sql(id: WorkspaceId, sql: String) {
    let store = SESSION.resolve();
    let mut hit = None;
    for w in store.workspaces().iter() {
        if w.id().cloned() == id {
            hit = Some(w);
            break;
        }
    }
    if let Some(w) = hit {
        w.sql().set(sql);
    }
}

/// Rebind workspace `id` to `origin` (after ⌘S / save-as-view).
pub fn set_origin(id: WorkspaceId, origin: Origin) {
    let store = SESSION.resolve();
    let mut hit = None;
    for w in store.workspaces().iter() {
        if w.id().cloned() == id {
            hit = Some(w);
            break;
        }
    }
    if let Some(mut w) = hit {
        w.write().set_origin(origin);
    }
}

/// Focus workspace `id`.
pub fn switch(id: WorkspaceId) {
    set_active(SESSION.resolve(), id);
}

/// Rename workspace `id` (an empty name is ignored by the caller).
pub fn rename(id: WorkspaceId, name: String) {
    let store = SESSION.resolve();
    let mut hit = None;
    for w in store.workspaces().iter() {
        if w.id().cloned() == id {
            hit = Some(w);
            break;
        }
    }
    if let Some(w) = hit {
        w.name().set(name);
    }
}

/// Remove `ids`, moving focus to a sensible neighbour if the active one went (or
/// leaving the session empty). The caller records the closed workspaces + reaps
/// their runs / engine scopes.
pub fn remove_ids(ids: &HashSet<WorkspaceId>) {
    let store = SESSION.resolve();
    let (active, active_pos) = {
        let s = store.read();
        (s.active, s.workspaces.iter().position(|w| w.id == s.active))
    };
    store.workspaces().write().retain(|w| !ids.contains(&w.id));
    if ids.contains(&active) {
        let new_active = {
            let s = store.read();
            let n = s.workspaces.len();
            if n == 0 {
                0
            } else {
                s.workspaces[active_pos.unwrap_or(0).min(n - 1)].id
            }
        };
        set_active(store, new_active);
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
    s.workspaces.push(Workspace::new(
        1,
        "query 1".into(),
        String::new(),
        Origin::Scratch,
    ));
    s.active = 1;
    s.next_id = 2;
    *SESSION.resolve().write() = s;
}

/// Repair a loaded session: keep persisted ids, but fix legacy (`0`) / duplicate
/// ones, ensure ≥1 workspace, rebuild `next_id`, and guarantee a valid `active`.
fn normalize(s: &mut Session) {
    if s.workspaces.is_empty() {
        s.workspaces.push(Workspace::new(
            1,
            "query 1".into(),
            String::new(),
            Origin::Scratch,
        ));
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
    // Keep the focus clock above any persisted stamp so new activations stay monotonic.
    s.view_clock = s.view_clock.max(
        s.workspaces
            .iter()
            .map(|w| w.last_viewed)
            .max()
            .unwrap_or(0),
    );
    if !s.workspaces.iter().any(|w| w.id == s.active) {
        s.active = s.workspaces[0].id;
    }
}
