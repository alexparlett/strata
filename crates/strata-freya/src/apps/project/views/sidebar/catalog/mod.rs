//! The **data-sources tree** (DB-05) — one tree answering "what data do I have", replacing both
//! the flat TABLES · VIEWS · QUERIES catalog pane and the Connections pane beside it.
//!
//! ## What the top level is
//!
//! Data sources. First the **project workspace** — labelled with the project's own name, because
//! it is not a "files provider" but the catalog Strata's federating engine defines: file tables,
//! internal tables, views and saved queries all live under it, and so does a **cross-source**
//! view joining workspace files to `pg.…`, since a node for a database groups by what it
//! *defines* rather than where the bytes live (the DataGrip/FDW precedent — a Postgres view over
//! a foreign table lives under Postgres). Then one node per connection: a **database** opens onto
//! its enabled schemas, and an **object store** onto the workspace defs that read through it, as
//! links rather than a second editable copy of those rows.
//!
//! ## Where the data comes from
//!
//! Two places, and the difference is the point. Everything under the workspace is the
//! [`ProjectState`] store — the project file's defs plus what engine registration *learned* about
//! each ([`Reg`](crate::apps::project::state::Reg)). **Not** an introspection query against
//! DataFusion, which would be wrong where it matters most: a def whose registration *failed* has
//! no engine presence at all yet is exactly the row the tree must keep showing. Everything under
//! a database connection is the opposite — there are no defs, because a database answers for
//! itself, so it is [`Engine::db_listing`](strata_core::engine::Engine::db_listing), which reads
//! the connect-time enumeration held beside the pool rather than the network. A ↻ re-connects,
//! and *that* is the refresh.
//!
//! ## Subscriptions
//!
//! Each node subscribes to its own [`ProjChan`], so a table registration landing wakes the TABLES
//! group alone — not the views, the saved queries or the connections. That is what the store's
//! per-section channels were built for, and it is why the tree is nested components rather than
//! one flat list built at the root.
//!
//! ## Local UI state
//!
//! Filter text, which nodes are open, and which nested columns are expanded are all
//! **pane-local** — none of it is project data, none of it persists. Expansion is one set keyed
//! by [node path](TreeCtx::open), which is what lets a jump from an object-store link open the
//! ancestors of a row three levels away.

mod columns;
mod database;
mod entry;
#[cfg(test)]
mod interaction;
mod menu;
mod row;
mod store;
mod workspace;

/// The catalog's own **actions**, for the command palette: its TABLES / VIEWS / SAVED QUERIES
/// rows are the same gestures as these menu items, so they call them rather than reimplementing
/// the SQL they generate and the `Origin` they bind a tab to.
pub use self::menu::{open_saved_query, use_catalog_actions, view_row, CatalogActions};

use std::collections::HashSet;

use freya::components::TreeConfig;
use freya::components::{define_theme, get_theme, ScrollConfig, ScrollController, ScrollView};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::util::contains_lowercased;
use strata_model::{ConnectionDef, ProviderId};

use self::database::DatabaseNode;
use self::row::{INDENT, ROW_HEIGHT};
use self::store::{AddConnectionRow, StoreNode};
use self::workspace::{seeded_paths, WorkspaceNode};
use crate::apps::project::state::{ProjChan, ProjectState};
use crate::components::metrics::{PANE_BODY_MIN_W, SP_3, SP_4};

define_theme!(
    %[component]
    pub Catalog {
        %[fields]
        label_color: Color,
        chevron_color: Color,
        name_color: Color,
        column_color: Color,
        meta_color: Color,
        /// The indent guide down each level of the tree, and the rail beside a column block.
        rail_fill: Color,
        /// The row's own dress — hover and selection, which the fork's `TreeItem` paints from
        /// the partial each row hands it.
        row_hover_fill: Color,
        row_selected_fill: Color,
        table_color: Color,
        /// A table whose data Strata owns (ED-04) — the row's icon *and* its `INTERNAL` badge,
        /// so the two marks read as one statement rather than two colours.
        internal_color: Color,
        view_color: Color,
        query_color: Color,
        /// A connection's provider badge (`S3` · `PG`), and the workspace node's own glyph.
        provider_color: Color,
        part_color: Color,
        part_background: Color,
        warn_color: Color,
    }
);

/// The tree's scroll inset.
const BODY_PAD: Gaps = Gaps::new(SP_3, SP_3, SP_4, SP_3);

/// Does `name` survive the filter? Case-insensitive substring, through the shared
/// [`contains_lowercased`], so this filter and every other one answer a needle the same way.
///
/// `needle` is **already lowercased**, which is that function's own contract and what every other
/// filter surface in the app honours: lowering inside the test allocates a `String` per *name*
/// rather than per keystroke, and this tree tests every def plus every relation of every open
/// database on three passes.
///
/// The filter spans **names at any depth the tree can enumerate for free** — defs, saved
/// queries, connections, schemas and relations — and deliberately not columns: a column name
/// surfacing its table was never this filter's job, and a remote relation's columns are an
/// introspection the pane will not run to answer a keystroke.
pub fn matches(name: &str, needle: &str) -> bool {
    needle.is_empty() || contains_lowercased(name, needle)
}

/// The tree's pane-local handles, in context because the tree is exactly the deep, open-ended
/// shape context is reserved for (state-arch §8): a schema node three levels down toggles the
/// same set the workspace node does, and a jump from an object-store link has to open ancestors
/// it cannot see.
#[derive(Clone, Copy, PartialEq)]
pub struct TreeCtx {
    /// Which node paths are open. One set for the whole tree, columns included, because a path
    /// is a path.
    pub open: State<HashSet<String>>,
    /// The node path a jump has asked to be shown — cleared by the row that answers it.
    pub reveal: State<Option<String>>,
    /// The body's scroller, so that row can bring itself into view.
    pub scroll: ScrollController,
}

impl TreeCtx {
    /// Is `path` **stored** open? Rarely what a container wants — see [`shows`](Self::shows).
    pub fn is_open(&self, path: &str) -> bool {
        self.open.read().contains(path)
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

    /// Open `path` if it is closed, close it if it is open — a chevron press.
    ///
    /// Takes the row's **effective** open state rather than flipping set membership, because a
    /// node a filter has forced open is stored *closed*: flipping membership there writes the
    /// wrong way, the row redraws open because the filter still forces it, and clearing the
    /// filter reveals the opposite of what the press appeared to do.
    pub fn toggle(&self, path: &str, open: bool) {
        let mut set = self.open;
        let mut set = set.write();
        match open {
            true => {
                set.remove(path);
            }
            false => {
                set.insert(path.to_string());
            }
        }
    }

    /// Open every path in `ancestors` and ask for `path` to be brought into view — the
    /// object-store link's jump.
    pub fn reveal(&self, ancestors: &[String], path: String) {
        let (mut open, mut reveal) = (self.open, self.reveal);
        let mut set = open.write();
        for ancestor in ancestors {
            set.insert(ancestor.clone());
        }
        drop(set);
        reveal.set(Some(path));
    }
}

/// The node paths open on a fresh pane: the workspace and its three groups, so a project opens
/// on its own catalog exactly as the flat pane did. Connections open on a press, because a
/// database's schemas are a listing and the user came here for their tables.
fn seeded() -> HashSet<String> {
    seeded_paths().into_iter().collect()
}

/// The data-sources tree — the sidebar body under the filter row. `filter` is owned by the
/// sidebar shell (it lives in the header row beside the ↻ and the `+`) and read here.
#[derive(PartialEq)]
pub struct Catalog {
    pub filter: State<String>,
    pub theme: Option<CatalogThemePartial>,
}

impl Catalog {
    pub fn new(filter: State<String>) -> Self {
        Self {
            filter,
            theme: None,
        }
    }
}

impl Component for Catalog {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, CatalogThemePreference, "catalog");
        let filter = self.filter.read().to_lowercase();

        let open = use_state(seeded);
        let reveal = use_state(|| None::<String>);
        let scroll = use_scroll_controller(ScrollConfig::default);
        use_provide_context(|| TreeCtx {
            open,
            reveal,
            scroll,
        });
        use_provide_context(|| TreeConfig {
            indent: INDENT,
            item_height: ROW_HEIGHT,
        });

        let radio = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
        let connections: Vec<ConnectionDef> = radio
            .read()
            .connections
            .iter()
            .map(|c| c.def.clone())
            .collect();
        let none_yet = connections.is_empty();

        let body = rect()
            .width(Size::fill())
            .min_width(Size::px(PANE_BODY_MIN_W))
            .vertical()
            .padding(BODY_PAD)
            .child(WorkspaceNode::new(filter.clone(), theme.clone()))
            .children(connections.into_iter().map(|def| {
                let url = def.url();
                match def.provider.id() {
                    ProviderId::Postgres => DatabaseNode::new(def, filter.clone(), theme.clone())
                        .key(url)
                        .into_element(),
                    _ => StoreNode::new(def, filter.clone(), theme.clone())
                        .key(url)
                        .into_element(),
                }
            }))
            .maybe_child(
                (none_yet && filter.is_empty())
                    .then(|| AddConnectionRow::new(theme).into_element()),
            );

        rect()
            .expanded()
            .child(ScrollView::new_controlled(scroll).child(body))
    }
}
