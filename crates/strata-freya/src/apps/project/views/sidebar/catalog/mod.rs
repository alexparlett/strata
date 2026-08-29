//! The **data-sources tree** (DB-05) — one tree answering "what data do I have", replacing both
//! the flat TABLES · VIEWS · QUERIES catalog pane and the Connections pane beside it.
//!
//! ## What the top level is
//!
//! Data sources. First the **project workspace** — labelled with the project's own name, because it
//! is not a "files provider" but the catalog Strata's federating engine defines: file tables,
//! internal tables, views and saved queries all live under it, and so does a **cross-source** view
//! joining workspace files to `pg.…`, since a node for a database groups by what it *defines*
//! rather than where the bytes live (the DataGrip/FDW precedent — a Postgres view over a foreign
//! table lives under Postgres). Then one node per connection: a **database** opens onto its enabled
//! schemas, and an **object store** onto the workspace defs that read through it, as links rather
//! than a second editable copy of those rows.
//!
//! ## How it is built
//!
//! **The pane walks the tree into a flat list of visible rows and hands that to the fork's
//! virtualized [`Tree`]** (`node::walk`), so only the rows on screen are mounted. That matters for
//! one node kind above all: everything else is bounded by the project file, but a schema's relation
//! list is the *server's*, and `RELATIONS_QUERY` carries no `LIMIT` — so opening one is the only
//! place a row count nobody here decides reaches the layout.
//!
//! ## Where the data comes from
//!
//! Two places, and the difference is the point. Everything under the workspace is the
//! [`ProjectState`] store — the project file's defs plus what engine registration *learned* about
//! each ([`Reg`](crate::apps::project::state::Reg)). **Not** an introspection query against
//! DataFusion, which would be wrong where it matters most: a def whose registration *failed* has no
//! engine presence at all yet is exactly the row the tree must keep showing. Everything under a
//! database connection is the opposite — there are no defs, because a database answers for itself,
//! so it is [`Connections::listing`](strata_engine::Connections::listing), which reads the
//! connect-time enumeration held beside the pool rather than the network. A ↻ re-connects, and
//! *that* is the refresh.
//!
//! **The two are joined once, before the walk** ([`assemble`]), so every connection row is drawn
//! from the same moment. It is a `use_side_effect_value` rather than a line in the render, which
//! is what keeps the join off the keystroke path: a schema's relation list is the *server's* and
//! can be enormous, and this re-runs only when one of the three things it reads moves — `Tables`,
//! `Connections`, the catalog generation. **Value**, not a plain effect writing a slot: it
//! computes once at mount, so there is no pass where the pane has a project full of connections
//! and nothing to draw for them. The walk it feeds reaches no engine at all, which is what makes
//! it the plain function of its inputs it is supposed to be.
//!
//! ## Subscriptions
//!
//! **The pane root subscribes to every section the walk reads** — `Meta` for the project's name,
//! then `Tables`, `Views` and `Queries`. That is what virtualizing costs: one registration landing
//! re-walks the tree, where the nested pane woke the one group it belonged to. The list is
//! exhaustive on purpose, because a walk input nothing subscribes to is a row that goes stale until
//! something unrelated happens to wake the pane. `Connections` is subscribed by the **join** rather
//! than here, beside the catalog epoch and `Tables`, because those three are exactly what the join
//! reads — and a write to any of them has to re-run it, not merely redraw what it last produced.
//!
//! ## The one subscription that is not free
//!
//! A remote relation's **columns** are a round trip (DB-07), and the pane holds that subscription
//! rather than a row: the walk decides which relations are open and it cannot await, and a
//! virtualized row's scope is a slot that scrolling hands to somebody else. The walk returns what
//! it drew open, an effect moves that into the query's key, and the answer comes back as an input
//! on the next pass — so opening a relation costs one extra pass, during which that row shows its
//! loading note because the read has not happened either way.
//!
//! **What the walk is handed is *accumulated*, not the query's current value**, and that is the
//! difference between a working tree and a flickering one. The key is the whole open set plus the
//! catalog generation, and freya-query starts a changed key at `Pending` with no carried value — so
//! reading the entry directly would blank *every* already-drawn relation back to its loading note
//! whenever any other relation was opened, or whenever any unrelated catalog pass moved that
//! number.
//! Merging each settled answer into a map the pane keeps is the same rule the inspector's
//! STATISTICS zone holds (`views/inspector/column.rs`): **never show less than a moment ago.** The
//! map only grows, bounded by the relations opened in this window's life, and a relation the
//! server has since dropped is corrected by the `Err` its next answer merges over the old one.
//!
//! ## Local UI state
//!
//! Filter text, which nodes are open, which nested columns are expanded and which saved query is
//! being renamed are all **pane-local** — none of it is project data, none of it persists. It is
//! also all on [`TreeCtx`] rather than in a row, and that is not a preference: a virtualized row's
//! scope is a **slot**, so scrolling hands it a different row, and anything the slot remembered
//! would then be remembered about the wrong one.

mod columns;
mod connection;
mod entry;
#[cfg(test)]
mod interaction;
mod menu;
mod node;
mod row;
mod view;
mod workspace;

/// The catalog's own **actions**, for the command palette: its TABLES / VIEWS / SAVED QUERIES rows
/// are the same gestures as these menu items, so they call them rather than reimplementing the SQL
/// they generate and the `Origin` they bind a tab to.
pub use self::menu::{open_saved_query, use_catalog_actions, view_row, CatalogActions};

use std::collections::HashSet;
use std::rc::Rc;

use freya::components::{define_theme, get_theme, ScrollConfig, Tree, TreeThemePartial};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::util::contains_lowercased;
use uuid::Uuid;

use self::menu::rename_saved_query;
use self::node::{walk, Node, NodeKind, Walked};
use self::row::{INDENT, ROW_HEIGHT};
use self::view::TreeRow;
use self::workspace::seeded_paths;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::use_remote_schemas;
use crate::apps::project::state::{assemble, use_catalog, ProjChan, ProjectState};
use crate::components::metrics::{SP_3, SP_4};
use crate::keymap::on_command;
use crate::state::use_config_station;
use strata_core::config::Command;

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
///
/// There is no `PANE_BODY_MIN_W` floor beside it, and that is the difference between a tree and the
/// prose bodies that carry one: a floor exists to stop *wrapping* degrading into one character per
/// line, and a tree row ellipsizes rather than wraps. On the pane's own frame it would stop the
/// panel shrinking instead, which is the opposite of the rule it comes from.
const BODY_PAD: Gaps = Gaps::new(SP_3, SP_3, SP_4, SP_3);

/// Does `name` survive the filter? Case-insensitive substring, through the shared
/// [`contains_lowercased`], so this filter and every other one answer a needle the same way.
///
/// `needle` is **already lowercased**, which is that function's own contract and what every other
/// filter surface in the app honours: lowering inside the test allocates a `String` per *name*
/// rather than per keystroke, and this tree tests every def plus every relation of every open
/// database on every walk.
///
/// The filter spans **names at any depth the tree can enumerate for free** — defs, saved queries,
/// connections, schemas and relations — and deliberately not columns: a column name surfacing its
/// table was never this filter's job, and a remote relation's columns are an introspection the pane
/// will not run to answer a keystroke.
pub fn matches(name: &str, needle: &str) -> bool {
    needle.is_empty() || contains_lowercased(name, needle)
}

/// The tree's pane-local handles, in context because the tree is exactly the deep, open-ended shape
/// context is reserved for (state-arch §8): a row four levels down toggles the same set the
/// workspace node does, and a jump from an object-store link has to open ancestors it cannot see.
#[derive(Clone, Copy, PartialEq)]
pub struct TreeCtx {
    /// Which node paths are open. One set for the whole tree, columns included, because a path is a
    /// path.
    pub open: State<HashSet<String>>,
    /// The node path a jump has asked to be shown — cleared by the pane once it has answered.
    ///
    /// Answered by the target's **index** in the flat list, because the row a jump names is usually
    /// not built, which for a virtualized list is the ordinary case rather than the exception. The
    /// scroller is deliberately not here beside this slot: only the pane holds that index, so a row
    /// that could reach the scroller could reach it with nothing to say.
    pub reveal: State<Option<String>>,
    /// Which saved query is being renamed, if any, and the text typed so far.
    ///
    /// Both are the pane's rather than the row's, and the draft has to travel with the flag: a
    /// virtualized row's scope is a slot, so scrolling the row out of the window destroys anything
    /// it was holding. Kept in the row, the draft was silently re-seeded from the stored name on the
    /// way back in, and a commit then wrote the name the user had just replaced.
    pub renaming: State<Option<Uuid>>,
    pub draft: State<String>,
}

impl TreeCtx {
    /// Open `path` if it is closed, close it if it is open — a chevron press.
    ///
    /// Takes the row's **effective** open state rather than flipping set membership, because a node
    /// a filter has forced open is stored *closed*: flipping membership there writes the wrong way,
    /// the row redraws open because the filter still forces it, and clearing the filter reveals the
    /// opposite of what the press appeared to do.
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

    /// Open every path in `ancestors` and ask for `path` to be brought into view — the object-store
    /// link's jump.
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

/// The rename that has lost the row it was being typed into, if any.
///
/// A rename ends when its row leaves the tree, and it ends the way a blur does. The row owns the
/// commit-on-outside-press and a virtualized row is unmounted by a scroll or a filter keystroke, so
/// a rename whose row has gone has nothing on screen left to commit it.
fn stranded_rename(nodes: &[Node], renaming: Option<Uuid>) -> Option<Uuid> {
    let id = renaming?;
    let drawn = nodes
        .iter()
        .any(|node| matches!(&node.kind, NodeKind::SavedQuery { id: row, .. } if *row == id));
    (!drawn).then_some(id)
}

/// The node paths open on a fresh pane: the workspace and its three groups, so a project opens on
/// its own catalog exactly as the flat pane did. Connections open on a press, because a database's
/// schemas are a listing and the user came here for their tables.
fn seeded() -> HashSet<String> {
    seeded_paths().into_iter().collect()
}

/// What the row builder is handed.
///
/// Both halves ride in the builder **data** rather than being captured, because a
/// `VirtualScrollView` memoizes its builder closure: anything captured there goes stale on the next
/// walk or the next theme.
#[derive(Clone, PartialEq)]
struct TreeData {
    nodes: Rc<Vec<Node>>,
    theme: CatalogTheme,
}

/// The data-sources tree — the sidebar body under the filter row. `filter` is owned by the sidebar
/// shell (it lives in the header row beside the ↻ and the `+`) and read here.
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
        let needle = self.filter.read().to_lowercase();
        let engine = use_consume::<EngineCtx>();
        let config = use_config_station();
        let actions = use_catalog_actions();

        let open = use_state(seeded);
        let reveal = use_state(|| None::<String>);
        let renaming = use_state(|| None::<Uuid>);
        let draft = use_state(String::new);
        let mut scroll = use_scroll_controller(ScrollConfig::default);
        use_provide_context(|| TreeCtx {
            open,
            reveal,
            renaming,
            draft,
        });

        let meta = use_radio::<ProjectState, ProjChan>(ProjChan::Meta);
        let tables = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let queries = use_radio::<ProjectState, ProjChan>(ProjChan::Queries);
        let connections = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
        drop(meta.read());
        drop(views.read());
        drop(queries.read());

        let wanted = use_state(Vec::new);
        let catalog = use_catalog();
        let generation = catalog.read().generation();
        let described = use_remote_schemas(&engine, wanted.read().clone(), generation);

        let sources = use_side_effect_value(move || {
            drop(catalog.read());
            drop(connections.read());
            assemble(&tables.read(), &engine.sources().listing())
        });

        let Walked {
            nodes,
            open_relations,
        } = {
            let project = tables.read();
            let expanded = open.read();
            walk(&project, &sources.read(), &needle, &expanded, &described)
        };
        use_side_effect_with_deps(&open_relations, move |relations| {
            let mut wanted = wanted;
            wanted.set_if_modified(relations.clone());
        });

        let wanted = reveal.read().clone();
        let target = wanted
            .as_deref()
            .and_then(|path| nodes.iter().position(|node| node.path() == Some(path)));
        let renamed = stranded_rename(&nodes, *renaming.read());
        use_side_effect_with_deps(&renamed, move |renamed| {
            let Some(id) = *renamed else {
                return;
            };
            let (mut renaming, mut draft) = (renaming, draft);
            rename_saved_query(&actions, id, &draft.peek().clone());
            draft.set(String::new());
            renaming.set(None);
        });

        use_side_effect_with_deps(&(wanted, target), move |(wanted, target)| {
            if wanted.is_none() {
                return;
            }
            if let Some(index) = target {
                scroll.scroll_to_offset(
                    *index as f32 * ROW_HEIGHT,
                    ROW_HEIGHT,
                    Direction::Vertical,
                );
            }
            let mut reveal = reveal;
            reveal.set(None);
        });

        let length = nodes.len();
        let data = TreeData {
            nodes: Rc::new(nodes),
            theme,
        };

        rect()
            .expanded()
            .padding(BODY_PAD)
            .on_global_key_down(on_command(config, Command::Cancel, move || {
                let mut renaming = renaming;
                renaming.peek().is_some() && renaming.take().is_some()
            }))
            .child(
                Tree::new_with_data(data, |item: VirtualItem, data: &TreeData| {
                    match data.nodes.get(item.index) {
                        Some(node) => TreeRow {
                            node: node.clone(),
                            theme: data.theme.clone(),
                        }
                        .into_element(),
                        None => rect().into_element(),
                    }
                })
                .length(length)
                .scroll_controller(scroll)
                .theme(
                    TreeThemePartial::default()
                        .background(Color::TRANSPARENT)
                        .indent(INDENT)
                        .item_height(ROW_HEIGHT),
                ),
            )
    }
}
