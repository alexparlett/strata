//! The per-window **Session**: the open query tabs and their arrangement.
//!
//! One window == one [`SessionState`] (a Radio store, provided in the window root). Each tab is
//! a stateful [`QueryTab`] that **owns its editor buffer** ([`CodeEditorData`]) — the Valin
//! pattern: the buffer lives in the store, keyed by [`TabId`], and the editor slices a
//! `Writable` into it. Dirty is the editor's own `is_edited()`; closing/reopening **moves** the
//! whole tab (no snapshot). Persistence (a serde snapshot) is a later slice.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use strata_code_editor::prelude::{CodeEditorData, EditorLanguage, Rope};
use uuid::Uuid;

/// Stable per-tab identity — real identity, so no allocator and no duplicate-id repair.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TabId(pub Uuid);

impl TabId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// A reference to a saved project artifact (a View or SavedQuery), by key. `String` for now —
/// promoted to a real key type when the Project store lands.
pub type ArtifactKey = String;

/// What a tab is bound to — its **save target** only. Dirty comes from the editor, not this.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Origin {
    Scratch,
    View(ArtifactKey),
    SavedQuery(ArtifactKey),
}

/// One query tab. Owns its editing buffer exactly like Valin's `EditorTab`.
pub struct QueryTab {
    pub id: TabId,
    pub name: String,
    pub editor: CodeEditorData,
    pub origin: Origin,
}

/// The SQL grammar (derekstride/tree-sitter-sql via `tree-sitter-sequel`) + its highlights query,
/// handed to each tab's editor for syntax highlighting.
fn sql_language() -> EditorLanguage {
    EditorLanguage::new(
        tree_sitter_sequel::LANGUAGE,
        tree_sitter_sequel::HIGHLIGHTS_QUERY,
    )
}

impl QueryTab {
    /// A tab holding `sql`, bound to `origin`. The editor is marked saved at its opening text,
    /// so a freshly-opened bound tab reads as *not* dirty until edited.
    pub fn new(name: String, sql: String, origin: Origin) -> Self {
        let mut editor = CodeEditorData::new(Rope::from_str(&sql), Some(sql_language()));
        // Populate the line metrics so the editor renders its content immediately (parse builds
        // the line blocks; measure sizes them). `mark_as_saved` then snapshots the opening text
        // as the dirty baseline so a freshly-opened tab isn't "edited".
        editor.parse();
        editor.measure(12.0, "Jetbrains Mono");
        editor.mark_as_saved();
        Self {
            id: TabId::new(),
            name,
            editor,
            origin,
        }
    }

    /// A blank scratch buffer.
    pub fn scratch(name: String) -> Self {
        Self::new(name, String::new(), Origin::Scratch)
    }

    /// The current editor text.
    pub fn text(&self) -> String {
        self.editor.rope.to_string()
    }

    /// Backed-only dirtiness: a bound tab whose editor has diverged from its saved baseline.
    /// Scratch tabs are working buffers → never dirty.
    pub fn is_dirty(&self) -> bool {
        !matches!(self.origin, Origin::Scratch) && self.editor.is_edited()
    }
}

/// Cap on the reopen stack; parking more drops the oldest (freeing its buffer).
const CLOSED_CAP: usize = 20;

/// The window's open tabs + arrangement. Holds live [`QueryTab`]s (not serde — persistence goes
/// through a snapshot, a later slice). Provided as a Radio store in the window root.
#[derive(Default)]
pub struct SessionState {
    pub tabs: HashMap<TabId, QueryTab>,
    pub order: Vec<TabId>,     // strip order (drag-reorder)
    pub active: Option<TabId>,
    pub closed: Vec<(usize, QueryTab)>, // reopen stack — parked tab + its strip index at close
    /// A throwaway editor buffer the [`EditorTab`](crate::apps::project::views::workbench) slice
    /// falls back to when its tab was closed mid-event. Closing the active tab (nav-dropdown ×)
    /// fires the editor's commit-on-click-outside *after* the close removed the tab, so its
    /// slice write lands here (and is discarded) instead of panicking on a missing tab.
    pub scratch: Option<CodeEditorData>,
}

impl SessionState {
    // --- reads ------------------------------------------------------------

    pub fn active_tab(&self) -> Option<&QueryTab> {
        self.active.and_then(|id| self.tabs.get(&id))
    }

    pub fn can_reopen(&self) -> bool {
        !self.closed.is_empty()
    }

    // --- structural mutations (each leaves a valid `active`) --------------

    /// Append a new blank scratch tab (⌘T) and focus it.
    pub fn open_blank(&mut self) -> TabId {
        let name = self.next_query_name();
        self.push_active(QueryTab::scratch(name))
    }

    /// Append `sql` bound to `origin`, uniquely named, and focus it.
    pub fn open_named(&mut self, name: &str, sql: String, origin: Origin) -> TabId {
        let name = self.unique_name(name);
        self.push_active(QueryTab::new(name, sql, origin))
    }

    /// Duplicate `id` into a new scratch tab immediately to its right, and focus it.
    pub fn duplicate(&mut self, id: TabId) {
        let Some(src) = self.tabs.get(&id) else {
            return;
        };
        let name = self.unique_name(&format!("{} copy", src.name));
        let text = src.text();
        let pos = self
            .order
            .iter()
            .position(|t| *t == id)
            .map_or(self.order.len(), |p| p + 1);
        let tab = QueryTab::new(name, text, Origin::Scratch);
        let new_id = tab.id;
        self.tabs.insert(new_id, tab);
        self.order.insert(pos, new_id);
        self.active = Some(new_id);
    }

    /// Focus `id` (no-op if absent).
    pub fn switch(&mut self, id: TabId) {
        if self.tabs.contains_key(&id) {
            self.active = Some(id);
        }
    }

    /// Rename `id` (caller ignores empty names).
    pub fn rename(&mut self, id: TabId, name: String) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.name = name;
        }
    }

    /// Drag-to-reorder: move `id` to the `insert` slot in the visible order.
    pub fn move_tab(&mut self, id: TabId, insert: usize) {
        let Some(from) = self.order.iter().position(|t| *t == id) else {
            return;
        };
        if insert == from {
            return;
        }
        let moved = self.order.remove(from);
        let to = insert.min(self.order.len());
        self.order.insert(to, moved);
    }

    /// Close `id`, parking it (with its strip index) on the reopen stack; refocus a neighbour
    /// if it was active.
    pub fn close_one(&mut self, id: TabId) {
        let pos = self.order.iter().position(|t| *t == id);
        let active_pos = self
            .active
            .and_then(|a| self.order.iter().position(|t| *t == a));
        let was_active = self.active == Some(id);

        if let Some(tab) = self.tabs.remove(&id) {
            let at = pos.unwrap_or(self.order.len());
            self.closed.push((at, tab));
            let overflow = self.closed.len().saturating_sub(CLOSED_CAP);
            if overflow > 0 {
                self.closed.drain(0..overflow);
            }
        }
        self.order.retain(|t| *t != id);

        if was_active {
            self.active = if self.order.is_empty() {
                None
            } else {
                let p = active_pos.unwrap_or(0).min(self.order.len() - 1);
                Some(self.order[p])
            };
        }
    }

    /// Close every open tab, parking each (with its strip index) on the reopen stack so they can be
    /// brought back one-by-one; leaves the session empty.
    pub fn close_all(&mut self) {
        for (at, id) in std::mem::take(&mut self.order).into_iter().enumerate() {
            if let Some(tab) = self.tabs.remove(&id) {
                self.closed.push((at, tab));
            }
        }
        let overflow = self.closed.len().saturating_sub(CLOSED_CAP);
        if overflow > 0 {
            self.closed.drain(0..overflow);
        }
        self.active = None;
    }

    /// Close every tab *except* `id`, parking each on the reopen stack (with its strip index);
    /// leaves `id` the only open tab, and active.
    pub fn close_others(&mut self, id: TabId) {
        if !self.tabs.contains_key(&id) {
            return;
        }
        let victims: Vec<(usize, TabId)> = self
            .order
            .iter()
            .enumerate()
            .filter(|(_, t)| **t != id)
            .map(|(i, t)| (i, *t))
            .collect();
        for (at, tid) in victims {
            if let Some(tab) = self.tabs.remove(&tid) {
                self.closed.push((at, tab));
            }
        }
        self.order.retain(|t| *t == id);
        let overflow = self.closed.len().saturating_sub(CLOSED_CAP);
        if overflow > 0 {
            self.closed.drain(0..overflow);
        }
        self.active = Some(id);
    }

    /// Close every tab to the *right* of `id` in strip order, parking each on the reopen stack. `id`
    /// stays; if the active tab was among those closed, `id` takes focus.
    pub fn close_right(&mut self, id: TabId) {
        let Some(from) = self.order.iter().position(|t| *t == id) else {
            return;
        };
        let victims: Vec<(usize, TabId)> = self
            .order
            .iter()
            .enumerate()
            .skip(from + 1)
            .map(|(i, t)| (i, *t))
            .collect();
        if victims.is_empty() {
            return;
        }
        let active_closed = self
            .active
            .is_some_and(|a| victims.iter().any(|(_, t)| *t == a));
        for (at, tid) in &victims {
            if let Some(tab) = self.tabs.remove(tid) {
                self.closed.push((*at, tab));
            }
        }
        self.order.truncate(from + 1);
        let overflow = self.closed.len().saturating_sub(CLOSED_CAP);
        if overflow > 0 {
            self.closed.drain(0..overflow);
        }
        if active_closed {
            self.active = Some(id);
        }
    }

    /// Re-open the most recently closed tab (⇧⌘T), restoring its full editor state at (close to)
    /// its original strip position.
    pub fn reopen_last(&mut self) {
        if let Some((at, tab)) = self.closed.pop() {
            let id = tab.id;
            self.tabs.insert(id, tab);
            let at = at.min(self.order.len());
            self.order.insert(at, id);
            // Reopen focuses the restored tab (⇧⌘T), matching the Dioxus behaviour and browsers.
            self.active = Some(id);
        }
    }

    // --- internals --------------------------------------------------------

    fn push_active(&mut self, tab: QueryTab) -> TabId {
        let id = tab.id;
        self.tabs.insert(id, tab);
        self.order.push(id);
        self.active = Some(id);
        id
    }

    /// The first free `query N` name.
    fn next_query_name(&self) -> String {
        (1..)
            .map(|i| format!("query {i}"))
            .find(|c| !self.name_taken(c))
            .unwrap()
    }

    /// `base`, else `base 2`, `base 3`, … — the first that doesn't collide.
    fn unique_name(&self, base: &str) -> String {
        if !self.name_taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|i| format!("{base} {i}"))
            .find(|c| !self.name_taken(c))
            .unwrap()
    }

    fn name_taken(&self, name: &str) -> bool {
        self.tabs.values().any(|t| t.name == name)
    }
}
