//! The tree's **one row component**, and the handles it resolves before it knows which row it is.
//!
//! Every row in the virtualized list is a [`TreeRow`], whatever it draws. That is not tidiness, it
//! is the contract the fork's differ needs: a `VirtualScrollView` rebuilds its children as a moving
//! window, and Freya pairs a parent's old and new children **by key, in order**, so a list of one
//! component type pairs slot 0 with slot 0. Mixed types in that window pair by type instead, which
//! puts one row's scope under a different row a level up the list, and a scope reused across two
//! different components allocates the wrong number of hooks and hard fails.
//!
//! What that buys is bounded to **the row list**: a scroll modifies each slot rather than
//! reshuffling the window. A row's own children still change shape as the fold plan gives marks up,
//! so the differ's `moved` path is reached from inside a row either way.
//!
//! The price is that **every row runs every hook**, since a hook cannot be conditional and a slot
//! does not know what will scroll into it. It is a bounded price — only the rows on screen exist at
//! all — and [`RowCtx`] is what it buys: each kind's builder is handed what the scope resolved and
//! spends only what it needs.

use freya::prelude::*;
use freya::radio::{use_radio, Radio};

use super::connection::{
    add_connection_row, connection_row, link_row, rel_group_row, relation_note_row, relation_row,
    schema_row,
};
use super::entry::{column_row, entry_row, saved_query_row};
use super::menu::{use_catalog_actions, use_connection_actions, CatalogActions, ConnectionActions};
use super::node::{Node, NodeKind};
use super::row::{mono_advance, use_status, StatusMark};
use super::workspace::{group_row, no_queries_row, workspace_row};
use super::{CatalogTheme, TreeCtx};
use crate::apps::project::state::{use_catalog_selection, CatalogSelection, Chan, SessionState};
use crate::apps::project::views::ConnectionRequest;
use crate::components::type_palette::{type_palette, TypePaletteTheme};

/// What a row's scope resolved before it knew which row it was.
///
/// Handed to the per-kind builders by reference, so adding a handle is one field here and one line
/// in [`TreeRow`] rather than an argument threaded through twelve signatures.
pub struct RowCtx {
    pub tree: TreeCtx,
    /// The workspace rows' menus and the TABLES group's `+`.
    pub catalog: CatalogActions,
    /// A connection row's Edit / Schemas… / Forget.
    pub connections: ConnectionActions,
    /// The inspected column, which a column row both reads and writes.
    pub selection: CatalogSelection,
    /// The window's layout, so selecting a column can reveal the inspector.
    pub layout: Radio<SessionState, Chan>,
    /// Where the empty state's press asks for the connection editor.
    pub editor: ConnectionRequest,
    /// The width of this row's measured run, for the rows that fold (see `row::fold_plan`).
    ///
    /// A fact about the **slot**, which is why it may be shared with whatever scrolls into it. Not
    /// quite a constant one, since the run is what the row's indent leaves and a slot's depth
    /// changes: a slot that hands a depth-0 connection row to a depth-2 entry folds on the old
    /// width for the pass before `on_sized` reports the new one. The fold is a chrome decision on a
    /// row about to be re-laid-out either way, so the frame it costs is the honest price of not
    /// re-deriving the layout here.
    pub measured: State<f32>,
    /// What the status slot is saying, on the two row kinds that have one.
    pub status: Option<StatusMark>,
    /// One character of the mono face, for the two row kinds whose fold plan is arithmetic.
    pub advance: f32,
    /// The column-type hues, for the one row kind that paints one.
    pub palette: TypePaletteTheme,
    pub theme: CatalogTheme,
}

/// One row of the tree.
#[derive(PartialEq)]
pub struct TreeRow {
    pub node: Node,
    pub theme: CatalogTheme,
}

impl Component for TreeRow {
    fn render(&self) -> impl IntoElement {
        let at = self.node.place();
        let (owner, waiting, problem) = match &self.node.kind {
            NodeKind::Entry(entry) => {
                (Some(at.path.as_str()), entry.waiting, entry.problem.clone())
            }
            NodeKind::Connection(conn) => {
                (Some(at.path.as_str()), conn.waiting, conn.problem.clone())
            }
            _ => (None, false, None),
        };

        let cx = RowCtx {
            tree: use_consume::<TreeCtx>(),
            catalog: use_catalog_actions(),
            connections: use_connection_actions(),
            selection: use_catalog_selection(),
            layout: use_radio::<SessionState, Chan>(Chan::Layout),
            editor: use_consume::<ConnectionRequest>(),
            measured: use_state(|| f32::INFINITY),
            status: use_status(owner, waiting, problem),
            advance: mono_advance(),
            palette: type_palette(),
            theme: self.theme.clone(),
        };

        let body = match &self.node.kind {
            NodeKind::Workspace { name } => workspace_row(&at, name, &cx),
            NodeKind::Group { group, count } => group_row(&at, *group, *count, &cx),
            NodeKind::NoQueries => no_queries_row(&at, &cx),
            NodeKind::Entry(entry) => entry_row(&at, entry, &cx),
            NodeKind::Column(column) => column_row(&at, column, &cx),
            NodeKind::SavedQuery { id, name } => saved_query_row(&at, *id, name, &cx),
            NodeKind::Connection(connection) => connection_row(&at, connection, &cx),
            NodeKind::Link { name } => link_row(&at, name, &cx),
            NodeKind::Schema { name, missing } => schema_row(&at, name, *missing, &cx),
            NodeKind::RelGroup { views, count } => rel_group_row(&at, *views, *count, &cx),
            NodeKind::Relation(relation) => relation_row(&at, relation, &cx),
            NodeKind::RelationNote { text, problem } => relation_note_row(&at, text, *problem, &cx),
            NodeKind::AddConnection => add_connection_row(&at, &cx),
        };

        rect().width(Size::fill()).vertical().children(body)
    }
}

/// A row's element, plus whatever it mounts beside it.
///
/// Exactly one kind uses the second slot: an entry whose status column has folded away mounts the
/// profile subscription there instead (`entry::watched_scan`). Anything put here shares the row's
/// slot, whose height the virtual list fixes at `row::ROW_HEIGHT`, so a second *visible* element
/// would overlap the row below and put every reveal on the wrong row.
pub type RowBody = Vec<Element>;

/// The ordinary case: a row is one element.
pub fn body(row: impl IntoElement) -> RowBody {
    vec![row.into_element()]
}
