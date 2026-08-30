//! The **workspace** branch of the walk — the project's own database — and the three rows that are
//! structure rather than content: the workspace node, one of its groups, and the note an empty
//! QUERIES group leaves.
//!
//! Labelled with the project's name rather than "workspace", because that is what it is: the
//! catalog Strata's federating engine defines, addressed as `strata`, holding everything the
//! project declares. Its children are the flat pane's TABLES · VIEWS · QUERIES sections verbatim —
//! same rows, same `Reg` status slots, same menus, same columns expansion — one level further in.

use freya::prelude::*;
use strata_engine::Registrations;
use strata_model::{CatalogKind, ColOwner, ColumnInfo, WORKSPACE_CATALOG};

use super::columns::flatten_cols;
use super::matches;
use super::node::{Column, Entry, Node, NodeKind, Open, Place};
use super::row::Row;
use super::view::{body, RowBody, RowCtx};
use crate::apps::configure::ConfigureTarget;
use crate::apps::project::state::ProjectState;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::ROW_ACTION;
use crate::components::typography::{Body, Caption, Eyebrow, MonoValue};

/// The workspace node's own path — the root of every path under it.
pub const WORKSPACE: &str = "ws";

/// How deep a workspace entry sits: the workspace node, then its group.
pub const ENTRY_DEPTH: usize = 2;

/// One of the workspace's three groups.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Tables,
    Views,
    Queries,
}

impl Group {
    /// Which group a catalog kind belongs to — the one place the mapping is written, so a fourth
    /// kind is a compile error here and nowhere else.
    pub fn of(kind: CatalogKind) -> Self {
        match kind {
            CatalogKind::View => Group::Views,
            CatalogKind::Query => Group::Queries,
            CatalogKind::Table => Group::Tables,
        }
    }

    /// The group's node path — the prefix every row under it is addressed by.
    pub fn path(self) -> &'static str {
        match self {
            Group::Tables => "ws/tables",
            Group::Views => "ws/views",
            Group::Queries => "ws/queries",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Group::Tables => "TABLES",
            Group::Views => "VIEWS",
            Group::Queries => "QUERIES",
        }
    }
}

/// The node paths a fresh pane opens on — the workspace and its three groups, so a project opens
/// on its own catalog exactly as the flat pane did.
pub fn seeded_paths() -> Vec<String> {
    vec![
        WORKSPACE.to_string(),
        Group::Tables.path().to_string(),
        Group::Views.path().to_string(),
        Group::Queries.path().to_string(),
    ]
}

/// The node path of a workspace def's row — so the object-store link rows can address one without
/// restating how a path is spelled.
pub fn entry_path(kind: CatalogKind, name: &str) -> String {
    format!("{}/{name}", Group::of(kind).path())
}

/// The nodes a jump has to open before [`entry_path`] can be seen.
pub fn entry_ancestors(kind: CatalogKind) -> Vec<String> {
    vec![WORKSPACE.to_string(), Group::of(kind).path().to_string()]
}

/// The workspace node, its three groups, and everything they hold.
///
/// The workspace holds the rows a filter is mostly for, so it opens on a match like any other
/// container; its groups then narrow themselves.
pub fn walk_workspace(
    project: &ProjectState,
    registrations: &Registrations,
    needle: &str,
    open: &Open,
    out: &mut Vec<Node>,
) {
    let answers = &registrations.workspace;
    let filtering = !needle.is_empty();
    let shown = open.shows(WORKSPACE, filtering);
    out.push(Node::branch(
        0,
        WORKSPACE.to_string(),
        shown,
        true,
        NodeKind::Workspace {
            name: project.name.clone(),
        },
    ));
    if !shown {
        return;
    }

    let tables: Vec<&_> = project
        .tables
        .iter()
        .filter(|t| matches(&t.def.name, needle))
        .collect();
    if group(Group::Tables, tables.len(), needle, open, out) {
        for row in tables {
            entry(
                Entry {
                    kind: CatalogKind::Table,
                    name: row.def.name.clone(),
                    internal: row.def.origin.is_internal(),
                    waiting: answers.of(&row.def.name).is_none(),
                    problem: answers.problem(&row.def.name).map(str::to_owned),
                    scan: row.profile,
                },
                row.meta.as_ref().map(|meta| meta.columns.as_slice()),
                &row.def.partition_cols,
                open,
                out,
            );
        }
    }

    let views: Vec<&_> = project
        .views
        .iter()
        .filter(|v| matches(&v.def.name, needle))
        .collect();
    if group(Group::Views, views.len(), needle, open, out) {
        for row in views {
            entry(
                Entry {
                    kind: CatalogKind::View,
                    name: row.def.name.clone(),
                    internal: false,
                    waiting: answers.of(&row.def.name).is_none(),
                    problem: project.view_problem(row, registrations),
                    scan: row.profile,
                },
                row.info.as_ref().map(|info| info.columns.as_slice()),
                &[],
                open,
                out,
            );
        }
    }

    let queries: Vec<&_> = project
        .saved_queries
        .iter()
        .filter(|q| matches(&q.name, needle))
        .collect();
    if group(Group::Queries, queries.len(), needle, open, out) {
        for query in &queries {
            out.push(Node::leaf(
                ENTRY_DEPTH,
                NodeKind::SavedQuery {
                    id: query.id,
                    name: query.name.clone(),
                },
            ));
        }
        if queries.is_empty() && !filtering {
            out.push(Node::leaf(ENTRY_DEPTH, NodeKind::NoQueries));
        }
    }
}

/// Push a group's header row and answer whether its contents follow.
///
/// **A group is structural, so it stays while a filter is typed** and its count follows the filter.
/// That is the one exception to the tree's "keep a node only if it or a descendant matches" rule,
/// and it is deliberate: the count going to `0` is what tells the user the filter found nothing
/// here, which an absent row cannot say.
fn group(group: Group, count: usize, needle: &str, open: &Open, out: &mut Vec<Node>) -> bool {
    let path = group.path();
    let shown = open.shows(path, !needle.is_empty() && count > 0);
    out.push(Node::branch(
        1,
        path.to_string(),
        shown,
        true,
        NodeKind::Group { group, count },
    ));
    shown
}

/// Push one entry and, when it is open, the column rows under it.
///
/// `columns` is `Some` only on a **registered** row, which is also the only way a row can be
/// expandable: a def whose registration failed has nothing to open.
fn entry(
    resolved: Entry,
    columns: Option<&[ColumnInfo]>,
    partitions: &[(String, String)],
    open: &Open,
    out: &mut Vec<Node>,
) {
    let path = entry_path(resolved.kind, &resolved.name);
    let is_open = open.is_open(&path);
    let columns = columns.filter(|c| !c.is_empty());
    let owner = ColOwner::Entry {
        kind: resolved.kind,
        name: resolved.name.clone(),
    };
    out.push(Node::branch(
        ENTRY_DEPTH,
        path.clone(),
        is_open,
        columns.is_some(),
        NodeKind::Entry(resolved),
    ));

    let Some(columns) = columns.filter(|_| is_open) else {
        return;
    };
    let mut rows = Vec::new();
    flatten_cols(&path, &[], 0, columns, partitions, open.0, &mut rows);
    out.extend(rows.into_iter().map(|row| {
        Node::branch(
            ENTRY_DEPTH + 1 + row.depth,
            row.key.clone(),
            row.is_expanded,
            row.has_children,
            NodeKind::Column(Column {
                owner: owner.clone(),
                row,
            }),
        )
    }));
}

/// The project's own database.
pub fn workspace_row(at: &Place, name: &str, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let (open, path) = (at.open, at.path.clone());

    body(
        Row::new(at.depth, cx.theme.clone())
            .disclosure(at.disclosure())
            .on_press({
                let path = path.clone();
                move |_: Event<PressEventData>| tree.toggle(&path, open)
            })
            .on_toggle(move |_: Event<PressEventData>| tree.toggle(&path, open))
            .child(
                Icon::new(IconName::Database)
                    .color(cx.theme.provider_color)
                    .size(14.),
            )
            .child(
                Body::new(name.to_string())
                    .color(cx.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .child(MonoValue::new(WORKSPACE_CATALOG).color(cx.theme.meta_color)),
    )
}

/// One of the workspace's three groups — `▾ TABLES · 4`, and the TABLES group's `+`.
pub fn group_row(at: &Place, group: Group, count: usize, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let (open, path) = (at.open, at.path.clone());

    let plus = (group == Group::Tables).then(|| {
        let actions = cx.catalog.clone();
        TooltipContainer::new(Tooltip::new_text("New table"))
            .position(AttachedPosition::Bottom)
            .child(
                Button::new()
                    .flat()
                    .theme_layout(
                        ButtonLayoutThemePartial::default()
                            .width(Size::px(ROW_ACTION))
                            .height(Size::px(ROW_ACTION))
                            .padding(Gaps::new_all(0.)),
                    )
                    .on_press(move |e: Event<PressEventData>| {
                        e.stop_propagation();
                        actions.configure(ConfigureTarget::New);
                    })
                    .child(
                        Icon::new(IconName::Plus)
                            .size(13.)
                            .color(cx.theme.label_color),
                    ),
            )
            .into_element()
    });

    body(
        Row::new(at.depth, cx.theme.clone())
            .disclosure(at.disclosure())
            .on_press({
                let path = path.clone();
                move |_: Event<PressEventData>| tree.toggle(&path, open)
            })
            .on_toggle(move |_: Event<PressEventData>| tree.toggle(&path, open))
            .child(
                Eyebrow::new(format!("{} · {count}", group.label()))
                    .color(cx.theme.label_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .map(plus, Row::trailing),
    )
}

/// What an empty QUERIES group says.
///
/// An ordinary [`Row`], because the tree's rows are one height — which is what lets the list be
/// virtualized at all — and a row's own indent already puts the note where the rows it stands in
/// for would be.
pub fn no_queries_row(at: &Place, cx: &RowCtx) -> RowBody {
    body(
        Row::new(at.depth, cx.theme.clone()).child(
            Caption::new("No saved queries yet")
                .color(cx.theme.meta_color)
                .width(Size::flex(1.))
                .text_overflow(TextOverflow::Ellipsis),
        ),
    )
}
