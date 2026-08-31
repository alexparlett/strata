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
    /// Mints a new identity.
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
    /// Nothing — the tab saves to a target the user has yet to pick.
    Scratch,
    /// A view, by name.
    View(String),
    /// A saved query, by id.
    SavedQuery(Uuid),
}

/// Which body the results pane shows for a settled rows outcome — the toolbar's Table/Chart
/// segmented toggle (P2-07). Per **tab** (`CHART_SPEC` §2): switching tabs restores the mode,
/// and it survives re-runs; the chart *config* will be per result set (Chart workstream).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ResultsView {
    /// The results grid.
    #[default]
    Grid,
    /// The chart.
    Chart,
}

/// The serde view of a session: the open tabs in strip order, which is active, the
/// window's geometry, and the panel layout. This *is* the shape of `.strata/session.json`.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct SessionSnapshot {
    /// The open tabs, in strip order.
    #[serde(default)]
    pub tabs: Vec<TabSnapshot>,
    /// Which of them is active.
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
///
/// **One variant, and it stays an enum.** The data-sources tree (DB-05) absorbed the Connections
/// pane, so the left edge has one pane to offer — but the edge still offers *a* pane, the rail
/// still toggles it, and `None` still means collapsed. Collapsing this to a `bool` would spend
/// [`sidebar_pane`]'s retired-name tolerance, which is what keeps a `session.json` written while
/// Connections was open from being moved aside and costing the user every tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SidebarPane {
    /// The catalog tree.
    Catalog,
}

/// A stored layout value this build may no longer have a variant for — see [`sidebar_pane`] for the
/// rule in full.
///
/// The fallback arm is a `String` and **not** `IgnoredAny`: a retired *name* is the one thing worth
/// tolerating, and taking anything would swallow a malformed value (`42`, `{}`) that ought to fail
/// the load. A retired variant that carried data is therefore still strict.
///
/// [`Retired`](Self::Retired)'s payload is never read, and it must stay a `String` rather than the
/// unit type rustc suggests: the *type* is the check, and `()` deserializes only from `null`, which
/// already means "collapsed".
#[derive(Deserialize)]
#[serde(untagged)]
enum Stored<T> {
    Known(T),
    #[allow(
        dead_code,
        reason = "the field's type is the validation; see the type's docs"
    )]
    Retired(String),
}

/// Read a **non-optional** layout field, resolving a variant this build has retired to `fallback`
/// rather than failing the whole session — the generic half of [`sidebar_pane`]'s rule.
fn retired_to<'de, D, T>(d: D, fallback: T) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(match Stored::<T>::deserialize(d)? {
        Stored::Known(value) => value,
        Stored::Retired(_) => fallback,
    })
}

/// Read an **optional** layout field, resolving a retired variant to `fallback` while keeping
/// `null` meaning what it always has: that surface is collapsed. Two different answers — the file
/// said the surface was *open*, and that half is still the user's arrangement.
fn retired_open<'de, D, T>(d: D, fallback: T) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(match Option::<Stored<T>>::deserialize(d)? {
        Some(Stored::Known(value)) => Some(value),
        Some(Stored::Retired(_)) => Some(fallback),
        None => None,
    })
}

/// [`Layout::right`]'s reader — a retired pane leaves the right side open on the inspector.
fn right_pane<'de, D>(d: D) -> Result<Option<RightPane>, D::Error>
where
    D: Deserializer<'de>,
{
    retired_open(d, RightPane::Inspector)
}

/// [`Layout::drawer`]'s reader — a retired tab leaves the drawer open on Problems.
fn drawer_tab<'de, D>(d: D) -> Result<Option<DrawerTab>, D::Error>
where
    D: Deserializer<'de>,
{
    retired_open(d, DrawerTab::Problems)
}

/// [`Layout::problems_tab`]'s reader — a retired scope falls back to the default one.
fn problems_tab<'de, D>(d: D) -> Result<ProblemsTab, D::Error>
where
    D: Deserializer<'de>,
{
    retired_to(d, ProblemsTab::default())
}

/// [`TabSnapshot::origin`]'s reader — a retired origin makes the tab a scratch tab. It keeps its
/// text, which is the part that cannot be regenerated.
fn tab_origin<'de, D>(d: D) -> Result<Origin, D::Error>
where
    D: Deserializer<'de>,
{
    retired_to(d, Origin::Scratch)
}

/// Read [`Layout::sidebar`], treating a pane this build no longer offers as the **default** pane
/// rather than as a corrupt session.
///
/// `#[serde(default)]` covers a *missing* field and nothing else, so a session written while a
/// since-removed pane was open would fail the whole `SessionSnapshot` — and the loader answers that
/// by moving `session.json` aside, costing every open tab. It resolves to
/// [`SidebarPane::Catalog`] and not to `None`, which stays reserved for an explicit `null`.
fn sidebar_pane<'de, D>(d: D) -> Result<Option<SidebarPane>, D::Error>
where
    D: Deserializer<'de>,
{
    retired_open(d, SidebarPane::Catalog)
}

/// Which assistive surface the **right** rail shows. `None` on [`Layout::right`] means the right
/// side is collapsed.
///
/// A single-selection pane rather than two independent flags, exactly as [`SidebarPane`] is on the
/// left: the right edge has one rail and one column beneath it, so the inspector and the chat are
/// alternatives rather than neighbours. That is what keeps a 1180px window readable.
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
    /// The tabs' SQL diagnostics.
    Problems,
    /// The event log.
    Events,
    /// Query history.
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
    #[serde(default, deserialize_with = "right_pane")]
    pub right: Option<RightPane>,
    /// The open drawer tab, or `None` when collapsed.
    #[serde(default, deserialize_with = "drawer_tab")]
    pub drawer: Option<DrawerTab>,
    /// The sidebar's width.
    #[serde(default = "default_sidebar_w")]
    pub sidebar_w: f32,
    /// The inspector's width.
    #[serde(default = "default_inspector_w")]
    pub inspector_w: f32,
    /// The chat pane's width. Its own field rather than one shared with the inspector: the two
    /// share a slot on screen and nothing else, and a user who sizes one has not sized the other.
    #[serde(default = "default_chat_w")]
    pub chat_w: f32,
    /// The drawer's height.
    #[serde(default = "default_drawer_h")]
    pub drawer_h: f32,
    /// The height the drawer's expand toggle will restore it to — and, by being `Some`, the
    /// fact that it is currently expanded. `None` is the ordinary state, so the toggle needs
    /// no separate flag to keep in step with the height.
    #[serde(default)]
    pub drawer_restore_h: Option<f32>,
    /// Which of the Problems drawer's two scopes is showing. Layout rather than a view-local flag,
    /// so it survives collapsing the drawer, switching to Events and back, and a restart.
    #[serde(default, deserialize_with = "problems_tab")]
    pub problems_tab: ProblemsTab,
}

/// The two scopes of the Problems drawer. A strip *inside* one drawer body rather than a fourth
/// entry on the rail, because these are the same kind of thing at two scopes, where the rail
/// chooses between different surfaces entirely.
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
    /// Which tab this is.
    pub id: TabId,
    /// Its title.
    pub name: String,
    /// What it saves to.
    #[serde(deserialize_with = "tab_origin")]
    pub origin: Origin,
    /// The rope contents (`rope.to_string()`), rebuilt into a fresh buffer on load.
    pub text: String,
    /// Which body the results pane shows.
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
    /// Outer-position left.
    pub y: f32,
    /// Inner (client) size.
    pub width: f32,
    /// Inner (client) width.
    pub height: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A session written while a since-retired pane was open still loads, and keeps its tabs.**
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
        assert_eq!(restored.layout.sidebar, Some(SidebarPane::Catalog));
    }

    /// …and a pane this build does have is read as itself, while an explicit `null` still
    /// means the sidebar is collapsed.
    ///
    /// **Three answers, and they have to stay three.** With one live variant left, a test that
    /// only checked "reads back as Catalog" would pass against a reader that had lost the
    /// distinction entirely — the retired name and the known name would both be answering
    /// `Catalog` for different reasons. So the retired name is asserted here too, beside the
    /// known one and the explicit `null`, which is the only place the three are visible at once.
    #[test]
    fn a_stored_sidebar_pane_reads_back() {
        let known: Layout = serde_json::from_str(r#"{"sidebar":"Catalog"}"#).unwrap();
        assert_eq!(known.sidebar, Some(SidebarPane::Catalog), "read as itself");

        let retired: Layout = serde_json::from_str(r#"{"sidebar":"Connections"}"#).unwrap();
        assert_eq!(
            retired.sidebar,
            Some(SidebarPane::Catalog),
            "a pane this build retired leaves the sidebar open on the default one"
        );

        let collapsed: Layout = serde_json::from_str(r#"{"sidebar":null}"#).unwrap();
        assert_eq!(collapsed.sidebar, None, "null is still collapsed");
    }

    /// A value that is not a pane name at all is **not** tolerated: it is corruption, and the
    /// loader's answer to that is to keep the file aside rather than write over it.
    #[test]
    fn a_malformed_sidebar_value_still_fails_the_load() {
        assert!(serde_json::from_str::<Layout>(r#"{"sidebar":42}"#).is_err());
        assert!(serde_json::from_str::<Layout>(r#"{"sidebar":{}}"#).is_err());
    }

    /// **Every closed vocabulary in a session reads the same way**, not just the sidebar: each
    /// falls back the way the sidebar does, with the surface staying *open* on the default choice.
    #[test]
    fn a_retired_layout_value_keeps_the_session_it_was_written_in() {
        let layout: Layout = serde_json::from_str(
            r#"{"right":"Agents","drawer":"Terminal","problems_tab":"connections"}"#,
        )
        .unwrap();

        assert_eq!(layout.right, Some(RightPane::Inspector));
        assert_eq!(layout.drawer, Some(DrawerTab::Problems));
        assert_eq!(layout.problems_tab, ProblemsTab::Queries);

        let known: Layout =
            serde_json::from_str(r#"{"right":"Chat","drawer":"History","problems_tab":"project"}"#)
                .unwrap();
        assert_eq!(known.right, Some(RightPane::Chat));
        assert_eq!(known.drawer, Some(DrawerTab::History));
        assert_eq!(known.problems_tab, ProblemsTab::Project);

        let collapsed: Layout = serde_json::from_str(r#"{"right":null,"drawer":null}"#).unwrap();
        assert_eq!(collapsed.right, None);
        assert_eq!(collapsed.drawer, None);

        assert!(serde_json::from_str::<Layout>(r#"{"drawer":42}"#).is_err());
    }

    /// A tab whose `origin` this build cannot read becomes a scratch tab **and keeps its text**,
    /// rather than taking the whole session down with it.
    #[test]
    fn a_retired_tab_origin_keeps_the_tab() {
        let tab = format!(
            r#"{{"id":"{}","name":"query 1","origin":"Notebook","text":"SELECT 1"}}"#,
            Uuid::nil()
        );
        let restored: SessionSnapshot =
            serde_json::from_str(&format!(r#"{{"tabs":[{tab}]}}"#)).unwrap();

        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(restored.tabs[0].origin, Origin::Scratch);
        assert_eq!(restored.tabs[0].text, "SELECT 1", "the SQL is what matters");

        let bound = format!(
            r#"{{"id":"{}","name":"v","origin":{{"View":"sales"}},"text":""}}"#,
            Uuid::nil()
        );
        let restored: SessionSnapshot =
            serde_json::from_str(&format!(r#"{{"tabs":[{bound}]}}"#)).unwrap();
        assert_eq!(restored.tabs[0].origin, Origin::View("sales".into()));
    }
}
