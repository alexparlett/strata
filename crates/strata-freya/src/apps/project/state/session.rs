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
use strata_model::Diagnostic;
use uuid::Uuid;

use crate::apps::project::query::QuerySpec;

/// Stable per-tab identity — real identity, so no allocator and no duplicate-id repair.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TabId(pub Uuid);

impl TabId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// What a tab is bound to — its **save target** only. Dirty comes from the editor, not this.
///
/// Keys mirror the Project store's identity rules: a view's key is its **name** (the
/// engine/SQL identity — a view rename goes through the Project store, which rewrites
/// these), a saved query's is its stable **id** (its name is only a label, so renames
/// can't dangle a tab).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Origin {
    Scratch,
    View(String),
    SavedQuery(Uuid),
}

/// Which body the results pane shows for a settled rows outcome — the toolbar's Table/Chart
/// segmented toggle (P2-07). Per **tab** (CHART_SPEC §1): switching tabs restores the mode,
/// and it survives re-runs; the chart *config* will be per result set (Chart workstream).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ResultsView {
    #[default]
    Grid,
    Chart,
}

/// One query tab. Owns its editing buffer exactly like Valin's `EditorTab`, and its own
/// Run trigger — the latest run request, whose results the tab's pane shows.
pub struct QueryTab {
    pub id: TabId,
    pub name: String,
    pub editor: CodeEditorData,
    pub origin: Origin,
    /// The tab's Run trigger (state-arch §6): the latest run request. Editing never touches
    /// it — only a Run press rebuilds it (fresh nonce → new execution) and only Cancel /
    /// Trash clear it; the results themselves live in the freya-query cache, keyed by this
    /// spec. Scoped to the tab, so no other tab's request (or cancel) can disturb it.
    /// Reads/writes go through [`Chan::Request`](super::Chan) — its own channel, so
    /// keystrokes (on `Chan::Tab`) never wake the results pane.
    pub request: Option<QuerySpec>,
    /// The results view mode (Table/Chart toggle). Its own channel too
    /// ([`Chan::View`](super::Chan)) — a flip wakes only the tab's results pane.
    pub view: ResultsView,
    /// The tab's current validation diagnostics (P2-18): the debounced engine
    /// dry-plan pass over the editor text. Its own channel
    /// ([`Chan::Diagnostics`](super::Chan)); the editor's squiggles are the same
    /// facts, carried as decorations *inside* the buffer (set by the same pass).
    pub diagnostics: Vec<Diagnostic>,
    /// The buffer [`revision`](CodeEditorData::revision) `diagnostics` was computed
    /// for — what makes them checkable for staleness: the Run gate only blocks on
    /// errors that describe the buffer *as it stands* (see
    /// [`SessionState::blocking_errors`]), never on leftovers from text since edited.
    pub diagnostics_rev: Option<u64>,
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
        // Populate the line blocks so the editor renders its content immediately. Measurement is
        // the mounted `CodeEditor`'s job — it measures with its theme-resolved type on mount
        // (the session doesn't know the editor's font). `mark_as_saved` then snapshots the
        // opening text as the dirty baseline so a freshly-opened tab isn't "edited".
        editor.parse();
        editor.mark_as_saved();
        Self {
            id: TabId::new(),
            name,
            editor,
            origin,
            request: None,
            view: ResultsView::default(),
            diagnostics: Vec::new(),
            diagnostics_rev: None,
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

    /// The tab's current run request, if any.
    pub fn request(&self, id: TabId) -> Option<&QuerySpec> {
        self.tabs.get(&id).and_then(|t| t.request.as_ref())
    }

    /// Set `id`'s Run trigger (a Run / Explain / Analyze press). Write on
    /// [`Chan::Request(id)`](super::Chan).
    pub fn set_request(&mut self, id: TabId, spec: QuerySpec) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.request = Some(spec);
        }
    }

    /// Drop `id`'s Run trigger (Cancel / Trash), returning its pane to empty. Write on
    /// [`Chan::Request(id)`](super::Chan).
    pub fn clear_request(&mut self, id: TabId) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.request = None;
        }
    }

    /// The tab's results view mode (a missing tab reads Grid — the default).
    pub fn view(&self, id: TabId) -> ResultsView {
        self.tabs.get(&id).map(|t| t.view).unwrap_or_default()
    }

    /// The tab's current validation diagnostics (P2-18). Read on
    /// [`Chan::Diagnostics(id)`](super::Chan).
    pub fn diagnostics(&self, id: TabId) -> &[Diagnostic] {
        self.tabs.get(&id).map(|t| t.diagnostics.as_slice()).unwrap_or(&[])
    }

    /// Replace `id`'s validation diagnostics (a validation pass settling), stamped
    /// with the buffer revision they were computed for. Write on
    /// [`Chan::Diagnostics(id)`](super::Chan).
    pub fn set_diagnostics(&mut self, id: TabId, diagnostics: Vec<Diagnostic>, rev: u64) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.diagnostics = diagnostics;
            t.diagnostics_rev = Some(rev);
        }
    }

    /// Whether `id` has validation **errors** that describe the buffer as it stands —
    /// the Run gate. Stale diagnostics (the buffer was edited since the last pass)
    /// never block: a just-fixed query runs immediately, and a just-broken one is the
    /// engine's to reject like before. Warnings never block.
    pub fn blocking_errors(&self, id: TabId) -> bool {
        self.tabs.get(&id).is_some_and(|t| {
            t.diagnostics_rev == Some(t.editor.revision())
                && t.diagnostics.iter().any(|d| d.is_error())
        })
    }

    /// Flip `id`'s results view (the toolbar's Table/Chart toggle). Write on
    /// [`Chan::View(id)`](super::Chan).
    pub fn set_view(&mut self, id: TabId, view: ResultsView) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.view = view;
        }
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

    /// A save landed: bind `id` to its save target and reset the editor's dirty
    /// baseline to the text just saved (state-arch §4 — the only session mutation a
    /// save makes). A Save-As passes `name` to also rename the tab to its target.
    pub fn bind_saved(&mut self, id: TabId, name: Option<String>, origin: Origin) {
        if let Some(t) = self.tabs.get_mut(&id) {
            if let Some(name) = name {
                t.name = name;
            }
            t.origin = origin;
            t.editor.mark_as_saved();
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

        if let Some(mut tab) = self.tabs.remove(&id) {
            // Parked without its request: reopen starts with no results, like a fresh tab —
            // matching the engine-side cleanup (SNAPSHOT_SPEC §4, the root's tab-diff funnel).
            tab.request = None;
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
            if let Some(mut tab) = self.tabs.remove(&id) {
                tab.request = None;
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
            if let Some(mut tab) = self.tabs.remove(&tid) {
                tab.request = None;
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
            if let Some(mut tab) = self.tabs.remove(tid) {
                tab.request = None;
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

    /// Whether any tab — open **or** parked on the reopen stack — already wears `name`. Parked
    /// tabs must count: otherwise a name freed by closing gets handed to a new tab, and reopening
    /// the closed original resurrects a duplicate (close "query 1", open a new tab → "query 1",
    /// reopen → two "query 1"s). Both auto-naming paths ([`next_query_name`](Self::next_query_name)
    /// and [`unique_name`](Self::unique_name)) route through here.
    fn name_taken(&self, name: &str) -> bool {
        self.tabs.values().any(|t| t.name == name)
            || self.closed.iter().any(|(_, t)| t.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The session side of a save (state-arch §4): the tab rebinds to its target, a
    /// Save-As renames it, and the editor's dirty baseline resets to the saved text.
    #[test]
    fn bind_saved_rebinds_and_resets_dirty() {
        let mut s = SessionState::default();
        let id = s.open_named("query 1", "SELECT 1".into(), Origin::Scratch);

        s.tabs.get_mut(&id).unwrap().editor.set_text("SELECT 2");
        s.bind_saved(id, Some("saved_view_1".into()), Origin::View("saved_view_1".into()));

        let t = &s.tabs[&id];
        assert_eq!(t.name, "saved_view_1");
        assert!(matches!(&t.origin, Origin::View(v) if v == "saved_view_1"));
        assert!(!t.is_dirty(), "a save resets the dirty baseline");

        // The next divergence reads dirty again — the baseline moved, not froze.
        s.tabs.get_mut(&id).unwrap().editor.set_text("SELECT 3");
        assert!(s.tabs[&id].is_dirty());
    }

    /// Auto-naming accounts for parked (closed) tabs, so reopening never resurrects a
    /// duplicate: close "query 1", open a fresh tab, reopen — the reopened tab keeps its
    /// name and the new one took the next free index instead of colliding.
    #[test]
    fn auto_name_skips_parked_tabs_so_reopen_never_collides() {
        let mut s = SessionState::default();

        let id1 = s.open_blank();
        assert_eq!(s.tabs[&id1].name, "query 1");

        s.close_one(id1);
        // The freed name is still parked, so the new tab must not reuse it.
        let id2 = s.open_blank();
        assert_eq!(s.tabs[&id2].name, "query 2");

        s.reopen_last();
        let names: Vec<&str> = s.order.iter().map(|id| s.tabs[id].name.as_str()).collect();
        assert_eq!(names.len(), 2, "both tabs are open");
        assert!(names.contains(&"query 1") && names.contains(&"query 2"), "no duplicate name");
    }

    /// The Run gate (P2-18): current errors block, stale ones (buffer edited since the
    /// pass) don't, and warnings never do.
    #[test]
    fn blocking_errors_respects_buffer_revision_and_severity() {
        use strata_model::{DiagSource, Severity};

        let mut s = SessionState::default();
        let id = s.open_named("q", "SELECT * FROM nope".into(), Origin::Scratch);
        let diag = |severity| Diagnostic {
            severity,
            source: DiagSource::Validation,
            message: "x".into(),
            loc: None,
            span: None,
        };

        let rev = s.tabs[&id].editor.revision();
        s.set_diagnostics(id, vec![diag(Severity::Warning)], rev);
        assert!(!s.blocking_errors(id), "warnings never block");

        s.set_diagnostics(id, vec![diag(Severity::Error)], rev);
        assert!(s.blocking_errors(id), "a current error blocks");

        // An edit outdates the pass — a just-fixed buffer must not stay locked.
        s.tabs.get_mut(&id).unwrap().editor.set_text("SELECT 1");
        assert!(!s.blocking_errors(id), "stale errors never block");
    }
}
