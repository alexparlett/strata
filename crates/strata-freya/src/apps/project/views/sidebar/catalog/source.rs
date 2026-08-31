//! The **data sources** branch of the walk, and the rows under it.
//!
//! Both kinds of data source draw one row ([`source_row`]) because both are one thing: a badge,
//! an address, a status glyph and the same three-item menu. What differs is what opens *underneath*
//! them, and that is the walk's business rather than the row's.
//!
//! What opens underneath is the whole difference, and **both halves arrive resolved**: the walk
//! is handed [`SourceNode`]s, which `state::sources::assemble` has already joined out of the
//! project's rows and the engine's one [`SourcesSnapshot`](strata_engine::sources::SourcesSnapshot).
//! A bucket cannot say what its tables are, so an object store's contents are **declared** — the
//! workspace defs that name it, as links back to their own rows rather than a second editable
//! copy. A database answers for itself, so its contents are **discovered**, from the connect-time
//! enumeration held beside the pool rather than from the network, already scoped and tagged, so
//! the tree, the schemas picker and completion all read one answer and none of them re-derives
//! visibility from the def. Collapsing and re-opening a schema costs nothing, and ↻ — which
//! re-connects — is the refresh.
//!
//! A relation **opens onto its columns** (DB-07), and it is the one node here whose children are
//! not free: they are a round trip through the provider the data source caches per relation. The
//! pane subscribes to them and hands the answer back to the walk, so the walk stays a plain
//! function of its inputs and a row still holds no state that identifies it.

use freya::prelude::*;
use strata_engine::sources::{SchemaListingView, SchemaVisibility};
use strata_engine::sql::SessionName;
use strata_engine::RemoteRelation;
use strata_model::{CatalogKind, ColOwner, RemoteRef};

use super::columns::flatten_cols;
use super::matches;
use super::menu::{query_relation, relation_menu, source_menu};
use super::node::{Column, Node, NodeKind, Open, Place, Remote, Source, Walked};
use super::row::{actions_button, fold_plan, name_width, tip, Row, StatusMark};
use super::view::{body, RowBody, RowCtx};
use super::workspace::{entry_ancestors, entry_path};
use crate::apps::project::query::RemoteSchemas;
use crate::apps::project::state::{SourceContents, SourceNode};
use crate::apps::source::SourceTarget;
use crate::components::badge::Badge;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_3, STATUS_DOT};
use crate::components::typography::{Body, Caption, Eyebrow, MonoValue};

/// Where a relation sits: data source ▸ schema ▸ Tables/Views ▸ **relation**. Named because its
/// children are laid out from it, and a hand-counted depth on a row that gained children is how a
/// tree quietly stops lining up.
const RELATION_DEPTH: usize = 3;

/// What a link row's trailing chevron says, on hover and to a screen reader — the trailing position
/// is the standard "this navigates" mark, and the leading disclosure slot is empty on a leaf, so
/// the two cannot be read for each other.
const JUMP: &str = "Show in the workspace";

/// What a schema that the def enables and the server does not have says on hover — the one
/// diagnosis the tree makes on its own, because nothing on our side observes a server-side drop or
/// rename.
///
/// "Not in the data source" means "not in what it last told us": the relation list is the
/// connect-time enumeration, so the fix is a ↻ (which re-connects) or an edit to the data source's
/// schemas.
fn missing_schema(name: &str) -> String {
    format!(
        "'{name}' is not in this data source. Refresh the catalog if it has since been created, \
         or remove it from the source's schemas."
    )
}

/// Every data source node, and whatever each of them has open.
pub fn walk_sources(
    sources: &[SourceNode],
    needle: &str,
    open: &Open,
    columns: &RemoteSchemas,
    out: &mut Walked,
) {
    for node in sources {
        match &node.contents {
            SourceContents::Catalog { catalog, schemas } => {
                database(node, catalog, schemas, needle, open, columns, out);
            }
            SourceContents::Store { tables } => store(node, tables, needle, open, &mut out.nodes),
        }
    }
    if sources.is_empty() && needle.is_empty() {
        out.nodes.push(Node::leaf(0, NodeKind::AddSource));
    }
}

/// A source, its enabled schemas, and the Tables / Views groups inside each.
///
/// Both halves arrive on the [`SourceNode`]: the schemas the data source shows — empty for one that
/// is not live, which is what makes an unreachable database a leaf — and the catalog it is
/// addressed by, which a collapsed row still has to say, so it comes from the def rather than from
/// a listing that data source may never have produced.
fn database(
    node: &SourceNode,
    catalog: &str,
    schemas: &[SchemaListingView],
    needle: &str,
    open: &Open,
    columns: &RemoteSchemas,
    out: &mut Walked,
) {
    let path = format!("conn/{}", node.name);
    let filtering = !needle.is_empty();
    let kept: Vec<&SchemaListingView> = schemas
        .iter()
        .filter(|schema| !filtering || survives(schema, needle))
        .collect();

    if filtering && !matches(&node.address, needle) && !matches(catalog, needle) && kept.is_empty()
    {
        return;
    }

    let shown = open.shows(&path, filtering && !kept.is_empty());
    out.nodes.push(Node::branch(
        0,
        path.clone(),
        shown,
        node.can_open(),
        NodeKind::Source(Source::of(node, Some(catalog.to_string()))),
    ));
    if !shown {
        return;
    }

    for schema in kept {
        let missing = schema.visibility == SchemaVisibility::EnabledButMissing;
        let schema_path = format!("{path}/{}", schema.name);
        let matched = filtering && schema.relations.iter().any(|r| matches(&r.name, needle));
        let schema_open = open.is_open(&schema_path) || matched;
        out.nodes.push(Node::branch(
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
                .filter(|r| r.view == views)
                .filter(|r| matches(&r.name, needle))
                .collect();
            let group_open = open.shows(&group_path, filtering && !relations.is_empty());
            out.nodes.push(Node::branch(
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
            for relation in relations {
                let reference = RemoteRef {
                    source: catalog.to_string(),
                    schema: schema.name.clone(),
                    relation: relation.name.clone(),
                };
                let relation_path = format!("{group_path}/{}", relation.name);
                let relation_open = open.is_open(&relation_path);
                out.nodes.push(Node::branch(
                    RELATION_DEPTH,
                    relation_path.clone(),
                    relation_open,
                    true,
                    NodeKind::Relation(Remote {
                        label: reference.label(),
                        address: SessionName::qualified([
                            reference.source.as_str(),
                            reference.schema.as_str(),
                            reference.relation.as_str(),
                        ])
                        .to_string(),
                        name: relation.name.clone(),
                        reference: reference.clone(),
                        view: views,
                    }),
                ));
                if !relation_open {
                    continue;
                }
                out.open_relations.push(reference.clone());
                relation_columns(
                    &relation_path,
                    &reference,
                    columns.get(&reference),
                    open,
                    &mut out.nodes,
                );
            }
        }
    }
}

/// The column rows under an open relation — or the one note that stands in for them.
///
/// **A relation is the one node in this tree whose children cost a round trip.** Everything else is
/// the project file or the connect-time enumeration; a relation's columns are the introspection
/// DB-02 made lazy on purpose, so that connecting to a database with a thousand tables is one
/// query rather than a thousand. So the three states are all real states, and each says which it
/// is: not read yet, read and refused, read.
///
/// A relation with no columns cannot happen server-side, so it takes the loading note rather than
/// an empty-list arm nothing can produce.
fn relation_columns(
    path: &str,
    reference: &RemoteRef,
    answer: Option<&Result<RemoteRelation, String>>,
    open: &Open,
    out: &mut Vec<Node>,
) {
    let columns = match answer {
        None => {
            out.push(Node::leaf(
                RELATION_DEPTH + 1,
                NodeKind::RelationNote {
                    text: LOADING_COLUMNS.to_string(),
                    problem: false,
                },
            ));
            return;
        }
        Some(Err(why)) => {
            out.push(Node::leaf(
                RELATION_DEPTH + 1,
                NodeKind::RelationNote {
                    text: why.clone(),
                    problem: true,
                },
            ));
            return;
        }
        Some(Ok(found)) => &found.columns,
    };

    let mut rows = Vec::new();
    flatten_cols(path, &[], 0, columns, &[], open.0, &mut rows);
    out.extend(rows.into_iter().map(|row| {
        Node::branch(
            RELATION_DEPTH + 1 + row.depth,
            row.key.clone(),
            row.is_expanded,
            row.has_children,
            NodeKind::Column(Column {
                owner: ColOwner::Remote(reference.clone()),
                row,
            }),
        )
    }));
}

/// What an open relation says while its one introspection is in flight.
const LOADING_COLUMNS: &str = "Reading columns…";

/// Does this schema, or anything in it, survive the filter?
fn survives(schema: &SchemaListingView, needle: &str) -> bool {
    matches(&schema.name, needle) || schema.relations.iter().any(|r| matches(&r.name, needle))
}

/// An object-store data source and the workspace defs reading through it.
///
/// The links arrive on the [`SourceNode`], joined once out of the project's tables: scanning per
/// bucket as the row is drawn costs a project full of tables the link names of every bucket on
/// every walk, closed ones included.
fn store(node: &SourceNode, tables: &[String], needle: &str, open: &Open, out: &mut Vec<Node>) {
    let path = format!("conn/{}", node.name);
    let filtering = !needle.is_empty();
    let kept: Vec<&String> = tables.iter().filter(|name| matches(name, needle)).collect();
    if filtering && !matches(&node.address, needle) && kept.is_empty() {
        return;
    }

    let shown = open.shows(&path, filtering && !kept.is_empty());
    out.push(Node::branch(
        0,
        path,
        shown,
        node.can_open(),
        NodeKind::Source(Source::of(node, None)),
    ));
    if shown {
        out.extend(
            kept.into_iter()
                .map(|name| Node::leaf(1, NodeKind::Link { name: name.clone() })),
        );
    }
}

/// One data source, of either kind.
///
/// The **mark** slot is a database's catalog label, whose width is its own, and nothing at all on an
/// object store: a slot budgeted for a mark the row never draws folds the ranks above it to pay for
/// room the row does not need.
pub fn source_row(at: &Place, source: &Source, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let mut measured = cx.measured;
    let actions = cx.sources;

    let (name, mode) = (source.name.clone(), source.mode);
    let address = source.address.clone();
    let mark = source.catalog.clone();
    let mark_slot = mark
        .as_ref()
        .map_or(0., |c| name_width(c, cx.advance) + SP_3);
    let folds = fold_plan(
        measured(),
        name_width(&address, cx.advance),
        true,
        mark_slot,
    );

    let build_menu = move || source_menu(&actions, name.clone(), mode);
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
                Badge::tag(source.badge.clone(), cx.theme.provider_color)
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

/// One schema of a data source — the def's enabled set, tagged against what the server
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

/// One relation inside a schema: its columns underneath, and the gestures on it (DB-06, DB-07).
///
/// **The chevron opens its columns and the press does not**, which is the *column* row's own shape
/// one level up rather than a new one — there, too, a press selects and only the disclosure opens.
/// It is what lets DB-06's gesture stand unchanged now that the row has children: a full read of a
/// remote table is still not something to start by pointing at it.
///
/// **Double-press queries it**, in the one `on_press` handler — a second registration under the
/// same event name would replace the first, so that is where a double is detected.
/// The ⋮ carries every gesture, and the right-click opens the same card.
///
/// **A press that is not a mouse press is an activation, not a failed double.** Wiring `on_press`
/// at all is what makes the fork's `TreeItem` a tab stop with the `Link` role and a focus ring —
/// its own comment is that those "promise an activation no key can perform" — so a keyboard Enter
/// has to *do* the gesture. There is no double-press to wait for on a keyboard, and every other
/// pressable row in this tree answers Enter.
pub fn relation_row(at: &Place, relation: &Remote, cx: &RowCtx) -> RowBody {
    let actions = cx.catalog.clone();
    let tree = cx.tree;
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
    let (open, path) = (at.open, at.path.clone());

    body(
        Row::new(at.depth, cx.theme.clone())
            .disclosure(at.disclosure())
            .on_toggle(move |_: Event<PressEventData>| tree.toggle(&path, open))
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

/// What an open relation shows in place of its columns — the one introspection in flight, or why
/// it did not answer.
///
/// A row rather than a spinner beside the relation's name: the wait belongs where the columns will
/// be, and a failure needs the room to say what to do about it.
pub fn relation_note_row(at: &Place, text: &str, problem: bool, cx: &RowCtx) -> RowBody {
    let color = match problem {
        true => cx.theme.warn_color,
        false => cx.theme.meta_color,
    };
    body(
        Row::new(at.depth, cx.theme.clone())
            .map(problem.then_some(color), |row, color| {
                row.child(Icon::new(IconName::Alert).color(color).size(12.))
            })
            .child(
                tip(text.to_string()).width(Size::flex(1.)).child(
                    rect().a11y_alt(text.to_string()).child(
                        Caption::new(text.to_string())
                            .color(color)
                            .width(Size::fill())
                            .text_overflow(TextOverflow::Ellipsis),
                    ),
                ),
            ),
    )
}

/// A workspace table read through this data source, as a **link**.
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

/// The pane's empty state: a project with no data sources at all.
///
/// One row rather than an illustrated tile, because it sits *under* a workspace node full of rows
/// rather than filling a pane of its own. It says what a data source is for and adds one; the
/// header's `+` is the same gesture, and the command palette's *New data source…* is the third.
pub fn add_source_row(at: &Place, cx: &RowCtx) -> RowBody {
    let mut editor = cx.editor;

    body(
        Row::new(at.depth, cx.theme.clone())
            .on_press(move |_: Event<PressEventData>| editor.set(Some(SourceTarget::New)))
            .child(
                Icon::new(IconName::Plus)
                    .color(cx.theme.meta_color)
                    .size(13.),
            )
            .child(
                Body::new("Add a data source")
                    .color(cx.theme.meta_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            ),
    )
}
