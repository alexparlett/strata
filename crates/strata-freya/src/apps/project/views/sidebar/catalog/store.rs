//! An **object-store connection**'s branch of the tree, and the pane's one empty state.
//!
//! A bucket cannot say what its tables are — that is the whole difference from a database — so
//! this node has nothing to discover. What it has is the workspace defs that *name* it
//! ([`ProjectState::tables_over`]), and those already have rows of their own under the workspace.
//! So its children are **links**: pressing one opens the def's ancestors and brings its row into
//! view. Deliberately not a second editable copy of the row — one def, one row, one menu.

use freya::components::Disclosure;
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::{CatalogKind, ConnectionDef};

use super::menu::{connection_menu, use_connection_actions};
use super::row::{actions_button, fold_plan, name_width, tip, use_status, Row};
use super::workspace::{entry_ancestors, entry_path};
use super::{matches, CatalogTheme, TreeCtx};
use crate::apps::connection::ConnectionTarget;
use crate::apps::project::state::{ProjChan, ProjectState};
use crate::apps::project::views::ConnectionRequest;
use crate::components::badge::Badge;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::STATUS_DOT;
use crate::components::typography::{Body, MonoValue};

/// What a link row's trailing chevron says, on hover and to a screen reader — the trailing
/// position is the standard "this navigates" mark, and the leading disclosure slot is empty on a
/// leaf, so the two cannot be read for each other.
const JUMP: &str = "Show in the workspace";

/// One object-store connection.
///
/// **Every hook runs before the filter's early return.** A node the filter narrows away is a
/// scope that still exists, so a first render that stopped short would allocate fewer hooks than
/// its next one asks for, and Freya hard-fails on that rather than guessing.
#[derive(PartialEq)]
pub struct StoreNode {
    def: ConnectionDef,
    /// The filter, **already lowercased** — see `matches`.
    needle: String,
    theme: CatalogTheme,
    key: DiffKey,
}

impl StoreNode {
    pub fn new(def: ConnectionDef, needle: String, theme: CatalogTheme) -> Self {
        Self {
            def,
            needle,
            theme,
            key: DiffKey::None,
        }
    }
}

impl KeyExt for StoreNode {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for StoreNode {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
        let radio = use_radio::<ProjectState, ProjChan>(ProjChan::Connections);
        let tables_radio = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let actions = use_connection_actions();
        let mut measured = use_state(|| f32::INFINITY);

        let url = self.def.url();
        let provider = self.def.provider.id();
        let path = format!("conn/{url}");
        let (waiting, problem) = radio.read().connection_problem(&url);
        let status = use_status(waiting, problem);
        let address_width = name_width(&self.def.address);

        let links: Vec<String> = tables_radio
            .read()
            .tables_over(&url)
            .into_iter()
            .filter(|name| matches(name, &self.needle))
            .collect();
        let filtering = !self.needle.is_empty();
        if filtering && !matches(&self.def.address, &self.needle) && links.is_empty() {
            return rect();
        }

        let open = tree.shows(&path, filtering && !links.is_empty());
        let folds = fold_plan(measured(), address_width, true, 0.);

        let build_menu = move || connection_menu(&actions, url.clone(), provider);
        let menu_for_row = build_menu.clone();
        let toggle = move |_: Event<PressEventData>| tree.toggle(&path, open);

        let row = Row::new(0, self.theme.clone())
            .disclosure(match links.is_empty() {
                true => Disclosure::Leaf,
                false => Disclosure::from_expanded(open),
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
                MonoValue::new(self.def.address.clone())
                    .color(self.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
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
                el.children(links.into_iter().map(|name| {
                    LinkRow {
                        name: name.clone(),
                        theme: self.theme.clone(),
                        key: DiffKey::None,
                    }
                    .key(name)
                }))
            })
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// A workspace table read through this connection, as a **link**.
///
/// It carries a jump affordance and no menu: the def's own row is the thing with a menu, and
/// pressing this is how you get there.
#[derive(PartialEq)]
struct LinkRow {
    name: String,
    theme: CatalogTheme,
    key: DiffKey,
}

impl KeyExt for LinkRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for LinkRow {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
        let name = self.name.clone();

        Row::new(1, self.theme.clone())
            .on_press(move |_| {
                tree.reveal(
                    &entry_ancestors(CatalogKind::Table),
                    entry_path(CatalogKind::Table, &name),
                );
            })
            .child(
                Icon::new(IconName::Database)
                    .color(self.theme.table_color)
                    .size(14.),
            )
            .child(
                MonoValue::new(self.name.clone())
                    .color(self.theme.column_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .child(
                tip(JUMP).child(
                    rect().a11y_alt(JUMP).child(
                        Icon::new(IconName::ChevronRight)
                            .color(self.theme.chevron_color)
                            .size(12.),
                    ),
                ),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The pane's empty state: a project with no connections at all.
///
/// One row rather than the Connections pane's illustrated tile, because it now sits *under* a
/// workspace node full of rows rather than filling a pane of its own. It says what a connection
/// is for and adds one; the header's `+` is the same gesture, and the command palette's *New
/// connection…* is the third.
#[derive(PartialEq)]
pub struct AddConnectionRow {
    theme: CatalogTheme,
}

impl AddConnectionRow {
    pub fn new(theme: CatalogTheme) -> Self {
        Self { theme }
    }
}

impl Component for AddConnectionRow {
    fn render(&self) -> impl IntoElement {
        let editor = use_consume::<ConnectionRequest>();

        Row::new(0, self.theme.clone())
            .on_press(move |_| {
                let mut editor = editor;
                editor.set(Some(ConnectionTarget::New));
            })
            .child(
                Icon::new(IconName::Plus)
                    .color(self.theme.meta_color)
                    .size(13.),
            )
            .child(
                Body::new("Add a connection")
                    .color(self.theme.meta_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
    }
}
