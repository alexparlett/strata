//! The **session** vocabulary: a window's open query tabs and their arrangement, as they
//! persist to `.strata/session.json`. Pure serde leaves — the live store (`SessionState` /
//! `QueryTab`, which own the editor buffer) is the frontend's; these are only its durable
//! shape, so `strata-core::project` can read/write them concretely (like `ProjectDefs`).

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::chart::ChartConfig;

/// Stable per-tab identity — real identity, so no allocator and no duplicate-id repair.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TabId(pub Uuid);

impl TabId {
    #[allow(clippy::new_without_default)]
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
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Origin {
    Scratch,
    View(String),
    SavedQuery(Uuid),
}

/// Which body the results pane shows for a settled rows outcome — the toolbar's Table/Chart
/// segmented toggle (P2-07). Per **tab** (`CHART_SPEC` §2): switching tabs restores the mode,
/// and it survives re-runs; the chart *config* will be per result set (Chart workstream).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ResultsView {
    #[default]
    Grid,
    Chart,
}

/// The serde view of a session: the open tabs in strip order, which is active, the
/// window's geometry, and the panel layout. This *is* the shape of `.strata/session.json`.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct SessionSnapshot {
    #[serde(default)]
    pub tabs: Vec<TabSnapshot>,
    #[serde(default)]
    pub active: Option<TabId>,
    /// Where and how big the window was, so it reopens in place. `None` until the first
    /// save (a fresh project).
    #[serde(default)]
    pub window: Option<WindowGeom>,
    /// The window's panel layout (which side panels / drawer are open, and their sizes),
    /// so a reopen restores the same shell arrangement. Defaults (a fresh project or an
    /// older session file) come from [`Layout::default`].
    #[serde(default)]
    pub layout: Layout,
}

/// Which tool pane the left sidebar shows. The rail's top group selects it; `None` on
/// [`Layout::sidebar`] means the sidebar is collapsed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SidebarPane {
    Catalog,
    Connections,
}

/// A stored pane this build may no longer have — see [`sidebar_pane`].
///
/// The fallback arm is a `String` and **not** `IgnoredAny`: a pane name this build has retired
/// is the one thing worth tolerating here, and taking anything at all would swallow a genuinely
/// malformed value (`42`, `{}`, `[1, 2]`) that ought to fail the load and get the file kept
/// aside for recovery.
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredPane {
    Known(SidebarPane),
    // Never read, and it must stay a `String` rather than the unit type rustc suggests: the
    // *type* is the check. `()` deserializes only from `null`, which would take this arm for
    // the one input that already means "collapsed" and let `42` through the other.
    #[allow(dead_code, reason = "the field's type is the validation; see above")]
    Retired(String),
}

/// Read [`Layout::sidebar`], treating a pane this build no longer offers as the **default**
/// pane rather than as a corrupt session.
///
/// `#[serde(default)]` covers a *missing* field and nothing else, so a session written while
/// a since-removed pane was open would fail the whole `SessionSnapshot` — which the loader
/// answers by moving `session.json` aside, costing the user every tab they had open. A pane
/// that no longer exists is not corruption; it is a layout value with nowhere to go.
///
/// It resolves to [`SidebarPane::Catalog`], not to `None`: the stored value said the sidebar
/// was **open**, and that half of it is still true and still the user's arrangement. `None`
/// stays reserved for what it has always meant — an explicit `null`, the sidebar collapsed —
/// which the arm below keeps distinct.
fn sidebar_pane<'de, D>(d: D) -> Result<Option<SidebarPane>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Option::<StoredPane>::deserialize(d)? {
        Some(StoredPane::Known(pane)) => Some(pane),
        Some(StoredPane::Retired(_)) => Some(SidebarPane::Catalog),
        None => None,
    })
}

/// Which assistive surface the **right** rail shows. `None` on [`Layout::right`] means the
/// right side is collapsed.
///
/// A single-selection pane rather than two independent flags, exactly as
/// [`SidebarPane`] is on the left: the canvas (`Strata.dc.html` `data-rg="rightrail"`) gives
/// the right edge its own 48px rail and one column beneath it, so the inspector and the chat
/// are alternatives rather than neighbours. That is what keeps a 1180px window readable with
/// both rails and the drawer up, and it is the same arrangement RustRover uses on its right
/// edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum RightPane {
    /// The selected column's facts (P3-08).
    Inspector,
    /// The assistant's conversation (AS-04).
    Chat,
}

/// Which tab the bottom drawer shows. The rail's bottom group selects it; `None` on
/// [`Layout::drawer`] means the drawer is collapsed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum DrawerTab {
    Problems,
    Events,
    History,
}

/// The window's panel-layout arrangement — which side panels / drawer are open (and on
/// which pane/tab), plus each resizable panel's last size. Sizes are **logical** px (like
/// [`WindowGeom`]). `ResizableContainer` owns live resizing; these persist the last size so
/// a collapse→reopen or a restart restores it. Defaults match the design's initial state.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Layout {
    /// The open sidebar pane, or `None` when collapsed.
    #[serde(default, deserialize_with = "sidebar_pane")]
    pub sidebar: Option<SidebarPane>,
    /// The open right pane, or `None` when the right side is collapsed.
    #[serde(default)]
    pub right: Option<RightPane>,
    /// The open drawer tab, or `None` when collapsed.
    #[serde(default)]
    pub drawer: Option<DrawerTab>,
    #[serde(default = "default_sidebar_w")]
    pub sidebar_w: f32,
    #[serde(default = "default_inspector_w")]
    pub inspector_w: f32,
    /// The chat pane's width. Its own field rather than one shared with the inspector: the two
    /// share a slot on screen and nothing else — a transcript wants more room than a column's
    /// facts do, and a user who sizes one has not sized the other.
    #[serde(default = "default_chat_w")]
    pub chat_w: f32,
    #[serde(default = "default_drawer_h")]
    pub drawer_h: f32,
    /// The height the drawer's expand toggle will restore it to — and, by being `Some`, the
    /// fact that it is currently expanded. `None` is the ordinary state, so the toggle needs
    /// no separate flag to keep in step with the height.
    #[serde(default)]
    pub drawer_restore_h: Option<f32>,
    /// Which of the Problems drawer's two scopes is showing. Layout, not a view-local flag, for
    /// the reason every other field here is: it is part of the arrangement the user set up, so
    /// it survives collapsing the drawer, switching to Events and back, and a restart.
    #[serde(default)]
    pub problems_tab: ProblemsTab,
}

/// The two scopes of the Problems drawer (P4-15 item 3).
///
/// A strip *inside* one drawer body rather than a fourth entry on the rail, because these are the
/// same kind of thing — problems — at two scopes, where the rail chooses between different
/// surfaces entirely. (JetBrains' Problems panel splits on exactly this axis.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemsTab {
    /// Every open tab's SQL diagnostics — the drawer as it was, and still the default.
    #[default]
    Queries,
    /// Conditions about the **project** rather than about a query's text: defs the engine
    /// refused, and `.strata` files that are behind the screen because a write failed.
    Project,
}

fn default_sidebar_w() -> f32 {
    288.0
}
fn default_inspector_w() -> f32 {
    292.0
}
fn default_chat_w() -> f32 {
    340.0
}
fn default_drawer_h() -> f32 {
    240.0
}
/// The height the drawer's expand toggle raises it to (design `onToggleLogHeight`).
pub fn expanded_drawer_h() -> f32 {
    560.0
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar: Some(SidebarPane::Catalog),
            right: None,
            drawer: None,
            sidebar_w: default_sidebar_w(),
            inspector_w: default_inspector_w(),
            chat_w: default_chat_w(),
            drawer_h: default_drawer_h(),
            drawer_restore_h: None,
            problems_tab: ProblemsTab::Queries,
        }
    }
}

/// One persisted tab — enough to rebuild its live tab: identity (so `active` / order still
/// resolve), title, save target, buffer text and results-view intent. Cursor / scroll /
/// undo are deliberately left out (state-arch §5 — "lean minimal").
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct TabSnapshot {
    pub id: TabId,
    pub name: String,
    pub origin: Origin,
    /// The rope contents (`rope.to_string()`), rebuilt into a fresh buffer on load.
    pub text: String,
    #[serde(default)]
    pub view: ResultsView,
    /// How the tab's chart is encoded (`docs/CHART_SPEC.md` §6). Column *references*, so a
    /// restored tab whose next result has different columns falls back to the defaults
    /// rather than asking for a column that isn't there.
    #[serde(default)]
    pub chart: ChartConfig,
}

/// The window's on-screen geometry, in **logical** units (like `Platform::root_size` /
/// `Platform::window_position`) so it never scale-factor-corrects.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
pub struct WindowGeom {
    /// Outer-position top-left.
    pub x: f32,
    pub y: f32,
    /// Inner (client) size.
    pub width: f32,
    pub height: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A session written while a since-retired pane was open still loads, and keeps its
    /// tabs.** That is the whole point of the lenient read: the alternative is a parse
    /// failure, which the loader answers by moving `session.json` aside — so removing a pane
    /// would cost anyone who had it open everything they had open with it.
    ///
    /// Through `SessionSnapshot` rather than `Layout`, because the tabs are the claim.
    #[test]
    fn a_retired_sidebar_pane_keeps_the_session_it_was_written_in() {
        let tab = format!(
            r#"{{"id":"{}","name":"query 1","origin":"Scratch","text":"SELECT 1"}}"#,
            Uuid::nil()
        );
        let restored: SessionSnapshot = serde_json::from_str(&format!(
            r#"{{"tabs":[{tab}],"layout":{{"sidebar":"Agents"}}}}"#
        ))
        .unwrap();

        assert_eq!(restored.tabs.len(), 1, "the tabs survive");
        // Open, on the default pane — the stored value said the sidebar was up.
        assert_eq!(restored.layout.sidebar, Some(SidebarPane::Catalog));
    }

    /// …and a pane this build does have is read as itself, while an explicit `null` still
    /// means the sidebar is collapsed.
    #[test]
    fn a_stored_sidebar_pane_reads_back() {
        let open: Layout = serde_json::from_str(r#"{"sidebar":"Connections"}"#).unwrap();
        assert_eq!(open.sidebar, Some(SidebarPane::Connections));

        let collapsed: Layout = serde_json::from_str(r#"{"sidebar":null}"#).unwrap();
        assert_eq!(collapsed.sidebar, None);
    }

    /// A value that is not a pane name at all is **not** tolerated: it is corruption, and the
    /// loader's answer to that is to keep the file aside rather than write over it.
    #[test]
    fn a_malformed_sidebar_value_still_fails_the_load() {
        assert!(serde_json::from_str::<Layout>(r#"{"sidebar":42}"#).is_err());
        assert!(serde_json::from_str::<Layout>(r#"{"sidebar":{}}"#).is_err());
    }
}
