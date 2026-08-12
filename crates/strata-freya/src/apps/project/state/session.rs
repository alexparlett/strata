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
    expanded_drawer_h, ChartConfig, Diagnostic, DrawerTab, Layout, Origin, ProblemsTab,
    ResultsView, RightPane, SessionSnapshot, SidebarPane, TabId, TabSnapshot,
};
use uuid::Uuid;

use crate::apps::project::query::QuerySpec;

/// What a tab's diagnostics **describe**: the buffer revision they were computed from, and the
/// catalog epoch they were resolved against. Those are validation's only two inputs, so a stamp
/// that no longer matches the tab is the whole definition of "needs another pass".
///
/// This is what lets the driver stop enumerating entry points. A tab restored at project open,
/// reopened with ⇧⌘T, opened from a saved query or a view, duplicated, or left behind by a pass
/// a tab switch cancelled are not five cases — they are one: the stamp does not match.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    pub revision: u64,
    pub epoch: u64,
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
    /// How this tab's chart is encoded (Rz2, `docs/CHART_SPEC.md` §6) — the mark, the column
    /// assignments and the sort, as *intent*: unset channels take the result schema's
    /// defaults, and a reference the current result cannot resolve falls back to one without
    /// being erased. Written on [`Chan::Chart`](super::Chan), so an encoder edit re-charts
    /// without waking the editor, the grid or the toolbar.
    pub chart: ChartConfig,
    /// The tab's current validation diagnostics: the debounced engine dry-plan pass over the
    /// editor text. Written on [`Chan::Diagnostics`](super::Chan) by the window's one driver
    /// (`state::diagnostics`); the editor's squiggles are the same facts, carried as
    /// decorations *inside* the buffer by the same pass. Purely advisory — Run is never gated
    /// on them (P2-23: diagnostics advise, the engine decides; a doomed run fails at plan time
    /// with the same error).
    pub diagnostics: Vec<Diagnostic>,
    /// What [`diagnostics`](Self::diagnostics) describes, or `None` when nothing has looked at
    /// this tab yet.
    ///
    /// The distinction is load-bearing, not bookkeeping: `Some(_)` with an empty vec is the
    /// only honest way to say **clean**, and `None` means **unchecked**. Reading an empty vec
    /// as clean is exactly why the Problems drawer could once speak for the active tab only.
    pub validated: Option<Stamp>,
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
            chart: ChartConfig::default(),
            diagnostics: Vec::new(),
            validated: None,
        }
    }

    /// A blank scratch buffer.
    pub fn scratch(name: String) -> Self {
        Self::new(name, String::new(), Origin::Scratch)
    }

    /// Rebuild a tab from a persisted snapshot (P4-14 load). Like [`new`](Self::new) but
    /// **keeps the original [`TabId`]** — so the snapshot's `active` / order still resolve
    /// against it — and restores its results-view and chart intent. Marked saved at the
    /// restored text (a reopened project starts clean, like a freshly loaded artifact); the
    /// validation pass re-derives diagnostics once the pane mounts.
    ///
    /// Takes the whole [`TabSnapshot`]: everything it carries is a tab field, and a
    /// positional list that grows with each persisted facet is how one ends up passed in the
    /// wrong order.
    pub fn restored(snap: TabSnapshot) -> Self {
        let mut editor = CodeEditorData::new(Rope::from_str(&snap.text), Some(sql_language()));
        editor.parse();
        editor.mark_as_saved();
        Self {
            id: snap.id,
            name: snap.name,
            editor,
            origin: snap.origin,
            request: None,
            view: snap.view,
            chart: snap.chart,
            diagnostics: Vec::new(),
            validated: None,
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

/// One tab's block in the Problems view: which tab the rows belong to, what it is called now,
/// and the rows themselves. The **group** is where a row's owning tab comes from — a
/// [`Diagnostic`] deliberately doesn't carry one, so there is no second copy to disagree with
/// the tab it is stored on.
#[derive(Clone, PartialEq, Debug)]
pub struct ProblemGroup {
    pub tab: TabId,
    pub name: String,
    pub rows: Vec<Diagnostic>,
}

/// Cap on the reopen stack; parking more drops the oldest (freeing its buffer).
const CLOSED_CAP: usize = 20;

/// Strip a tab of everything that describes a world it is no longer part of, on the way to the
/// reopen stack. Its `request` goes for the reason `close_one` has always given (a reopened tab
/// starts with no results, matching the engine-side cleanup); its diagnostics and stamp go with
/// it, because `reopen_last` restores the parked tab **whole** and would otherwise bring back a
/// verdict on text from before the close, against a catalog from before whatever happened in
/// between. Cleared, it comes back **unvalidated**, and the driver simply picks it up.
fn park(tab: &mut QueryTab) {
    tab.request = None;
    tab.diagnostics.clear();
    tab.validated = None;
}

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

    /// The tab's display name — what the tab strip shows, and what names a run everywhere
    /// outside this window (the Export window's subtitle and its suggested filename).
    pub fn name(&self, id: TabId) -> String {
        self.tabs
            .get(&id)
            .map(|t| t.name.clone())
            .unwrap_or_default()
    }

    /// A settled validation pass: replace `id`'s diagnostics **and** stamp them, so the two can
    /// never disagree about which text and which catalog they describe. Write on
    /// [`Chan::Diagnostics`](super::Chan). A no-op for a tab closed while the pass ran.
    pub fn set_diagnostics(&mut self, id: TabId, stamp: Stamp, diagnostics: Vec<Diagnostic>) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.diagnostics = diagnostics;
            t.validated = Some(stamp);
        }
    }

    /// Every open tab whose diagnostics no longer describe what it holds — the validation
    /// driver's whole work list, in the order it should run them: the **active tab first** (it
    /// is the one being looked at), then strip order.
    ///
    /// This is the reason the driver needs no list of entry points. A restored tab has no stamp;
    /// a reopened one carries a stamp from before its close; a duplicated one copied text but no
    /// stamp; an edited one moved its revision; and every tab goes stale at once when the
    /// catalog epoch moves. All of them are just "the stamp does not match".
    pub fn stale_tabs(&self, epoch: u64) -> Vec<TabId> {
        let stale = |id: &TabId| {
            self.tabs.get(id).is_some_and(|t| {
                t.validated
                    != Some(Stamp {
                        revision: t.editor.revision(),
                        epoch,
                    })
            })
        };
        self.active
            .filter(stale)
            .into_iter()
            .chain(
                self.order
                    .iter()
                    .copied()
                    .filter(|id| Some(*id) != self.active && stale(id)),
            )
            .collect()
    }

    /// The Problems view's groups: every **validated** tab that has something to report, in
    /// strip order.
    ///
    /// Unvalidated tabs are skipped rather than shown clean — an empty vec means "clean" only
    /// once something has looked, which is what the stamp exists to distinguish.
    pub fn problem_groups(&self) -> Vec<ProblemGroup> {
        self.order
            .iter()
            .filter_map(|id| self.tabs.get(id))
            .filter(|t| t.validated.is_some() && !t.diagnostics.is_empty())
            .map(|t| ProblemGroup {
                tab: t.id,
                name: t.name.clone(),
                rows: t.diagnostics.clone(),
            })
            .collect()
    }

    /// How many **errors** are open across every validated tab — the drawer header's tally and
    /// the rail badge, from one function so the two can't disagree. Warnings and infos still
    /// list in the drawer; they just don't claim the query is broken.
    pub fn error_count(&self) -> usize {
        self.tabs
            .values()
            .filter(|t| t.validated.is_some())
            .flat_map(|t| t.diagnostics.iter())
            .filter(|d| d.is_error())
            .count()
    }

    /// Flip `id`'s results view (the toolbar's Table/Chart toggle). Write on
    /// [`Chan::View(id)`](super::Chan).
    pub fn set_view(&mut self, id: TabId, view: ResultsView) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.view = view;
        }
    }

    /// The tab's chart encoding (a missing tab reads the defaults — every channel unset,
    /// which is what a chart with nothing chosen is).
    pub fn chart(&self, id: TabId) -> ChartConfig {
        self.tabs
            .get(&id)
            .map(|t| t.chart.clone())
            .unwrap_or_default()
    }

    /// Set `id`'s chart encoding (any control in the encoder strip). Write on
    /// [`Chan::Chart(id)`](super::Chan).
    pub fn set_chart(&mut self, id: TabId, chart: ChartConfig) {
        if let Some(t) = self.tabs.get_mut(&id) {
            t.chart = chart;
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

    /// Show the drawer on `tab` — the command palette's **Query history**, and any other
    /// surface that names a drawer tab rather than offering the rail's toggle.
    ///
    /// Not [`toggle_drawer`](Self::toggle_drawer): a rail button says "this pane", so pressing
    /// the lit one to put it away is the whole gesture, but a palette row says "Query history"
    /// and has to mean it — asking for the drawer you are already looking at must not collapse
    /// it. The same distinction [`show_problems_tab`](Self::show_problems_tab) already draws.
    pub fn open_drawer(&mut self, tab: DrawerTab) {
        self.layout.drawer = Some(tab);
    }

    /// Collapse the drawer (its header ×).
    pub fn close_drawer(&mut self) {
        self.layout.drawer = None;
    }

    /// The Problems header's scope strip: show `tab`. Not a toggle like the rail's — a strip
    /// always has one selected, and pressing the selected one is a no-op rather than a collapse.
    pub fn show_problems_tab(&mut self, tab: ProblemsTab) {
        self.layout.problems_tab = tab;
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

    /// The right rail's toggle (AS-04): open the right side on `pane`, or — if it's already
    /// showing `pane` — collapse it. [`toggle_pane`](Self::toggle_pane)'s rule on the other
    /// edge, because it is the same gesture on the same kind of control.
    pub fn toggle_right_pane(&mut self, pane: RightPane) {
        self.layout.right = (self.layout.right != Some(pane)).then_some(pane);
    }

    /// Show `pane` on the right — for the surfaces that *name* one rather than offering the
    /// rail's toggle: selecting a catalog column reveals the inspector, and the three friction
    /// entries open the chat. [`open_drawer`](Self::open_drawer)'s distinction, for its reason:
    /// asking for the pane you are already looking at must not put it away.
    pub fn open_right_pane(&mut self, pane: RightPane) {
        self.layout.right = Some(pane);
    }

    /// Collapse the right side (a pane header's ×).
    pub fn close_right_pane(&mut self) {
        self.layout.right = None;
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

    /// Remember the chat pane's dragged width. Write on [`Chan::LayoutSize`](super::Chan).
    pub fn set_chat_w(&mut self, w: f32) {
        self.layout.chat_w = w;
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
            // Parked without its request or its verdict — reopen starts with no results and
            // unvalidated, like a fresh tab (see `park`; SNAPSHOT_SPEC §4 for the engine half).
            park(&mut tab);
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
                park(&mut tab);
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
                park(&mut tab);
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
                park(&mut tab);
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
                chart: t.chart.clone(),
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
            let tab = QueryTab::restored(t);
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
    use strata_model::{ChartMark, ChartSort, ChartX};

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

    /// Snapshot → restore preserves order, active, text, origin, view and chart encoding;
    /// the reopen stack and diagnostics do not travel.
    #[test]
    fn snapshot_round_trips_tabs_order_active_and_view() {
        let mut s = SessionState::default();
        let a = s.open_named("alpha", "SELECT 1".into(), Origin::View("alpha".into()));
        let b = s.open_named("beta", "SELECT 2".into(), Origin::Scratch);
        s.set_view(b, ResultsView::Chart);
        s.set_chart(
            b,
            ChartConfig {
                mark: Some(ChartMark::Area),
                x: ChartX::Column("month".into()),
                ys: Some(vec!["revenue".into()]),
                series: Some("region".into()),
                sort: ChartSort::ByYDesc,
                bins: Some(24),
                hidden: vec!["cost".into()],
                log_y: true,
                trend: true,
                y_lo: Some("floor".into()),
                y_hi: Some("ceil".into()),
                q1: Some("p25".into()),
                q3: Some("p75".into()),
            },
        );
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
        assert_eq!(
            ra.chart,
            ChartConfig::default(),
            "an untouched chart restores unset, not resolved"
        );
        assert!(!ra.is_dirty(), "a restored bound tab starts clean");

        let rb = &restored.tabs[&b];
        assert_eq!(rb.text(), "SELECT 2");
        assert_eq!(rb.view, ResultsView::Chart, "per-tab view intent preserved");
        assert_eq!(
            rb.chart.mark,
            Some(ChartMark::Area),
            "and so is the chart encoding, whole"
        );
        assert_eq!(rb.chart.x, ChartX::Column("month".into()));
        assert_eq!(
            rb.chart.ys.as_deref(),
            Some(["revenue".to_string()].as_ref())
        );
        assert_eq!(rb.chart.series.as_deref(), Some("region"));
        assert_eq!(rb.chart.sort, ChartSort::ByYDesc);
        // The bin count and the two display preferences travel with the rest of it — a tab
        // reopened on a histogram must come back binned the way it was left.
        assert_eq!(rb.chart.bins, Some(24));
        assert_eq!(rb.chart.hidden, ["cost"]);
        assert!(rb.chart.log_y);
    }

    /// The rail toggles follow the design's `onRailPane` / `onOpen*` semantics: toggling the
    /// active pane/tab collapses it, toggling another switches to it, and the × closers
    /// collapse.
    #[test]
    fn layout_toggles_follow_design_semantics() {
        let mut s = SessionState::default();
        // Defaults: sidebar open on Catalog, the right side and the drawer collapsed.
        assert_eq!(s.layout.sidebar, Some(SidebarPane::Catalog));
        assert_eq!(s.layout.right, None);
        assert_eq!(s.layout.drawer, None);

        // The right rail is the sidebar's rule on the other edge: naming a pane opens it,
        // toggling the open one collapses, toggling the other switches.
        s.open_right_pane(RightPane::Inspector);
        assert_eq!(s.layout.right, Some(RightPane::Inspector));
        s.open_right_pane(RightPane::Inspector);
        assert_eq!(
            s.layout.right,
            Some(RightPane::Inspector),
            "naming is not a toggle"
        );
        s.toggle_right_pane(RightPane::Chat);
        assert_eq!(s.layout.right, Some(RightPane::Chat));
        s.toggle_right_pane(RightPane::Chat);
        assert_eq!(s.layout.right, None);

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

        s.open_right_pane(RightPane::Inspector);
        s.close_right_pane();
        assert_eq!(s.layout.right, None);
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

        assert_eq!(
            s.toggle_drawer_height(),
            310.0,
            "the dragged height is back"
        );
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
        s.open_right_pane(RightPane::Chat);
        s.toggle_drawer(DrawerTab::Events);
        s.set_sidebar_w(333.0);
        s.set_drawer_h(199.0);

        let restored =
            SessionState::from_snapshot(s.snapshot()).expect("non-empty snapshot restores");
        assert_eq!(restored.layout.sidebar, Some(SidebarPane::Connections));
        assert_eq!(restored.layout.right, Some(RightPane::Chat));
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

    // --- diagnostics: the stamp, and the projections over it --------------

    use strata_model::Severity;

    fn problem(message: &str) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            loc: Some("line 1:1".into()),
            span: Some(0..6),
        }
    }

    fn warning(message: &str) -> Diagnostic {
        Diagnostic {
            severity: Severity::Warning,
            ..problem(message)
        }
    }

    /// Stamp a tab as validated against the world it currently holds.
    fn mark(s: &mut SessionState, id: TabId, epoch: u64, rows: Vec<Diagnostic>) {
        let revision = s.tabs[&id].editor.revision();
        s.set_diagnostics(id, Stamp { revision, epoch }, rows);
    }

    /// The driver's whole work list. Every way a tab can come to need a pass reduces to one
    /// question — does its stamp match? — which is what replaced enumerating entry points.
    #[test]
    fn stale_tabs_is_every_tab_whose_stamp_no_longer_matches() {
        let mut s = SessionState::default();
        let a = s.open_named("a", "SELECT 1".into(), Origin::Scratch);
        let b = s.open_named("b", "SELECT 2".into(), Origin::Scratch);

        // Nothing has looked at either yet.
        assert_eq!(s.stale_tabs(1).len(), 2, "unvalidated tabs are stale");

        mark(&mut s, a, 1, vec![]);
        mark(&mut s, b, 1, vec![]);
        assert!(
            s.stale_tabs(1).is_empty(),
            "both describe the world as it is"
        );

        // The user types in one of them.
        s.tabs.get_mut(&a).unwrap().editor.set_text("SELECT 3");
        assert_eq!(s.stale_tabs(1), vec![a], "a moved its text, b did not");

        // The catalog is fixed: every tab's verdict was resolved against the old one.
        mark(&mut s, a, 1, vec![]);
        let stale = s.stale_tabs(2);
        assert_eq!(stale.len(), 2, "a catalog epoch bump stales everything");
    }

    /// The active tab runs first — it is the one being looked at — and the rest follow strip
    /// order, so a 20-tab project open doesn't make you wait for tab 19 to squiggle tab 1.
    #[test]
    fn stale_tabs_puts_the_active_tab_first() {
        let mut s = SessionState::default();
        let a = s.open_named("a", "SELECT 1".into(), Origin::Scratch);
        let b = s.open_named("b", "SELECT 2".into(), Origin::Scratch);
        let c = s.open_named("c", "SELECT 3".into(), Origin::Scratch);
        s.switch(b);

        assert_eq!(s.stale_tabs(1), vec![b, a, c]);
    }

    /// A settled pass stamps and replaces together, so a tab can never carry rows that describe
    /// one world and a stamp that claims another.
    #[test]
    fn set_diagnostics_stamps_and_replaces_together() {
        let mut s = SessionState::default();
        let a = s.open_named("a", "SELECT 1".into(), Origin::Scratch);

        mark(
            &mut s,
            a,
            3,
            vec![problem("Table or view 'nope' not found")],
        );
        let t = &s.tabs[&a];
        assert_eq!(t.diagnostics.len(), 1);
        assert_eq!(t.validated.unwrap().epoch, 3);

        // A later clean pass retracts wholesale — nothing accumulates.
        mark(&mut s, a, 3, vec![]);
        assert!(s.tabs[&a].diagnostics.is_empty());
        assert!(s.tabs[&a].validated.is_some(), "clean, not unchecked");

        // A tab closed mid-pass simply doesn't take the answer.
        let gone = TabId::new();
        s.set_diagnostics(
            gone,
            Stamp {
                revision: 0,
                epoch: 3,
            },
            vec![problem("x")],
        );
        assert!(!s.tabs.contains_key(&gone));
    }

    /// The drawer's groups: strip order, clean tabs skipped, and — the distinction the stamp
    /// exists for — **unvalidated** tabs skipped too. An unchecked tab is not a clean one.
    #[test]
    fn problem_groups_skip_clean_and_unchecked_tabs_alike() {
        let mut s = SessionState::default();
        let a = s.open_named("a", "SELECT 1".into(), Origin::Scratch);
        let b = s.open_named("b", "SELECT 2".into(), Origin::Scratch);
        let c = s.open_named("c", "SELECT 3".into(), Origin::Scratch);

        mark(&mut s, a, 1, vec![problem("bad a")]);
        mark(&mut s, b, 1, vec![]); // clean
                                    // c is left unvalidated

        let groups = s.problem_groups();
        assert_eq!(groups.len(), 1, "only the tab with something to report");
        assert_eq!(groups[0].tab, a);
        assert_eq!(groups[0].rows.len(), 1);

        // Give c a verdict and it joins, in strip order behind a.
        mark(&mut s, c, 1, vec![problem("bad c")]);
        assert_eq!(
            s.problem_groups().iter().map(|g| g.tab).collect::<Vec<_>>(),
            vec![a, c]
        );
    }

    /// A group is labelled with what the tab is called *now*, so renaming a tab relabels its
    /// problems without re-validating anything.
    #[test]
    fn a_group_carries_the_tabs_current_name() {
        let mut s = SessionState::default();
        let a = s.open_named("a", "SELECT 1".into(), Origin::Scratch);
        mark(&mut s, a, 1, vec![problem("bad")]);

        s.rename(a, "orders_daily".into());
        assert_eq!(s.problem_groups()[0].name, "orders_daily");
    }

    /// The header tally and the rail badge count **errors**, not rows: a keyword-typo warning
    /// listing in the drawer must not claim the query is broken.
    #[test]
    fn error_count_counts_errors_across_validated_tabs_only() {
        let mut s = SessionState::default();
        let a = s.open_named("a", "SELECT 1".into(), Origin::Scratch);
        let b = s.open_named("b", "SELECT 2".into(), Origin::Scratch);
        let c = s.open_named("c", "SELECT 3".into(), Origin::Scratch);

        mark(&mut s, a, 1, vec![problem("e1"), warning("w1")]);
        mark(&mut s, b, 1, vec![problem("e2")]);
        s.tabs.get_mut(&c).unwrap().diagnostics = vec![problem("never validated")];

        assert_eq!(s.error_count(), 2, "two errors; the warning doesn't count");
    }

    /// Closing parks the tab **unvalidated**, so ⇧⌘T can't bring back a verdict on text from
    /// before the close (against a catalog from before whatever happened in between). The
    /// driver picks the reopened tab up like any other unstamped one.
    #[test]
    fn closing_drops_the_verdict_so_reopen_starts_unchecked() {
        let mut s = SessionState::default();
        let a = s.open_named("a", "SELECT 1".into(), Origin::Scratch);
        mark(&mut s, a, 1, vec![problem("bad")]);

        s.close_one(a);
        s.reopen_last();

        let t = &s.tabs[&a];
        assert!(t.diagnostics.is_empty(), "the stale verdict did not travel");
        assert!(
            t.validated.is_none(),
            "and it reads as unchecked, not clean"
        );
        assert_eq!(s.stale_tabs(1), vec![a], "so the driver re-validates it");
    }
}
