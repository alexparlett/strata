//! The tree's **flat list of visible rows** — what the pane hands the fork's virtualized
//! [`Tree`](freya::components::Tree), and the walk that builds it.
//!
//! A [`Node`] is one row's data, resolved: its depth, its node path and disclosure if it opens, and
//! whatever the row draws. Nothing here renders and nothing here subscribes, so the tree's contents
//! are a plain function of the store, the engine's listings, the filter and the expansion set.
//!
//! **The walk is where the tree's shape lives**, and it is the only place that decides it: one
//! pass, one clone per visible row, and the rows below are then only what each one draws. A
//! container that re-derived its own children as it rendered would re-render the whole pane on any
//! chevron press and clone a schema's relations once per level on the way down.

use std::collections::HashSet;

use freya::components::Disclosure;
use strata_core::engine::Engine;
use strata_model::{CatalogKind, ConnectionDef};
use uuid::Uuid;

use super::columns::ColRow;
use super::connection::walk_connections;
use super::workspace::{walk_workspace, Group};
use crate::apps::project::query::ScanId;
use crate::apps::project::state::ProjectState;

/// One visible row of the tree.
#[derive(Clone, PartialEq)]
pub struct Node {
    /// Levels of indentation, which is also this row's place in the hierarchy.
    pub depth: usize,
    /// Set on a row that opens, or that a jump can address. `None` on a leaf nothing addresses —
    /// a relation, an object store's link, an empty group's note.
    pub branch: Option<Branch>,
    pub kind: NodeKind,
}

/// A row that opens: the node path it is addressed by, whether it is **stored** open, and whether
/// it has anything to open.
///
/// The two flags are kept apart because they answer different questions and only one of them is a
/// chevron. A row that is open and has nothing to show draws a `Leaf` but is still *open*, and a
/// press on it has to close it: deriving the press's answer from the chevron makes an unregistered
/// table, a reconnecting database and an emptied object store all impossible to collapse.
#[derive(Clone, PartialEq)]
pub struct Branch {
    pub path: String,
    pub open: bool,
    pub can_open: bool,
}

impl Node {
    /// A row nothing opens and nothing addresses.
    pub fn leaf(depth: usize, kind: NodeKind) -> Self {
        Self {
            depth,
            branch: None,
            kind,
        }
    }

    /// A row addressed by `path`, showing its children if `open` and able to if `can_open`.
    pub fn branch(depth: usize, path: String, open: bool, can_open: bool, kind: NodeKind) -> Self {
        Self {
            depth,
            branch: Some(Branch {
                path,
                open,
                can_open,
            }),
            kind,
        }
    }

    /// This row's node path, if it has one — what a jump looks itself up by.
    pub fn path(&self) -> Option<&str> {
        self.branch.as_ref().map(|b| b.path.as_str())
    }

    /// Where this row sits and how it opens.
    ///
    /// A leaf has no address to toggle and nothing to open, which is what the fallback says: the
    /// empty path reaches no row kind that toggles.
    pub fn place(&self) -> Place {
        match &self.branch {
            Some(Branch {
                path,
                open,
                can_open,
            }) => Place {
                depth: self.depth,
                path: path.clone(),
                open: *open,
                can_open: *can_open,
            },
            None => Place {
                depth: self.depth,
                path: String::new(),
                open: false,
                can_open: false,
            },
        }
    }
}

/// Where a row sits and how it opens — what every row kind is built from, so no row restates it.
#[derive(Clone, PartialEq)]
pub struct Place {
    pub depth: usize,
    pub path: String,
    /// Whether this row is **stored** open. What a press has to negate, and deliberately not a
    /// function of [`disclosure`](Self::disclosure) — see [`Branch`].
    pub open: bool,
    pub can_open: bool,
}

impl Place {
    /// The chevron this row draws: none when there is nothing to open, and otherwise which way it
    /// points.
    pub fn disclosure(&self) -> Disclosure {
        match self.can_open {
            true => Disclosure::from_expanded(self.open),
            false => Disclosure::Leaf,
        }
    }
}

/// What a row is.
#[derive(Clone, PartialEq)]
pub enum NodeKind {
    /// The project's own database, labelled with the project's name.
    Workspace {
        name: String,
    },
    /// One of the workspace's three groups, with the count the filter left it.
    Group {
        group: Group,
        count: usize,
    },
    /// A workspace table or view.
    Entry(Entry),
    /// One column of an entry, or an expanded nested field of one.
    Column(Column),
    SavedQuery {
        id: Uuid,
        name: String,
    },
    /// What the QUERIES group says when it has nothing in it.
    NoQueries,
    /// A connection of either kind.
    Connection(Connection),
    /// A workspace table read through an object-store connection, as a jump to its own row.
    Link {
        name: String,
    },
    /// One schema of a database connection.
    Schema {
        name: String,
        missing: bool,
    },
    /// The Tables / Views split inside a schema.
    RelGroup {
        views: bool,
        count: usize,
    },
    /// One relation inside a schema. A leaf: its columns are DB-07's.
    Relation(Remote),
    /// The pane's empty state, on a project with no connections at all.
    AddConnection,
}

/// A workspace table or view, resolved.
///
/// `waiting` and `problem` are the row's registration state read **in the walk**, not by the row: a
/// row is where the hold-back lives (a hook), never where the verdict is looked up.
#[derive(Clone, PartialEq)]
pub struct Entry {
    pub kind: CatalogKind,
    pub name: String,
    /// A table whose data Strata owns (ED-04) — the row's icon tint and its `INTERNAL` badge.
    pub internal: bool,
    pub waiting: bool,
    pub problem: Option<String>,
    /// The profile scan asked for on this row, if any (P3-09).
    pub scan: Option<ScanId>,
}

/// One column row, and the entry it belongs to.
#[derive(Clone, PartialEq)]
pub struct Column {
    pub owner_kind: CatalogKind,
    pub owner: String,
    pub row: ColRow,
}

/// One relation inside a database connection's catalog, resolved (DB-06).
///
/// Both three-part forms are built **in the walk**, because that is where the catalog and the
/// schema are still in hand — and because a gesture composing them from three fields is three
/// chances to quote one of them wrong. They are two forms and not one: an
/// [`address`](Self::address) is written into a statement and so carries whatever quoting
/// `qualified` had to add, while a [`label`](Self::label) is read by a person and must not. What
/// the row itself draws is still [`name`](Self::name): the tree says where a relation is by where
/// it sits.
#[derive(Clone, PartialEq)]
pub struct Remote {
    pub name: String,
    /// `catalog.schema.relation` in the plain segments — a tab title, never SQL.
    pub label: String,
    /// `catalog.schema.relation` rendered by
    /// [`sql::qualified`](strata_core::engine::sql::qualified), ready to interpolate into a
    /// statement.
    pub address: String,
    pub view: bool,
}

/// A connection of either kind, resolved.
#[derive(Clone, PartialEq)]
pub struct Connection {
    pub def: ConnectionDef,
    /// The catalog a **database** is addressed by, taken from the def rather than from a listing a
    /// collapsed row has not fetched. `None` on an object store, which has none.
    pub catalog: Option<String>,
    pub waiting: bool,
    pub problem: Option<String>,
}

/// The expansion set, as the walk reads it.
///
/// A borrow rather than the pane's `State`, because the walk asks it once per container and the
/// whole point of the walk is that it is a plain function of its inputs.
pub struct Open<'a>(pub &'a HashSet<String>);

impl Open<'_> {
    /// Is `path` **stored** open? Rarely what a container wants — see [`shows`](Self::shows).
    pub fn is_open(&self, path: &str) -> bool {
        self.0.contains(path)
    }

    /// Does `path` draw its children? The stored answer, **or** open regardless because a filter
    /// is narrowing the tree and this node kept something: keeping a node because a descendant
    /// matched and then hiding the match is worse than not keeping it at all.
    ///
    /// One method rather than the rule restated at each container, because the site that forgot to
    /// restate it was the workspace — the node holding the tables, views and saved queries a
    /// filter is mostly for.
    pub fn shows(&self, path: &str, kept: bool) -> bool {
        self.is_open(path) || kept
    }
}

/// Every row the tree is currently showing, top to bottom.
///
/// `needle` is **already lowercased** — see [`matches`](super::matches).
pub fn walk(
    project: &ProjectState,
    engine: &Engine,
    needle: &str,
    open: &HashSet<String>,
) -> Vec<Node> {
    let open = Open(open);
    let mut out = Vec::new();
    walk_workspace(project, needle, &open, &mut out);
    walk_connections(project, engine, needle, &open, &mut out);
    out
}
