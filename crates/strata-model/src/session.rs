//! The **session** vocabulary: a window's open query tabs and their arrangement, as they
//! persist to `.strata/session.json`. Pure serde leaves — the live store (`SessionState` /
//! `QueryTab`, which own the editor buffer) is the frontend's; these are only its durable
//! shape, so `strata-core::project` can read/write them concretely (like `ProjectDefs`).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

/// The serde view of a session: the open tabs in strip order, which is active, the
/// window's geometry, and the panel layout. This *is* the shape of `.strata/session.json`.
#[derive(Serialize, Deserialize, Default)]
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
            inspector_open: true,
            drawer: None,
            sidebar_w: default_sidebar_w(),
            inspector_w: default_inspector_w(),
            drawer_h: default_drawer_h(),
            drawer_restore_h: None,
        }
    }
}

/// One persisted tab — enough to rebuild its live tab: identity (so `active` / order still
/// resolve), title, save target, buffer text and results-view intent. Cursor / scroll /
/// undo are deliberately left out (state-arch §12 — "lean minimal").
#[derive(Serialize, Deserialize)]
pub struct TabSnapshot {
    pub id: TabId,
    pub name: String,
    pub origin: Origin,
    /// The rope contents (`rope.to_string()`), rebuilt into a fresh buffer on load.
    pub text: String,
    #[serde(default)]
    pub view: ResultsView,
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
