//! The **session** vocabulary: a window's open query tabs and their arrangement, as they
//! persist to `.strata/session.json`. Pure serde leaves — the live store (`SessionState` /
//! `QueryTab`, which own the editor buffer) is the frontend's; these are only its durable
//! shape, so `strata-core::project` can read/write them concretely (like `ProjectDefs`).

use serde::{Deserialize, Serialize};
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
/// segmented toggle (P2-07). Per **tab** (CHART_SPEC §1): switching tabs restores the mode,
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
    /// What each connected agent is doing (AA-03b). A *tool pane* rather than a drawer tab
    /// because an agent's work is a live thing you look at while you work, like the catalog,
    /// not a log of what already finished.
    Agents,
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
    #[serde(default)]
    pub sidebar: Option<SidebarPane>,
    /// Whether the right column inspector is open.
    #[serde(default)]
    pub inspector_open: bool,
    /// The open drawer tab, or `None` when collapsed.
    #[serde(default)]
    pub drawer: Option<DrawerTab>,
    #[serde(default = "default_sidebar_w")]
    pub sidebar_w: f32,
    #[serde(default = "default_inspector_w")]
    pub inspector_w: f32,
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
            inspector_open: false,
            drawer: None,
            sidebar_w: default_sidebar_w(),
            inspector_w: default_inspector_w(),
            drawer_h: default_drawer_h(),
            drawer_restore_h: None,
            problems_tab: ProblemsTab::Queries,
        }
    }
}

/// One persisted tab — enough to rebuild its live tab: identity (so `active` / order still
/// resolve), title, save target, buffer text and results-view intent. Cursor / scroll /
/// undo are deliberately left out (state-arch §12 — "lean minimal").
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
