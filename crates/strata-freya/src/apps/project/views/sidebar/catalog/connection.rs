//! The **connections** branch of the walk, and the rows under it.
//!
//! Both kinds of connection draw one row ([`connection_row`]) because both are one thing: a badge,
//! an address, a status glyph and the same three-item menu. What differs is what opens *underneath*
//! them, and that is the walk's business rather than the row's.
//!
//! What opens underneath is the whole difference. A bucket cannot say what its tables are, so an
//! object store's contents are **declared**: its children are the workspace defs that name it
//! ([`ProjectState::tables_over`]), as links back to their own rows rather than a second editable
//! copy. A database answers for itself, so its contents are **discovered** — one call to
//! [`Engine::db_listing`], which reads the connect-time enumeration held beside the pool rather
//! than the network, already scoped and tagged, so the tree, the schemas picker and completion all
//! read one answer and none of them re-derives visibility from the def. Collapsing and re-opening a
//! schema costs nothing, and ↻ — which re-connects — is the refresh.
//!
//! A relation is a **leaf**: its columns are a round trip through the cached provider, and the
//! surface that reads them, and that can address one, is the inspector DB-07 builds.

use freya::prelude::*;
use strata_core::engine::db::{SchemaListingView, SchemaVisibility};
use strata_core::engine::sql::qualified;
use strata_core::engine::Engine;
use strata_model::{CatalogKind, Provider, ProviderId};

use super::matches;
use super::menu::{connection_menu, query_relation, relation_menu};
use super::node::{Connection, Node, NodeKind, Open, Place, Remote};
use super::row::{actions_button, fold_plan, name_width, tip, Row, StatusMark};
use super::view::{body, RowBody, RowCtx};
use super::workspace::{entry_ancestors, entry_path};
use crate::apps::connection::ConnectionTarget;
use crate::apps::project::state::{ConnRow, ProjectState, Reg};
use crate::components::badge::Badge;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_3, STATUS_DOT};
use crate::components::typography::{Body, Eyebrow, MonoValue};

/// What a link row's trailing chevron says, on hover and to a screen reader — the trailing position
/// is the standard "this navigates" mark, and the leading disclosure slot is empty on a leaf, so
/// the two cannot be read for each other.
const JUMP: &str = "Show in the workspace";

/// What a schema that the def enables and the server does not have says on hover — the one
/// diagnosis the tree makes on its own, because nothing on our side observes a server-side drop or
/// rename.
///
/// "Not in the connection" means "not in what it last told us": the relation list is the
/// connect-time enumeration, so the fix is a ↻ (which re-connects) or an edit to the connection's
/// schemas.
fn missing_schema(name: &str) -> String {
    format!(
        "'{name}' is not in this connection. Refresh the catalog if it has since been created, \
         or remove it from the connection's schemas."
    )
}

/// Every connection node, and whatever each of them has open.
pub fn walk_connections(
    project: &ProjectState,
    engine: &Engine,
    needle: &str,
    open: &Open,
    out: &mut Vec<Node>,
) {
    for row in &project.connections {
        match row.def.provider.id() {
            ProviderId::Postgres => database(engine, row, needle, open, out),
            _ => store(project, row, needle, open, out),
        }
    }
    if project.connections.is_empty() && needle.is_empty() {
        out.push(Node::leaf(0, NodeKind::AddConnection));
    }
}

/// A database connection, its enabled schemas, and the Tables / Views groups inside each.
///
/// The listing is fetched only for a node that is **connected and either open or being filtered**,
/// so a collapsed or unreachable database costs nothing. The catalog label comes from the def
/// regardless, because a collapsed row still has to say what it is addressed by — and a blank one is
/// *no* catalog rather than an empty one, since `Some("")` budgets a fold slot for a mark the row
/// then draws as an empty label, buying room at the provider badge's expense.
fn database(engine: &Engine, row: &ConnRow, needle: &str, open: &Open, out: &mut Vec<Node>) {
    let def = &row.def;
    let path = format!("conn/{}", def.url());
    let waiting = matches!(row.reg, Reg::Loading);
    let problem = row.reg.error().map(str::to_owned);
    let connected = row.reg.ready().is_some();
    let filtering = !needle.is_empty();
    let catalog = match &def.provider {
        Provider::Postgres(pg) => Some(pg.catalog.trim()).filter(|c| !c.is_empty()),
        _ => None,
    }
    .map(str::to_owned);

    let listing = (connected && (open.is_open(&path) || filtering))
        .then(|| engine.db_listing(def))
        .flatten();
    // The name relations are addressed by rides with them, because it is the *registered* one and
    // an unlisted connection has neither: no listing, no schemas, no relation to address.
    let (registered, schemas): (String, Vec<SchemaListingView>) = match listing {
        Some((registered, schemas)) => (
            registered,
            schemas
                .into_iter()
                .filter(|s| s.visibility != SchemaVisibility::NotEnabled)
                .filter(|s| !filtering || survives(s, needle))
                .collect(),
        ),
        None => (String::new(), Vec::new()),
    };

    let named =
        matches(&def.address, needle) || catalog.as_deref().is_some_and(|c| matches(c, needle));
    if filtering && !named && schemas.is_empty() {
        return;
    }

    let shown = open.shows(&path, filtering && !schemas.is_empty());
    out.push(Node::branch(
        0,
        path.clone(),
        shown,
        connected,
        NodeKind::Connection(Connection {
            def: def.clone(),
            catalog,
            waiting,
            problem,
        }),
    ));
    if !shown {
        return;
    }

    for schema in schemas {
        let missing = schema.visibility == SchemaVisibility::EnabledButMissing;
        let schema_path = format!("{path}/{}", schema.name);
        let matched = filtering && schema.relations.iter().any(|r| matches(&r.name, needle));
        let schema_open = open.is_open(&schema_path) || matched;
        out.push(Node::branch(
            1,
            schema_path.clone(),
            schema_open,
            !missing,
            NodeKind::Schema {
                name: schema.name.clone(),
                missing,
            },
        ));
        if missing || !schema_open {
            continue;
        }

        for views in [false, true] {
            let group_path = format!("{schema_path}/{}", if views { "views" } else { "tables" });
            let relations: Vec<&_> = schema
                .relations
                .iter()
                .filter(|r| r.is_view() == views)
                .filter(|r| matches(&r.name, needle))
                .collect();
            let group_open = open.shows(&group_path, filtering && !relations.is_empty());
            out.push(Node::branch(
                2,
                group_path.clone(),
                group_open,
                !relations.is_empty(),
                NodeKind::RelGroup {
                    views,
                    count: relations.len(),
                },
            ));
            if !group_open {
                continue;
            }
            out.extend(relations.into_iter().map(|relation| {
                Node::leaf(
                    3,
                    NodeKind::Relation(Remote {
                        label: format!("{registered}.{}.{}", schema.name, relation.name),
                        address: qualified([
                            registered.as_str(),
                            schema.name.as_str(),
                            relation.name.as_str(),
                        ]),
                        name: relation.name.clone(),
                        view: views,
                    }),
                )
            }));
        }
    }
}

/// Does this schema, or anything in it, survive the filter?
fn survives(schema: &SchemaListingView, needle: &str) -> bool {
    matches(&schema.name, needle) || schema.relations.iter().any(|r| matches(&r.name, needle))
}

/// An object-store connection and the workspace defs reading through it.
///
/// A collapsed node asks only **whether** it has children, never which: `tables_over` clones a name
/// per match, and the walk runs on every registration, so a project full of tables was paying for
/// every link name of every closed bucket and dropping them all.
fn store(project: &ProjectState, row: &ConnRow, needle: &str, open: &Open, out: &mut Vec<Node>) {
    let def = &row.def;
    let url = def.url();
    let path = format!("conn/{url}");
    let filtering = !needle.is_empty();
    let any = project
        .tables
        .iter()
        .any(|t| t.def.connection.as_deref() == Some(url.as_str()) && matches(&t.def.name, needle));
    if filtering && !matches(&def.address, needle) && !any {
        return;
    }

    let shown = open.shows(&path, filtering && any);
    out.push(Node::branch(
        0,
        path,
        shown,
        any,
        NodeKind::Connection(Connection {
            def: def.clone(),
            catalog: None,
            waiting: matches!(row.reg, Reg::Loading),
            problem: row.reg.error().map(str::to_owned),
        }),
    ));
    if shown {
        out.extend(
            project
                .tables_over(&url)
                .into_iter()
                .filter(|name| matches(name, needle))
                .map(|name| Node::leaf(1, NodeKind::Link { name })),
        );
    }
}

/// One connection, of either kind.
///
/// The **mark** slot is a database's catalog label, whose width is its own, and nothing at all on an
/// object store: a slot budgeted for a mark the row never draws folds the ranks above it to pay for
/// room the row does not need.
pub fn connection_row(at: &Place, connection: &Connection, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let mut measured = cx.measured;
    let actions = cx.connections;

    let (url, provider) = (connection.def.url(), connection.def.provider.id());
    let address = connection.def.address.clone();
    let mark = connection.catalog.clone();
    let mark_slot = mark
        .as_ref()
        .map_or(0., |c| name_width(c, cx.advance) + SP_3);
    let folds = fold_plan(
        measured(),
        name_width(&address, cx.advance),
        true,
        mark_slot,
    );

    let build_menu = move || connection_menu(&actions, url.clone(), provider);
    let menu_for_row = build_menu.clone();
    let (open, path) = (at.open, at.path.clone());
    let toggle = move |_: Event<PressEventData>| tree.toggle(&path, open);

    body(
        Row::new(at.depth, cx.theme.clone())
            .disclosure(at.disclosure())
            .on_press(toggle.clone())
            .on_toggle(toggle)
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            .on_sized(move |e: Event<SizedEventData>| {
                measured.set_if_modified(e.area.width());
            })
            .trailing(actions_button(build_menu))
            .maybe_child(folds.badge.then(|| {
                Badge::tag(connection.def.provider.to_string(), cx.theme.provider_color)
                    .outlined()
                    .height(16.)
                    .into_element()
            }))
            .child(
                MonoValue::new(address)
                    .color(cx.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(mark.filter(|_| folds.mark).map(|catalog| {
                MonoValue::new(catalog)
                    .color(cx.theme.meta_color)
                    .into_element()
            }))
            .maybe_child(folds.status.then(|| {
                rect()
                    .width(Size::px(STATUS_DOT))
                    .cross_align(Alignment::Center)
                    .maybe_child(cx.status.as_ref().map(|s| s.glyph(&cx.theme)))
                    .into_element()
            })),
    )
}

/// One schema of a database connection — the def's enabled set, tagged against what the server
/// answered.
pub fn schema_row(at: &Place, name: &str, missing: bool, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let (open, path) = (at.open, at.path.clone());
    let pressable = (!missing).then_some(path);

    body(
        Row::new(at.depth, cx.theme.clone())
            .disclosure(at.disclosure())
            .map(pressable.clone(), |row, path| {
                row.on_press(move |_: Event<PressEventData>| tree.toggle(&path, open))
            })
            .map(pressable, |row, path| {
                row.on_toggle(move |_: Event<PressEventData>| tree.toggle(&path, open))
            })
            .child(
                Icon::new(IconName::Folder)
                    .color(cx.theme.chevron_color)
                    .size(13.),
            )
            .child(
                MonoValue::new(name.to_string())
                    .color(cx.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(
                missing.then(|| StatusMark::Problem(missing_schema(name)).glyph(&cx.theme)),
            ),
    )
}

/// The Tables / Views split inside a schema, from the listing's own `relkind`.
pub fn rel_group_row(at: &Place, views: bool, count: usize, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let (open, path) = (at.open, at.path.clone());
    let label = if views { "VIEWS" } else { "TABLES" };

    body(
        Row::new(at.depth, cx.theme.clone())
            .disclosure(at.disclosure())
            .on_press({
                let path = path.clone();
                move |_: Event<PressEventData>| tree.toggle(&path, open)
            })
            .on_toggle(move |_: Event<PressEventData>| tree.toggle(&path, open))
            .child(
                Eyebrow::new(format!("{label} · {count}"))
                    .color(cx.theme.label_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            ),
    )
}

/// One relation inside a schema, and the two gestures on it (DB-06).
///
/// A leaf, and the disclosure it does not draw is the honest mark of where the tree stops: its
/// columns are an introspection, and the surface that reads them is the inspector DB-07 builds.
///
/// **Double-press queries it**, in the one `on_press` handler — a second registration under the
/// same event name would replace the first, so that is where a double is detected (AGENTS.md §3).
/// A single *mouse* press deliberately does nothing: the row is a leaf, so there is no disclosure
/// for it to mean, and a full read of a remote table is not something to start by pointing at it.
/// The ⋮ carries both gestures, and the right-click opens the same card.
///
/// **A press that is not a mouse press is an activation, not a failed double.** Wiring `on_press`
/// at all is what makes the fork's `TreeItem` a tab stop with the `Link` role and a focus ring —
/// its own comment is that those "promise an activation no key can perform" — so a keyboard Enter
/// has to *do* the gesture. There is no double-press to wait for on a keyboard, and every other
/// pressable row in this tree answers Enter.
pub fn relation_row(at: &Place, relation: &Remote, cx: &RowCtx) -> RowBody {
    let actions = cx.catalog.clone();
    let (icon, color) = match relation.view {
        true => (IconName::Eye, cx.theme.view_color),
        false => (IconName::Database, cx.theme.table_color),
    };

    let build_menu = {
        let (actions, relation) = (actions.clone(), relation.clone());
        move || relation_menu(&actions, &relation)
    };
    let menu_for_row = build_menu.clone();
    let queried = relation.clone();

    body(
        Row::new(at.depth, cx.theme.clone())
            .on_press(move |e: Event<PressEventData>| {
                let activates = match e.data() {
                    PressEventData::Mouse(m) => {
                        EventsCombos::pressed(m.global_location).is_double()
                    }
                    _ => true,
                };
                if activates {
                    query_relation(&actions, &queried);
                }
            })
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            .trailing(actions_button(build_menu))
            .child(Icon::new(icon).color(color).size(14.))
            .child(
                MonoValue::new(relation.name.clone())
                    .color(cx.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            ),
    )
}

/// A workspace table read through this connection, as a **link**.
///
/// It carries a jump affordance and no menu: the def's own row is the thing with a menu, and
/// pressing this is how you get there.
pub fn link_row(at: &Place, name: &str, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let target = name.to_string();

    body(
        Row::new(at.depth, cx.theme.clone())
            .on_press(move |_: Event<PressEventData>| {
                tree.reveal(
                    &entry_ancestors(CatalogKind::Table),
                    entry_path(CatalogKind::Table, &target),
                );
            })
            .child(
                Icon::new(IconName::Database)
                    .color(cx.theme.table_color)
                    .size(14.),
            )
            .child(
                MonoValue::new(name.to_string())
                    .color(cx.theme.column_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .child(
                tip(JUMP).child(
                    rect().a11y_alt(JUMP).child(
                        Icon::new(IconName::ChevronRight)
                            .color(cx.theme.chevron_color)
                            .size(12.),
                    ),
                ),
            ),
    )
}

/// The pane's empty state: a project with no connections at all.
///
/// One row rather than an illustrated tile, because it sits *under* a workspace node full of rows
/// rather than filling a pane of its own. It says what a connection is for and adds one; the
/// header's `+` is the same gesture, and the command palette's *New connection…* is the third.
pub fn add_connection_row(at: &Place, cx: &RowCtx) -> RowBody {
    let mut editor = cx.editor;

    body(
        Row::new(at.depth, cx.theme.clone())
            .on_press(move |_: Event<PressEventData>| editor.set(Some(ConnectionTarget::New)))
            .child(
                Icon::new(IconName::Plus)
                    .color(cx.theme.meta_color)
                    .size(13.),
            )
            .child(
                Body::new("Add a connection")
                    .color(cx.theme.meta_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            ),
    )
}
