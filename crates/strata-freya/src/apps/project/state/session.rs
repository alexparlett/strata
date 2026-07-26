//! The per-window **Session**: the open query tabs and their arrangement.
//!
//! One window == one [`SessionState`] (a Radio store, provided in the window root). Each tab is
//! a stateful [`QueryTab`] that **owns its editor buffer** ([`CodeEditorData`]) — the Valin
//! pattern: the buffer lives in the store, keyed by [`TabId`], and the editor slices a
//! `Writable` into it. Dirty is the editor's own `is_edited()`; closing/reopening **moves** the
//! whole tab (no snapshot).
//!
//! Persistence mirrors the Project store's (`project.rs`): the live tabs project to a serde
//! [`SessionSnapshot`] (`strata_model`) and rebuild from one — [`SessionState::snapshot`] /
//! [`SessionState::from_snapshot`]. The `.strata/session.json` IO is `strata-core::project`.

use std::collections::HashMap;
use std::mem;

use strata_code_editor::prelude::{CodeEditorData, EditorLanguage, Rope};
use strata_model::{
    expanded_drawer_h, Diagnostic, DrawerTab, Layout, Origin, ResultsView, SessionSnapshot,
    SidebarPane, TabId, TabSnapshot,
};
use uuid::Uuid;

use crate::apps::project::query::QuerySpec;

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
    /// Purely advisory — Run is never gated on them (P2-23: diagnostics advise,
    /// the engine decides; a doomed run fails at plan time with the same error).
    pub diagnostics: Vec<Diagnostic>,
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
        }
    }

    /// A blank scratch buffer.
    pub fn scratch(name: String) -> Self {
        Self::new(name, String::new(), Origin::Scratch)
    }

    /// Rebuild a tab from a persisted snapshot (P4-14 load). Like [`new`](Self::new) but
    /// **keeps the original [`TabId`]** — so the snapshot's `active` / order still resolve
    /// against it — and restores its results-view intent. Marked saved at the restored
    /// text (a reopened project starts clean, like a freshly loaded artifact); the
    /// validation pass re-derives diagnostics once the pane mounts.
    pub fn restored(
        id: TabId,
        name: String,
        sql: String,
        origin: Origin,
        view: ResultsView,
    ) -> Self {
        let mut editor = CodeEditorData::new(Rope::from_str(&sql), Some(sql_language()));
        editor.parse();
        editor.mark_as_saved();
        Self {
            id,
            name,
            editor,
            origin,
            request: None,
            view,
            diagnostics: Vec::new(),
        }
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
    pub order: Vec<TabId>, // strip order (drag-reorder)
    pub active: Option<TabId>,
    pub closed: Vec<(usize, QueryTab)>, // reopen stack — parked tab + its strip index at close
    /// A throwaway editor buffer the `EditorTab` slice
    /// falls back to when its tab was closed mid-event. Closing the active tab (nav-dropdown ×)
    /// fires the editor's commit-on-click-outside *after* the close removed the tab, so its
    /// slice write lands here (and is discarded) instead of panicking on a missing tab.
    pub scratch: Option<CodeEditorData>,
    /// The window's panel-layout arrangement (P3-01): which side panels / drawer are open, on
    /// which pane / tab, and each resizable panel's last size. Structure changes write on
    /// [`Chan::Layout`](super::Chan) (the shell + rail subscribe); size captures write on
    /// [`Chan::LayoutSize`](super::Chan) (nobody subscribes — the shell peeks it to seed panel
    /// sizes). Both derive [`Chan::Persist`](super::Chan), so it rides session autosave.
    pub layout: Layout,
}

impl SessionState {
    // --- reads ------------------------------------------------------------

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
        self.tabs
            .get(&id)
            .map(|t| t.diagnostics.as_slice())
            .unwrap_or(&[])
    }

    /// Replace `id`'s validation diagnostics (a validation pass settling). Write on
    /// [`Chan::Diagnostics(id)`](super::Chan).
    pub fn set_diagnostics(&mut self, id: TabId, diagnostics: Vec<Diagnostic>) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.diagnostics = diagnostics;
        }
    }

    /// Flip `id`'s results view (the toolbar's Table/Chart toggle). Write on
    /// [`Chan::View(id)`](super::Chan).
    pub fn set_view(&mut self, id: TabId, view: ResultsView) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.view = view;
        }
    }

    // --- layout (P3-01) ---------------------------------------------------
    // Structure toggles write on `Chan::Layout`; the size setters on `Chan::LayoutSize`.

    /// The rail's top-group toggle (design `onRailPane`): open the sidebar on `pane`, or —
    /// if it's already showing `pane` — collapse it.
    pub fn toggle_pane(&mut self, pane: SidebarPane) {
        self.layout.sidebar = (self.layout.sidebar != Some(pane)).then_some(pane);
    }

    /// Collapse the sidebar (design `onToggleSidebar` / the sidebar header ×).
    pub fn close_sidebar(&mut self) {
        self.layout.sidebar = None;
    }

    /// The rail's bottom-group toggle (design `onOpen{Problems,Events,History}`): open the
    /// drawer on `tab`, or — if it's already showing `tab` — collapse it.
    pub fn toggle_drawer(&mut self, tab: DrawerTab) {
        self.layout.drawer = (self.layout.drawer != Some(tab)).then_some(tab);
    }

    /// Collapse the drawer (its header ×).
    pub fn close_drawer(&mut self) {
        self.layout.drawer = None;
    }

    /// The drawer header's expand / restore toggle (design `onToggleLogHeight`): raise the
    /// drawer to [`expanded_drawer_h`], remembering the height it had, or put that height
    /// back. The remembered height *is* the expanded flag, so the icon can never disagree
    /// with the height.
    ///
    /// A structure write ([`Chan::Layout`](super::Chan)), not a size one: it re-seeds the
    /// panel's `initial_size` for its next mount (a collapse→reopen, or a restart), which the
    /// per-frame drag channel deliberately never wakes. Moving the *live* panel is the
    /// container controller's job — see `views::shell::set_drawer_panel_height`.
    ///
    /// Dragging the drawer while it is expanded leaves it expanded: the drag records a new
    /// height on `drawer_h`, and restore still returns to the pre-expand one. Restore means
    /// "back to before I expanded", not "back to the last drag".
    ///
    /// Returns the height it settled on, so the caller can drive the live panel to it without
    /// re-deriving the rule.
    pub fn toggle_drawer_height(&mut self) -> f32 {
        self.layout.drawer_h = match self.layout.drawer_restore_h.take() {
            Some(restore) => restore,
            None => {
                self.layout.drawer_restore_h = Some(self.layout.drawer_h);
                expanded_drawer_h()
            }
        };
        self.layout.drawer_h
    }

    /// Collapse the inspector (its header ×).
    pub fn close_inspector(&mut self) {
        self.layout.inspector_open = false;
    }

    /// Reveal the inspector — selecting a catalog column (P3-02) is the way it reopens once
    /// collapsed. Idempotent, so every selection can call it without checking first.
    pub fn open_inspector(&mut self) {
        self.layout.inspector_open = true;
    }

    /// Remember the sidebar's dragged width (a `ResizableContainer` resize). Write on
    /// [`Chan::LayoutSize`](super::Chan) so it persists without waking the shell.
    pub fn set_sidebar_w(&mut self, w: f32) {
        self.layout.sidebar_w = w;
    }

    /// Remember the inspector's dragged width. Write on [`Chan::LayoutSize`](super::Chan).
    pub fn set_inspector_w(&mut self, w: f32) {
        self.layout.inspector_w = w;
    }

    /// Remember the drawer's dragged height. Write on [`Chan::LayoutSize`](super::Chan).
    pub fn set_drawer_h(&mut self, h: f32) {
        self.layout.drawer_h = h;
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

    /// Open a catalog row in a tab — or focus the tab that **already is** it (P3-06).
    ///
    /// Two ways a tab can already be this, matching the two kinds of identity in the catalog:
    ///
    /// - it is **bound** to the same save target (`Origin::View` / `Origin::SavedQuery`). This
    ///   is the one that matters: opening a view a second time must not leave two tabs both
    ///   saving to it, and if the first has unsaved edits, that tab *is* what the user means by
    ///   "edit this view". Its buffer is left exactly as it is.
    /// - it is an unedited tab of the same **name and text** — how a repeated "View table" finds
    ///   the `SELECT *` tab it opened a moment ago instead of stacking `orders 2`, `orders 3`.
    ///   Edit it and the next press opens a fresh tab, which is the honest reading: the buffer
    ///   is no longer the thing the row would have opened.
    ///
    /// Anything else is a new tab under a unique name, exactly as [`open_named`](Self::open_named).
    pub fn open_or_focus(&mut self, name: &str, sql: String, origin: Origin) -> TabId {
        let existing = self.order.iter().copied().find(|id| {
            self.tabs.get(id).is_some_and(|t| match &origin {
                Origin::View(view) => matches!(&t.origin, Origin::View(v) if v == view),
                Origin::SavedQuery(query) => {
                    matches!(&t.origin, Origin::SavedQuery(q) if q == query)
                }
                Origin::Scratch => t.name == name && t.text() == sql,
            })
        });
        if let Some(id) = existing {
            self.active = Some(id);
            return id;
        }
        self.open_named(name, sql, origin)
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

    /// The save target of every tab bound to the view `name` is gone (P3-05's drop) — cut the
    /// binding, keep the buffer.
    ///
    /// The tab is not closed and its SQL is not touched: the user still has the text, which is
    /// what the drop confirm promises. What must not survive is the *binding* — a tab left on
    /// `Origin::View("orders_daily")` would re-create the view the user just dropped on the next
    /// ⌘S, silently undoing the drop. As a scratch tab it saves as a new saved query instead,
    /// which is what an unbound buffer means everywhere else.
    ///
    /// Matched exactly, not folded: both sides are this store's own strings (a bind copies the
    /// def name), and a view rename rewrites them through the Project store.
    pub fn unbind_view(&mut self, name: &str) {
        for t in self.tabs.values_mut() {
            if matches!(&t.origin, Origin::View(v) if v == name) {
                t.origin = Origin::Scratch;
            }
        }
    }

    /// Whether any open tab saves to the view `name` — the guard that keeps a drop of a view no
    /// tab is bound to from taking a `Chan::Tabs` write guard. That write notifies whether or not
    /// it changed anything, and `Chan::Tabs` derives `Chan::Persist`, so an unguarded call
    /// re-renders the tab strip and schedules a rewrite of `session.json` for a session that did
    /// not change.
    pub fn is_bound_to_view(&self, name: &str) -> bool {
        self.tabs
            .values()
            .any(|t| matches!(&t.origin, Origin::View(v) if v == name))
    }

    /// The saved-query counterpart of [`is_bound_to_view`](Self::is_bound_to_view).
    pub fn is_bound_to_query(&self, id: Uuid) -> bool {
        self.tabs
            .values()
            .any(|t| matches!(&t.origin, Origin::SavedQuery(q) if *q == id))
    }

    /// The saved query `id` is gone (P3-05's delete) — cut the binding, keep the buffer. Same
    /// reasoning as [`unbind_view`](Self::unbind_view); addressed by id because that is a saved
    /// query's identity.
    pub fn unbind_saved_query(&mut self, id: Uuid) {
        for t in self.tabs.values_mut() {
            if matches!(&t.origin, Origin::SavedQuery(q) if *q == id) {
                t.origin = Origin::Scratch;
            }
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
        for (at, id) in mem::take(&mut self.order).into_iter().enumerate() {
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

    // --- persistence (project.rs mirrors this for the Project store) -------

    /// Project the live session to its serde [`SessionSnapshot`] (state-arch §5), walking
    /// tabs in strip order so the file preserves the visible arrangement. Window geometry
    /// isn't in `SessionState` — the autosave hook fills it from `Platform` before writing.
    pub fn snapshot(&self) -> SessionSnapshot {
        let tabs = self
            .order
            .iter()
            .filter_map(|id| self.tabs.get(id))
            .map(|t| TabSnapshot {
                id: t.id,
                name: t.name.clone(),
                origin: t.origin.clone(),
                text: t.text(),
                view: t.view,
            })
            .collect();
        SessionSnapshot {
            tabs,
            active: self.active,
            window: None,
            layout: self.layout,
        }
    }

    /// Rebuild a session from a persisted snapshot (state-arch §5). Each tab comes back with
    /// its full editor buffer (marked clean — its saved baseline is the persisted text).
    /// `active` is validated against the restored tabs: a dangling id falls back to the first
    /// tab, never leaving the session pointed at a tab that isn't there. The reopen stack
    /// starts empty. `None` when the snapshot has no tabs, so the caller can open a fresh
    /// blank one instead of restoring an empty window.
    pub fn from_snapshot(snap: SessionSnapshot) -> Option<Self> {
        if snap.tabs.is_empty() {
            return None;
        }
        let mut tabs = HashMap::with_capacity(snap.tabs.len());
        let mut order = Vec::with_capacity(snap.tabs.len());
        for t in snap.tabs {
            let tab = QueryTab::restored(t.id, t.name, t.text, t.origin, t.view);
            order.push(tab.id);
            tabs.insert(tab.id, tab);
        }
        let active = snap
            .active
            .filter(|id| tabs.contains_key(id))
            .or_else(|| order.first().copied());
        Some(SessionState {
            tabs,
            order,
            active,
            closed: Vec::new(),
            scratch: None,
            layout: snap.layout,
        })
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
        s.bind_saved(
            id,
            Some("saved_view_1".into()),
            Origin::View("saved_view_1".into()),
        );

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
        assert!(
            names.contains(&"query 1") && names.contains(&"query 2"),
            "no duplicate name"
        );
    }

    /// Opening a bound catalog row twice focuses the tab that is already saving to it rather
    /// than minting a second one — **including** when that tab has diverged, because a tab with
    /// unsaved edits to a view is precisely what "edit this view" should land on. Two tabs on
    /// one `Origin::View` would mean two ⌘S targets for the same view.
    #[test]
    fn opening_a_bound_row_twice_focuses_the_tab_already_bound_to_it() {
        let mut s = SessionState::default();
        let first = s.open_or_focus(
            "orders_daily",
            "SELECT * FROM orders".into(),
            Origin::View("orders_daily".into()),
        );
        s.open_blank();
        s.tabs
            .get_mut(&first)
            .unwrap()
            .editor
            .set_text("SELECT 1 -- half-written");

        let again = s.open_or_focus(
            "orders_daily",
            "SELECT * FROM orders".into(),
            Origin::View("orders_daily".into()),
        );

        assert_eq!(again, first, "the bound tab, not a new one");
        assert_eq!(s.active, Some(first), "and it is focused");
        assert_eq!(s.tabs.len(), 2, "no third tab");
        assert_eq!(
            s.tabs[&first].text(),
            "SELECT 1 -- half-written",
            "the buffer is left exactly as the user left it"
        );
    }

    /// A scratch row (View table's `SELECT *`) has no binding to match on, so it reuses an
    /// untouched tab of the same name and text — and stops reusing it the moment that buffer is
    /// edited, since it is then no longer what the row would have opened.
    #[test]
    fn view_table_reuses_its_own_untouched_tab_but_not_an_edited_one() {
        let mut s = SessionState::default();
        let sql = "SELECT *\nFROM orders\nLIMIT 100;".to_string();
        let first = s.open_or_focus("orders", sql.clone(), Origin::Scratch);

        assert_eq!(
            s.open_or_focus("orders", sql.clone(), Origin::Scratch),
            first,
            "pressing again lands on the tab it just opened"
        );

        s.tabs.get_mut(&first).unwrap().editor.set_text("SELECT 1");
        let second = s.open_or_focus("orders", sql, Origin::Scratch);

        assert_ne!(second, first, "an edited buffer is not reused");
        assert_eq!(
            s.tabs[&second].name, "orders 2",
            "and the new tab takes the next free name"
        );
    }

    /// Snapshot → restore preserves order, active, text, origin and view; the reopen stack
    /// and diagnostics do not travel.
    #[test]
    fn snapshot_round_trips_tabs_order_active_and_view() {
        let mut s = SessionState::default();
        let a = s.open_named("alpha", "SELECT 1".into(), Origin::View("alpha".into()));
        let b = s.open_named("beta", "SELECT 2".into(), Origin::Scratch);
        s.set_view(b, ResultsView::Chart);
        s.switch(a);

        let restored =
            SessionState::from_snapshot(s.snapshot()).expect("non-empty snapshot restores");

        assert_eq!(restored.order, vec![a, b], "strip order preserved");
        assert_eq!(restored.active, Some(a), "active tab preserved");
        assert!(restored.closed.is_empty(), "reopen stack is not persisted");

        let ra = &restored.tabs[&a];
        assert_eq!(ra.text(), "SELECT 1");
        assert!(matches!(&ra.origin, Origin::View(v) if v == "alpha"));
        assert_eq!(ra.view, ResultsView::Grid);
        assert!(!ra.is_dirty(), "a restored bound tab starts clean");

        let rb = &restored.tabs[&b];
        assert_eq!(rb.text(), "SELECT 2");
        assert_eq!(rb.view, ResultsView::Chart, "per-tab view intent preserved");
    }

    /// The rail toggles follow the design's `onRailPane` / `onOpen*` semantics: toggling the
    /// active pane/tab collapses it, toggling another switches to it, and the × closers
    /// collapse.
    #[test]
    fn layout_toggles_follow_design_semantics() {
        let mut s = SessionState::default();
        // Defaults: sidebar open on Catalog, inspector open, drawer collapsed.
        assert_eq!(s.layout.sidebar, Some(SidebarPane::Catalog));
        assert!(s.layout.inspector_open);
        assert_eq!(s.layout.drawer, None);

        // Toggling the *active* pane collapses the sidebar; toggling while collapsed reopens
        // it on that pane; toggling a *different* pane switches without collapsing.
        s.toggle_pane(SidebarPane::Catalog);
        assert_eq!(s.layout.sidebar, None);
        s.toggle_pane(SidebarPane::Connections);
        assert_eq!(s.layout.sidebar, Some(SidebarPane::Connections));
        s.toggle_pane(SidebarPane::Catalog);
        assert_eq!(s.layout.sidebar, Some(SidebarPane::Catalog));
        s.close_sidebar();
        assert_eq!(s.layout.sidebar, None);

        // Drawer: open on a tab, switch tabs, then toggle the active tab off.
        s.toggle_drawer(DrawerTab::Problems);
        assert_eq!(s.layout.drawer, Some(DrawerTab::Problems));
        s.toggle_drawer(DrawerTab::History);
        assert_eq!(s.layout.drawer, Some(DrawerTab::History));
        s.toggle_drawer(DrawerTab::History);
        assert_eq!(s.layout.drawer, None);

        s.close_inspector();
        assert!(!s.layout.inspector_open);
    }

    /// The drawer header's expand / restore toggle (P3-11): expanding remembers the height it
    /// left and restoring puts exactly that back — including a height the user dragged to,
    /// which the design's two-fixed-heights version threw away.
    #[test]
    fn drawer_expand_restores_the_height_it_left() {
        let mut s = SessionState::default();
        s.set_drawer_h(310.0);
        assert_eq!(s.layout.drawer_restore_h, None, "not expanded to start");

        assert_eq!(s.toggle_drawer_height(), expanded_drawer_h());
        assert_eq!(s.layout.drawer_h, expanded_drawer_h());
        assert_eq!(s.layout.drawer_restore_h, Some(310.0), "and is expanded");

        assert_eq!(s.toggle_drawer_height(), 310.0, "the dragged height is back");
        assert_eq!(s.layout.drawer_h, 310.0);
        assert_eq!(s.layout.drawer_restore_h, None, "no longer expanded");
    }

    /// Dragging while expanded records the new height but leaves the drawer expanded: restore
    /// means "back to before I expanded", not "back to the last drag". Deliberate — clearing
    /// the flag on drag would need a `Chan::Layout` write per drag frame, which the size
    /// channel exists to avoid.
    #[test]
    fn dragging_while_expanded_keeps_the_pre_expand_height() {
        let mut s = SessionState::default();
        let before = s.layout.drawer_h;
        s.toggle_drawer_height();
        s.set_drawer_h(400.0);

        assert_eq!(s.layout.drawer_restore_h, Some(before));
        assert_eq!(s.toggle_drawer_height(), before);
    }

    /// Layout (open panes + sizes) survives the snapshot round-trip alongside the tabs — so a
    /// restart restores the same shell arrangement.
    #[test]
    fn snapshot_round_trips_layout() {
        let mut s = SessionState::default();
        s.open_named("a", "SELECT 1".into(), Origin::Scratch);
        s.toggle_pane(SidebarPane::Connections);
        s.close_inspector();
        s.toggle_drawer(DrawerTab::Events);
        s.set_sidebar_w(333.0);
        s.set_drawer_h(199.0);

        let restored =
            SessionState::from_snapshot(s.snapshot()).expect("non-empty snapshot restores");
        assert_eq!(restored.layout.sidebar, Some(SidebarPane::Connections));
        assert!(!restored.layout.inspector_open);
        assert_eq!(restored.layout.drawer, Some(DrawerTab::Events));
        assert_eq!(restored.layout.sidebar_w, 333.0);
        assert_eq!(restored.layout.drawer_h, 199.0);
    }

    /// An expanded drawer restarts expanded, with the height to restore — both halves of the
    /// toggle's state ride the snapshot. Its own test rather than a line in
    /// [`snapshot_round_trips_layout`], which has to keep asserting that an ordinary *dragged*
    /// height round-trips.
    #[test]
    fn snapshot_round_trips_an_expanded_drawer() {
        let mut s = SessionState::default();
        s.open_named("a", "SELECT 1".into(), Origin::Scratch);
        s.set_drawer_h(199.0);
        s.toggle_drawer_height();

        let restored =
            SessionState::from_snapshot(s.snapshot()).expect("non-empty snapshot restores");
        assert_eq!(restored.layout.drawer_h, expanded_drawer_h());
        assert_eq!(restored.layout.drawer_restore_h, Some(199.0));
    }

    /// A dangling `active` (its tab dropped from the snapshot) falls back to the first tab
    /// rather than restoring a session pointed at nothing.
    #[test]
    fn from_snapshot_repairs_dangling_active() {
        let mut s = SessionState::default();
        let a = s.open_named("a", "SELECT 1".into(), Origin::Scratch);
        let _b = s.open_named("b", "SELECT 2".into(), Origin::Scratch);
        let mut snap = s.snapshot();
        snap.active = Some(TabId::new()); // a tab that's no longer in the list

        let restored = SessionState::from_snapshot(snap).unwrap();
        assert_eq!(restored.active, Some(a), "falls back to the first tab");
    }

    /// An empty snapshot restores nothing — the caller opens a fresh blank tab instead.
    #[test]
    fn empty_snapshot_is_none() {
        assert!(SessionState::from_snapshot(SessionSnapshot::default()).is_none());
    }
}
