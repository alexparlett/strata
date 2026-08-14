//! The **workspace** node and its three groups — the project's own database.
//!
//! Labelled with the project's name rather than "workspace", because that is what it is: the
//! catalog Strata's federating engine defines, addressed as `strata`, holding everything the
//! project declares. Its children are the flat pane's TABLES · VIEWS · QUERIES sections
//! verbatim — same rows, same `Reg` status slots, same menus, same columns expansion — one level
//! further in.

use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station};
use strata_model::{CatalogKind, WORKSPACE_CATALOG};
use uuid::Uuid;

use super::entry::{EntryRow, SavedQueryRow};
use super::menu::use_catalog_actions;
use super::row::{Row, INDENT};
use super::{matches, CatalogTheme, TreeCtx};
use crate::apps::configure::ConfigureTarget;
use crate::apps::project::state::{ProjChan, ProjectState};
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::ROW_ACTION;
use crate::components::typography::{Body, Caption, Eyebrow, MonoValue};

/// The workspace node's own path — the root of every path under it.
pub const WORKSPACE: &str = "ws";

/// Where the QUERIES group's empty note starts: level-in from the group, so it lines up with the
/// rows it stands in for rather than with the header above them.
const NOTE_INSET: f32 = 3. * INDENT;

/// One of the workspace's three groups.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Tables,
    Views,
    Queries,
}

impl Group {
    const ALL: [Group; 3] = [Group::Tables, Group::Views, Group::Queries];

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

    /// The store channel this group's contents live on — the whole reason the tree is nested
    /// components: a table registration landing must not wake the views or the saved queries.
    fn channel(self) -> ProjChan {
        match self {
            Group::Tables => ProjChan::Tables,
            Group::Views => ProjChan::Views,
            Group::Queries => ProjChan::Queries,
        }
    }
}

/// The node paths a fresh pane opens on — the workspace and its three groups, so a project opens
/// on its own catalog exactly as the flat pane did.
pub fn seeded_paths() -> Vec<String> {
    let mut paths = vec![WORKSPACE.to_string()];
    paths.extend(Group::ALL.iter().map(|g| g.path().to_string()));
    paths
}

/// The project's own database.
#[derive(PartialEq)]
pub struct WorkspaceNode {
    /// The filter, **already lowercased** — see `matches`.
    needle: String,
    theme: CatalogTheme,
}

impl WorkspaceNode {
    pub fn new(needle: String, theme: CatalogTheme) -> Self {
        Self { needle, theme }
    }
}

impl Component for WorkspaceNode {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
        let name = use_radio_station::<ProjectState, ProjChan>()
            .peek()
            .name
            .clone();
        // The workspace holds the rows a filter is mostly for, so it opens on a match like any
        // other container; its groups then narrow themselves.
        let open = tree.shows(WORKSPACE, !self.needle.is_empty());

        let row = Row::new(0, self.theme.clone())
            .expanded(open)
            .on_press(move |_| tree.toggle(WORKSPACE, open))
            .on_toggle(move |_| tree.toggle(WORKSPACE, open))
            .child(
                Icon::new(IconName::Database)
                    .color(self.theme.provider_color)
                    .size(14.),
            )
            .child(
                Body::new(name)
                    .color(self.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .child(MonoValue::new(WORKSPACE_CATALOG).color(self.theme.meta_color));

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
            .maybe(open, |el| {
                el.children(Group::ALL.into_iter().map(|group| {
                    GroupNode {
                        group,
                        needle: self.needle.clone(),
                        theme: self.theme.clone(),
                        key: DiffKey::None,
                    }
                    .key(group.path())
                }))
            })
    }
}

/// One of the workspace's three groups — `▾ TABLES · 4` over its rows.
///
/// **A group is structural, so it stays while a filter is typed** and its count follows the
/// filter. That is the one exception to the tree's "keep a node only if it or a descendant
/// matches" rule, and it is deliberate: the count going to `0` is what tells the user the filter
/// found nothing here, which an absent row cannot say.
#[derive(PartialEq)]
struct GroupNode {
    group: Group,
    /// The filter, **already lowercased** — see `matches`.
    needle: String,
    theme: CatalogTheme,
    key: DiffKey,
}

impl KeyExt for GroupNode {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for GroupNode {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
        let radio = use_radio::<ProjectState, ProjChan>(self.group.channel());
        let actions = use_catalog_actions();
        let path = self.group.path();

        let rows = {
            let p = radio.read();
            match self.group {
                Group::Tables => Rows::Entries(
                    CatalogKind::Table,
                    p.tables
                        .iter()
                        .map(|t| t.def.name.clone())
                        .filter(|n| matches(n, &self.needle))
                        .collect(),
                ),
                Group::Views => Rows::Entries(
                    CatalogKind::View,
                    p.views
                        .iter()
                        .map(|v| v.def.name.clone())
                        .filter(|n| matches(n, &self.needle))
                        .collect(),
                ),
                Group::Queries => Rows::Queries(
                    p.saved_queries
                        .iter()
                        .map(|q| (q.id, q.name.clone()))
                        .filter(|(_, n)| matches(n, &self.needle))
                        .collect(),
                ),
            }
        };
        let total = rows.len();
        let open = tree.shows(path, !self.needle.is_empty() && total > 0);

        let plus = (self.group == Group::Tables).then(|| {
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
                                .color(self.theme.label_color),
                        ),
                )
                .into_element()
        });

        let row = Row::new(1, self.theme.clone())
            .expanded(open)
            .on_press(move |_| tree.toggle(path, open))
            .on_toggle(move |_| tree.toggle(path, open))
            .child(
                Eyebrow::new(format!("{} · {total}", self.group.label()))
                    .color(self.theme.label_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe(plus.is_some(), |el| el.trailing(plus.clone().unwrap()));

        let empty_note = (self.group == Group::Queries && total == 0 && self.needle.is_empty())
            .then(|| {
                rect()
                    .padding(Gaps::new(0., 0., 0., NOTE_INSET))
                    .child(Caption::new("No saved queries yet").color(self.theme.meta_color))
            });

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
            .maybe(open, |el| {
                el.children(rows.into_elements(self.group, &self.theme))
                    .maybe_child(empty_note)
            })
    }
}

/// What a group holds — entries addressed by name, or saved queries addressed by `Uuid`.
enum Rows {
    Entries(CatalogKind, Vec<String>),
    Queries(Vec<(Uuid, String)>),
}

impl Rows {
    fn len(&self) -> usize {
        match self {
            Rows::Entries(_, names) => names.len(),
            Rows::Queries(queries) => queries.len(),
        }
    }

    fn into_elements(self, group: Group, theme: &CatalogTheme) -> Vec<Element> {
        match self {
            Rows::Entries(kind, names) => names
                .into_iter()
                .map(|name| {
                    let key = name.clone();
                    EntryRow::new(kind, name, group.path(), theme.clone())
                        .key(key)
                        .into_element()
                })
                .collect(),
            Rows::Queries(queries) => queries
                .into_iter()
                .map(|(id, name)| SavedQueryRow::new(id, name, theme.clone()).into_element())
                .collect(),
        }
    }
}

/// The node path of a workspace def's row — so the object-store link rows can address one
/// without restating how a path is spelled.
pub fn entry_path(kind: CatalogKind, name: &str) -> String {
    format!("{}/{name}", Group::of(kind).path())
}

/// The nodes a jump has to open before [`entry_path`] can be seen.
pub fn entry_ancestors(kind: CatalogKind) -> Vec<String> {
    vec![WORKSPACE.to_string(), Group::of(kind).path().to_string()]
}
