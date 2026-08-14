//! A **database connection**'s branch of the tree: the connection, its enabled schemas, and the
//! Tables / Views groups inside each.
//!
//! Nothing here is a def. A bucket cannot say what its tables are, so an object store's contents
//! are declared; a database answers for itself, so its contents are *discovered* — which is why
//! this whole subtree comes from one call,
//! [`Engine::db_listing`](strata_core::engine::Engine::db_listing), and why there is no `Reg` row
//! under the connection's own. That call is free (it reads the connect-time enumeration held
//! beside the pool, not the network) and **already scoped and tagged**, so the tree, the schemas
//! picker and completion all read one answer and none of them re-derives visibility from the
//! def. Collapsing and re-opening a schema therefore costs nothing, and ↻ — which re-connects —
//! is the refresh.
//!
//! A relation is a **leaf here**: its columns are an introspection through the cached provider,
//! and the surface that reads them is the inspector (DB-07), which is also what will make a
//! column of one selectable.

use freya::components::Disclosure;
use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::engine::db::{Relation, SchemaListingView, SchemaVisibility};
use strata_model::ConnectionDef;

use super::menu::{connection_menu, use_connection_actions};
use super::row::{actions_button, fold_plan, name_width, use_status, Row, StatusMark};
use super::{matches, CatalogTheme, TreeCtx};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{ProjChan, ProjectState};
use crate::components::badge::Badge;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_3, STATUS_DOT};
use crate::components::typography::{Eyebrow, MonoValue};

/// What a schema that the def enables and the server does not have says on hover — the one
/// diagnosis the tree makes on its own, because nothing on our side observes a server-side drop
/// or rename.
///
/// "Not in the connection" means "not in what it last told us": the relation list is the
/// connect-time enumeration, so the fix is a ↻ (which re-connects) or an edit to the
/// connection's schemas.
fn missing_schema(name: &str) -> String {
    format!(
        "'{name}' is not in this connection. Refresh the catalog if it has since been created, \
         or remove it from the connection's schemas."
    )
}

/// One database connection.
///
/// Its **catalog name comes from the def**, through the store's own reader, not off the listing:
/// the listing is only fetched once the node is open, so taking it from there made a collapsed
/// row print its provider badge's word twice and swap it for the catalog name on the first press.
#[derive(PartialEq)]
pub struct DatabaseNode {
    def: ConnectionDef,
    /// The filter, **already lowercased** — see `matches`.
    needle: String,
    theme: CatalogTheme,
    key: DiffKey,
}

impl DatabaseNode {
    pub fn new(def: ConnectionDef, needle: String, theme: CatalogTheme) -> Self {
        Self {
            def,
            needle,
            theme,
            key: DiffKey::None,
        }
    }
}

impl KeyExt for DatabaseNode {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for DatabaseNode {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
        let engine = use_consume::<EngineCtx>();
        let radio = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
        let actions = use_connection_actions();

        let mut measured = use_state(|| f32::INFINITY);

        let url = self.def.url();
        let provider = self.def.provider.id();
        let path = format!("conn/{url}");
        let (waiting, problem) = radio.read().connection_problem(&url);
        let connected = !waiting && problem.is_none();
        let status = use_status(waiting, problem);

        let name = self.def.address.clone();
        let catalog = radio.read().database_catalog(&url).unwrap_or_default();
        let (address_width, catalog_width) = (name_width(&name), name_width(&catalog));

        let stored_open = tree.is_open(&path);
        let filtering = !self.needle.is_empty();
        let listing = (connected && (stored_open || filtering))
            .then(|| engine.db_listing(&self.def))
            .flatten();
        let schemas: Vec<SchemaListingView> = listing
            .map(|(_, schemas)| schemas)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.visibility != SchemaVisibility::NotEnabled)
            .collect();

        let under: Vec<SchemaListingView> = match filtering {
            false => schemas,
            true => schemas
                .into_iter()
                .filter(|s| schema_survives(s, &self.needle))
                .collect(),
        };
        let named = matches(&name, &self.needle) || matches(&catalog, &self.needle);
        if filtering && !named && under.is_empty() {
            return rect();
        }

        let open = tree.shows(&path, filtering && !under.is_empty());
        let folds = fold_plan(measured(), address_width, true, catalog_width + SP_3);
        let build_menu = move || connection_menu(&actions, url.clone(), provider);
        let menu_for_row = build_menu.clone();
        let toggle = {
            let path = path.clone();
            move |_: Event<PressEventData>| tree.toggle(&path, open)
        };

        let row = Row::new(0, self.theme.clone())
            .disclosure(match connected {
                true => Disclosure::from_expanded(open),
                false => Disclosure::Leaf,
            })
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
                Badge::tag(self.def.provider.to_string(), self.theme.provider_color)
                    .outlined()
                    .height(16.)
                    .into_element()
            }))
            .child(
                MonoValue::new(name)
                    .color(self.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(folds.mark.then(|| {
                MonoValue::new(catalog)
                    .color(self.theme.meta_color)
                    .into_element()
            }))
            .maybe_child(folds.status.then(|| {
                rect()
                    .width(Size::px(STATUS_DOT))
                    .cross_align(Alignment::Center)
                    .maybe_child(status.as_ref().map(|s| s.glyph(&self.theme)))
                    .into_element()
            }));

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
            .maybe(open, |el| {
                el.children(under.into_iter().map(|schema| {
                    let key = format!("{path}/{}", schema.name);
                    SchemaNode {
                        path: key.clone(),
                        schema,
                        needle: self.needle.clone(),
                        theme: self.theme.clone(),
                        key: DiffKey::None,
                    }
                    .key(key)
                }))
            })
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// Does this schema, or anything in it, survive `filter`?
fn schema_survives(schema: &SchemaListingView, filter: &str) -> bool {
    filter.is_empty()
        || matches(&schema.name, filter)
        || schema.relations.iter().any(|r| matches(&r.name, filter))
}

/// One schema of a database connection — the def's enabled set, tagged against what the server
/// answered.
#[derive(PartialEq)]
struct SchemaNode {
    path: String,
    schema: SchemaListingView,
    /// The filter, **already lowercased** — see `matches`.
    needle: String,
    theme: CatalogTheme,
    key: DiffKey,
}

impl KeyExt for SchemaNode {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for SchemaNode {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
        let matched = !self.needle.is_empty()
            && self
                .schema
                .relations
                .iter()
                .any(|r| matches(&r.name, &self.needle));
        let open = tree.is_open(&self.path) || matched;
        let missing = self.schema.visibility == SchemaVisibility::EnabledButMissing;
        let path = self.path.clone();

        let row = Row::new(1, self.theme.clone())
            .disclosure(match missing {
                true => Disclosure::Leaf,
                false => Disclosure::from_expanded(open),
            })
            .map((!missing).then(|| path.clone()), |row, path| {
                row.on_press(move |_: Event<PressEventData>| tree.toggle(&path, open))
            })
            .map((!missing).then_some(path), |row, path| {
                row.on_toggle(move |_: Event<PressEventData>| tree.toggle(&path, open))
            })
            .child(
                Icon::new(IconName::Folder)
                    .color(self.theme.chevron_color)
                    .size(13.),
            )
            .child(
                MonoValue::new(self.schema.name.clone())
                    .color(self.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(missing.then(|| {
                StatusMark::Problem(missing_schema(&self.schema.name)).glyph(&self.theme)
            }));

        let groups: Vec<Element> = match !missing && open {
            false => Vec::new(),
            true => [false, true]
                .into_iter()
                .map(|views| {
                    let key = format!("{}/{}", self.path, if views { "views" } else { "tables" });
                    RelGroupNode {
                        path: key.clone(),
                        relations: self
                            .schema
                            .relations
                            .iter()
                            .filter(|r| r.is_view() == views)
                            .filter(|r| matches(&r.name, &self.needle))
                            .cloned()
                            .collect(),
                        views,
                        filtering: !self.needle.is_empty(),
                        theme: self.theme.clone(),
                        key: DiffKey::None,
                    }
                    .key(key)
                    .into_element()
                })
                .collect(),
        };

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
            .children(groups)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The Tables / Views split inside a schema, from the listing's own `relkind`.
#[derive(PartialEq)]
struct RelGroupNode {
    path: String,
    relations: Vec<Relation>,
    views: bool,
    /// Whether a filter is narrowing the tree — a group whose relations are all matches opens
    /// itself, because a node kept *by* a descendant match that then hides the match is worse
    /// than not keeping it at all.
    filtering: bool,
    theme: CatalogTheme,
    key: DiffKey,
}

impl KeyExt for RelGroupNode {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for RelGroupNode {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
        let open = tree.shows(&self.path, self.filtering && !self.relations.is_empty());
        let path = self.path.clone();
        let label = if self.views { "VIEWS" } else { "TABLES" };

        let row = Row::new(2, self.theme.clone())
            .disclosure(match self.relations.is_empty() {
                true => Disclosure::Leaf,
                false => Disclosure::from_expanded(open),
            })
            .on_press({
                let path = path.clone();
                move |_: Event<PressEventData>| tree.toggle(&path, open)
            })
            .on_toggle(move |_| tree.toggle(&path, open))
            .child(
                Eyebrow::new(format!("{label} · {}", self.relations.len()))
                    .color(self.theme.label_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            );

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
            .maybe(open, |el| {
                el.children(self.relations.iter().map(|relation| {
                    RelationRow {
                        name: relation.name.clone(),
                        view: relation.is_view(),
                        theme: self.theme.clone(),
                        key: DiffKey::None,
                    }
                    .key(relation.name.clone())
                }))
            })
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// One relation inside a schema.
///
/// A leaf, and the disclosure it does not draw is the honest mark of where DB-05 stops: a
/// relation's columns are a round trip through the cached provider, and the surface that reads
/// them — and that can address one — is the inspector DB-07 builds.
#[derive(PartialEq)]
struct RelationRow {
    name: String,
    view: bool,
    theme: CatalogTheme,
    key: DiffKey,
}

impl KeyExt for RelationRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for RelationRow {
    fn render(&self) -> impl IntoElement {
        let (icon, color) = match self.view {
            true => (IconName::Eye, self.theme.view_color),
            false => (IconName::Database, self.theme.table_color),
        };

        Row::new(3, self.theme.clone())
            .child(Icon::new(icon).color(color).size(14.))
            .child(
                MonoValue::new(self.name.clone())
                    .color(self.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
