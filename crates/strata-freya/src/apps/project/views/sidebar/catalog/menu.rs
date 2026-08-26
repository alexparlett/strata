//! The tree rows' **context menus** (P3-06 · W7 · DB-05) — one [`Menu`] per row kind, opened
//! either by right-clicking the row or by pressing its ⋮ button. One item list per kind, built
//! here, so the two triggers can't drift apart (the Dioxus sidebar had exactly this pair, sharing
//! one `catalog_menu_items`).
//!
//! **The actions are direct calls, not cache invalidations.** The store *is* the catalog, so there
//! is no `FetchCatalog` query to invalidate: every item calls the engine and/or mutates
//! [`ProjectState`] on the matching [`ProjChan`], and the rows subscribed to it re-render.
//!
//! **Drop opens the confirm; it does not drop.** The item sets the [`DropTarget`] slot the dialog
//! watches, and that is all it does — there is deliberately no second drop path.
//!
//! **Profile asks the same question the inspector's card does**, through
//! [`ProfileActions::ask`], so a first scan raises the cost confirm and a re-scan does not. The
//! item leaves a request on the row and nothing else.
//!
//! **A menu is a snapshot.** These builders run inside an event handler, which has no reactive
//! context, so every read is a `peek` — the same trade the tab strip's menu makes. The rows
//! themselves stay live; only the labels in the open card are frozen.

use freya::components::MenuItemThemePartial;
use freya::prelude::*;
use freya::radio::{use_radio_station, RadioStation};
use strata_engine::quote_ident;
use strata_model::{CatalogKind, Origin, ProviderId, SavedQuery};
use uuid::Uuid;

use super::node::Remote;
use crate::apps::configure::ConfigureTarget;
use crate::apps::connection::ConnectionTarget;
use crate::apps::project::query::ProfileTarget;
use crate::apps::project::state::{
    persisted_defs, refresh_table, use_catalog, use_catalog_rescan, use_report, Anchor, Catalog,
    CatalogRescan, Chan, ChatsCtx, ProjChan, ProjectState, Reg, ReportCtx, SessionState,
};
use crate::apps::project::views::{
    ask_about, profile_verb, use_profile_actions, ConfigureRequest, ConnectionRequest, DropTarget,
    ProfileActions, SchemasRequest,
};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{CONTEXT_MENU_WIDTH, MENU_ICON, SP_4};
use crate::components::tones::tones;
use crate::components::typography::Prose;
use crate::state::{use_config_station, ConfigStation};

const ITEM_GAP: f32 = SP_4;

/// The handles a catalog row's menu acts through, gathered once per row.
///
/// Both stores are **stations**, not subscribing radios: a row must not re-render because a
/// keystroke landed in some tab, or because another row's registration answered. The menus only
/// ever peek and write.
#[derive(Clone)]
pub struct CatalogActions {
    pub session: RadioStation<SessionState, Chan>,
    pub project: RadioStation<ProjectState, ProjChan>,
    /// Whether a catalog pass is in flight — Refresh is a no-op while one is, so it says so
    /// rather than offering a press that does nothing.
    pub catalog: Catalog,
    /// Where Refresh puts its request. The menu item deliberately cannot spawn the pass itself:
    /// its own scope is a `MenuButton` that the same press closes, and Freya drops a scope's
    /// tasks before polling them. The window root's driver owns the pass (`state/catalog.rs`).
    pub rescan: CatalogRescan,
    /// The app-global config: "View table" takes its `LIMIT` from the row-limit setting.
    pub config: ConfigStation,
    /// The drop-confirm slot provided at the window root (P3-05). Setting it *is* the drop
    /// action.
    pub drop_target: State<Option<DropTarget>>,
    /// The profile action (P3-09): asking for a scan of this row, through the cost confirm on a
    /// first one (P3-10). The same call the inspector's scan card makes — this is the catalog
    /// side of one entry point, not a second copy of it.
    pub profile: ProfileActions,
    /// The sheet's destructive tone, resolved here because the menu itself is built from an
    /// event handler, where no hook — `use_theme` included — may run.
    pub danger: Color,
    /// The Configure-window request slot (P4-11). Setting it *is* the action — the root's
    /// `ConfigureLauncher` holds the handles a window needs, so a row does not have to.
    pub configure_target: ConfigureRequest,
    /// Where a failed write reports (P4-15) — for the one action here that writes
    /// `project.json` itself (`rename_saved_query`). Every other mutation this menu offers sets a
    /// slot for a dialog or a window that carries its own; a rename commits inline, so it
    /// reports here.
    pub report: ReportCtx,
    /// The window's conversations (AS-04) — where **Ask about this** pins its anchor. Held
    /// rather than reached for, because the menu is built inside an event handler where no hook
    /// may run.
    pub chats: ChatsCtx,
}

/// Gather this row's action handles from the window's stores + context.
pub fn use_catalog_actions() -> CatalogActions {
    CatalogActions {
        session: use_radio_station::<SessionState, Chan>(),
        project: use_radio_station::<ProjectState, ProjChan>(),
        catalog: use_catalog(),
        rescan: use_catalog_rescan(),
        config: use_config_station(),
        drop_target: use_consume::<State<Option<DropTarget>>>(),
        chats: use_consume::<ChatsCtx>(),
        profile: use_profile_actions(),
        danger: tones().error,
        configure_target: use_consume::<ConfigureRequest>(),
        report: use_report(),
    }
}

impl CatalogActions {
    /// A menu item: glyph, label, and an action run against these handles. Every item closes
    /// the menu afterwards — a press *inside* the card doesn't dismiss it on its own (only an
    /// outside press does).
    fn item(
        &self,
        icon: IconName,
        label: impl Into<String>,
        action: impl Fn(&CatalogActions) + 'static,
    ) -> MenuButton {
        let actions = self.clone();
        MenuButton::new()
            .on_press(move |_| {
                action(&actions);
                ContextMenu::close();
            })
            .child(menu_row(icon, label))
    }

    /// **Ask about this row** (AS-04): open the chat pane with the entry pinned, so the schema
    /// is attached to the next question rather than spent as a tool round.
    ///
    /// One press into the pane that already exists, through the shared `ask_about` funnel — the
    /// same one the failed-run and result entries use, so all three land the same way.
    fn ask(&self, icon: IconName, label: &'static str, anchor: Anchor) -> MenuButton {
        let (session, chats) = (self.session, self.chats);
        MenuButton::new()
            .on_press(move |_| {
                ask_about(session, chats, anchor.clone());
                ContextMenu::close();
            })
            .child(menu_row(icon, label))
    }

    /// Ask for the **Configure** window on `target` — a new table, or this row's def.
    ///
    /// Sets the slot and stops, like the drop item: the project root's `ConfigureLauncher` is
    /// the one thing that opens the window, so the row menu and the TABLES `+` cannot drift.
    pub fn configure(&self, target: ConfigureTarget) {
        let mut slot = self.configure_target;
        slot.set(Some(target));
    }

    /// Has the engine actually answered for this row? The precondition for offering a **scan**:
    /// a def the engine refused has no provider behind it, so a scan of it can only fail — and it
    /// would fail out of sight, because the inspector shows a failed row's *reason* rather than
    /// any column a scan could report on. An unanswered row is not offered one either; the answer
    /// is moments away, and by then the offer means something.
    fn registered(&self, kind: CatalogKind, name: &str) -> bool {
        let p = self.project.peek();
        match kind {
            CatalogKind::View => p
                .views
                .iter()
                .find(|v| v.def.name == name)
                .is_some_and(|v| v.reg.ready().is_some()),
            CatalogKind::Query => false,
            CatalogKind::Table => p
                .tables
                .iter()
                .find(|t| t.def.name == name)
                .is_some_and(|t| t.reg.ready().is_some()),
        }
    }

    /// The destructive item — the canvas's `--c-err` row. Colour rides the `menu_item` theme's
    /// own `color` slot, which [`MenuItem`] applies to the whole row, so the glyph and the label
    /// tint together (and the disabled fade still works on top of it).
    fn danger(
        &self,
        label: impl Into<String>,
        action: impl Fn(&CatalogActions) + 'static,
    ) -> MenuButton {
        self.item(IconName::Trash, label, action)
            .theme(MenuItemThemePartial::default().color(self.danger))
    }
}

/// One menu row: the glyph over its label, at the canvas's gap.
fn menu_row(icon: IconName, label: impl Into<String>) -> impl IntoElement {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(ITEM_GAP)
        .child(Icon::new(icon).size(MENU_ICON))
        .child(Prose::new(label))
}

/// A **table** row's menu: open it in a tab · profile it · re-scan it · configure it · drop it.
///
/// **Configure is absent on an internal table** (ED-04), not parked. It is the only item here
/// that could never apply rather than merely being unavailable right now: it edits the sources,
/// format and partition columns of a def that points at the user's own files, and a table Strata
/// wrote into `.strata/tables/` has none of that to edit, ever. That is this menu's established
/// treatment for a row kind an item cannot apply to — the view menu has no Refresh at all — while
/// parking (`enabled(false)`) means "not this second", which is what Refresh already uses while a
/// scan is in flight. Nothing is lost by its absence: the column list is on the row's own
/// expansion and Profile answers everything else about the data.
///
/// It also makes the Configure window's contract structural rather than guarded:
/// `ConfigureTarget::Edit` is set from exactly two places — this item, and Configure's own
/// post-save transition on a *New* table, which is external by construction — so with the item
/// gone the window cannot receive an internal def at all.
pub fn table_menu(actions: &CatalogActions, name: String) -> Menu {
    let scanning = actions.catalog.peek().is_scanning();
    let (loading, origin) = {
        let p = actions.project.peek();
        let row = p.tables.iter().find(|t| t.def.name == name);
        (
            matches!(row.map(|t| &t.reg), Some(Reg::Loading)),
            row.map(|t| t.def.origin).unwrap_or_default(),
        )
    };
    let internal = origin.is_internal();
    let registered = actions.registered(CatalogKind::Table, &name);

    Menu::new()
        .min_width(Size::px(CONTEXT_MENU_WIDTH))
        .child({
            let name = name.clone();
            actions.item(IconName::Play, "View table", move |a| {
                view_row(a, &name);
            })
        })
        .child({
            let name = name.clone();
            actions
                .item(
                    IconName::Chart,
                    profile_verb(CatalogKind::Table),
                    move |a| {
                        a.profile.ask(&ProfileTarget::Workspace {
                            kind: CatalogKind::Table,
                            name: name.clone(),
                        });
                    },
                )
                .enabled(registered)
        })
        .child(actions.ask(
            IconName::Chat,
            "Ask about this table",
            Anchor::Entry {
                name: name.clone(),
                kind: CatalogKind::Table,
            },
        ))
        .child(Divider::menu())
        .child({
            let name = name.clone();
            actions
                .item(
                    IconName::Reload,
                    if loading {
                        "Refreshing…"
                    } else {
                        "Refresh table"
                    },
                    move |a| refresh_table(a.rescan, name.clone()),
                )
                .enabled(!scanning)
        })
        .maybe_child((!internal).then(|| {
            let name = name.clone();
            actions
                .item(IconName::Gear, "Configure", move |a| {
                    a.configure(ConfigureTarget::Edit(name.clone()));
                })
                .into_element()
        }))
        .child(Divider::menu())
        .child(actions.danger("Drop table", move |a| {
            let mut slot = a.drop_target;
            slot.set(Some(DropTarget::Table {
                name: name.clone(),
                origin,
            }));
        }))
}

/// A **view** row's menu: open it in a tab · profile it · edit the SQL behind it · drop it.
///
/// No Refresh: a view has no files of its own to re-infer. Re-creating it is what a *table*
/// refresh does to the views over it ([`ProjectState::views_to_refresh`]), because that is when
/// its plan goes stale.
pub fn view_menu(actions: &CatalogActions, name: String) -> Menu {
    let registered = actions.registered(CatalogKind::View, &name);

    Menu::new()
        .min_width(Size::px(CONTEXT_MENU_WIDTH))
        .child({
            let name = name.clone();
            actions.item(IconName::Play, "View view", move |a| {
                view_row(a, &name);
            })
        })
        .child({
            let name = name.clone();
            actions
                .item(IconName::Chart, profile_verb(CatalogKind::View), move |a| {
                    a.profile.ask(&ProfileTarget::Workspace {
                        kind: CatalogKind::View,
                        name: name.clone(),
                    });
                })
                .enabled(registered)
        })
        .child(actions.ask(
            IconName::Chat,
            "Ask about this view",
            Anchor::Entry {
                name: name.clone(),
                kind: CatalogKind::View,
            },
        ))
        .child({
            let name = name.clone();
            actions.item(IconName::Pencil, "Edit query", move |a| {
                edit_view(a, &name);
            })
        })
        .child(Divider::menu())
        .child(actions.danger("Drop view", move |a| {
            let mut slot = a.drop_target;
            slot.set(Some(DropTarget::View(name.clone())));
        }))
}

/// A **saved query** row's menu: open it · rename it · delete it.
///
/// `renaming` names **which** saved query is being renamed and `draft` holds what has been typed;
/// the item points one at this row and seeds the other, so the rename survives both this menu
/// closing and the row scrolling out of the virtualized window. Seeding here rather than in the row
/// is what makes that true: a row that seeded its own draft re-seeded it from the stored name every
/// time it was rebuilt.
pub fn query_menu(
    actions: &CatalogActions,
    id: Uuid,
    name: String,
    mut renaming: State<Option<Uuid>>,
    mut draft: State<String>,
) -> Menu {
    Menu::new()
        .min_width(Size::px(CONTEXT_MENU_WIDTH))
        .child(actions.item(IconName::Play, "Open in new tab", move |a| {
            open_saved_query(a, id);
        }))
        .child(actions.ask(
            IconName::Chat,
            "Ask about this query",
            Anchor::SavedQuery {
                id,
                name: name.clone(),
            },
        ))
        .child(
            MenuButton::new()
                .on_press({
                    let name = name.clone();
                    move |_| {
                        draft.set(name.clone());
                        renaming.set(Some(id));
                        ContextMenu::close();
                    }
                })
                .child(menu_row(IconName::Pencil, "Rename")),
        )
        .child(Divider::menu())
        .child(actions.danger("Delete query", move |a| {
            let mut slot = a.drop_target;
            slot.set(Some(DropTarget::Query {
                id,
                name: name.clone(),
            }));
        }))
}

/// The statement every "look at this" gesture puts in a tab: a full read of `target`, where
/// `target` is a name **already rendered** for interpolation.
///
/// The `LIMIT` is the row-limit setting, as in the Dioxus app; `0` means no limit, so the clause
/// is dropped rather than written as `LIMIT 0`.
pub fn select_sql(target: &str, limit: usize) -> String {
    match limit > 0 {
        true => format!("SELECT *\nFROM {target}\nLIMIT {limit};"),
        false => format!("SELECT *\nFROM {target};"),
    }
}

/// The statement **Pin as view…** composes (DB-06) — `name` and `target` both already rendered,
/// and by *different* renderers: the view is a workspace def, so its name is the one the store
/// will key it under, while the target's spelling is a server's. See
/// [`quote_ident`](strata_engine::quote_ident).
pub fn pin_view_sql(name: &str, target: &str) -> String {
    format!("CREATE VIEW {name} AS\nSELECT *\nFROM {target};")
}

/// Put `sql` in a scratch tab titled `name`, ready to run but not run.
///
/// Nothing here runs anything, and that is the shared rule rather than a per-gesture choice: the
/// row was pressed to look at something, and a full-width scan of a big table shouldn't start
/// itself. The composing gestures (Pin as view…) are the Shape panel's precedent — a statement
/// handed over for the user to finish.
///
/// **Two gestures on one row need two names**, because [`open_or_focus`] finds a scratch tab by
/// name *and* text: with one name between them the second gesture never owns it, so its own
/// repeat press matches nothing and stacks `… 2`, `… 3`, `… 4` — the very thing that funnel
/// exists to prevent. The name is also a **label**, never a statement, so it is built from plain
/// segments: an address rendered for SQL carries quotes a tab strip should not show.
///
/// [`open_or_focus`]: crate::apps::project::state::SessionState::open_or_focus
fn open_composed(actions: &CatalogActions, name: &str, sql: String) {
    let mut session = actions.session;
    session
        .write_channel(Chan::Tabs)
        .open_or_focus(name, sql, Origin::Scratch);
}

/// **View table / View view** — a full read of a workspace row.
///
/// `pub` because the command palette's TABLES and VIEWS rows are the same gesture as this menu
/// item — that is what makes the two agree on the generated SQL and its `LIMIT` rather than
/// happening to.
pub fn view_row(actions: &CatalogActions, name: &str) {
    let limit = actions.config.peek().settings.row_limit;
    open_composed(actions, name, select_sql(&quote_ident(name), limit));
}

/// A **remote relation** row's menu: query it · profile it · pin it as a workspace view.
///
/// Three items and no more. Everything the workspace rows offer beyond these is about a **def** —
/// Configure edits one, Drop removes one, Refresh re-infers one — and a remote relation has none:
/// the database answers for itself, and the connection's own row is where its lifecycle lives.
/// Profile (DB-07) is the one that arrived later, and it arrived as an arm of the entry point every
/// other profile gesture already went through, not as a second one.
pub fn relation_menu(actions: &CatalogActions, relation: &Remote) -> Menu {
    let verb = match relation.view {
        true => "Query view",
        false => "Query table",
    };
    let kind = match relation.view {
        true => CatalogKind::View,
        false => CatalogKind::Table,
    };
    Menu::new()
        .min_width(Size::px(CONTEXT_MENU_WIDTH))
        .child({
            let relation = relation.clone();
            actions.item(IconName::Play, verb, move |a| {
                query_relation(a, &relation);
            })
        })
        .child({
            let reference = relation.reference.clone();
            actions.item(IconName::Chart, profile_verb(kind), move |a| {
                a.profile.ask(&ProfileTarget::Remote {
                    kind,
                    relation: reference.clone(),
                });
            })
        })
        .child({
            let relation = relation.clone();
            actions.item(IconName::Eye, "Pin as view…", move |a| {
                pin_relation(a, &relation);
            })
        })
}

/// **Query table / Query view** — [`view_row`] over a three-part name. Both forms were built by
/// the walk, so this composes rather than quotes: the label titles the tab, the address goes in
/// the statement.
pub fn query_relation(actions: &CatalogActions, relation: &Remote) {
    let limit = actions.config.peek().settings.row_limit;
    open_composed(
        actions,
        &relation.label,
        select_sql(&relation.address, limit),
    );
}

/// **Pin as view…** — the workstream's "make it a bare-named def" gesture: a `CREATE VIEW` over
/// the remote relation, in an **unrun** tab for the user to rename and run.
///
/// Composing rather than executing is the point. The name is a guess — the relation's own, which
/// is the only one available and frequently the wrong one in a workspace that already has a table
/// called `orders` — so the user finishes the statement, and running it lands the def through the
/// view funnel that already exists (⌘S and typed view DDL are the other two ways into it). A
/// gesture that created the view itself would have had to invent a name, or refuse.
///
/// The tab is titled with the **view being made** rather than the relation being read, which is
/// what the workspace rows already do and what keeps this gesture's tab apart from Query's on the
/// same row (see [`open_composed`]).
pub fn pin_relation(actions: &CatalogActions, relation: &Remote) {
    open_composed(
        actions,
        &relation.name,
        pin_view_sql(&quote_ident(&relation.name), &relation.address),
    );
}

/// **Edit query** — open the view's own SQL in a tab bound to it (`Origin::View`), so ⌘S
/// redefines *that view* rather than saving a new query. A tab already bound to it is focused
/// instead of opened twice (see [`SessionState::open_or_focus`]).
fn edit_view(actions: &CatalogActions, name: &str) {
    let Some(sql) = actions
        .project
        .peek()
        .views
        .iter()
        .find(|v| v.def.name == name)
        .map(|v| v.def.sql.clone())
    else {
        return;
    };
    let mut session = actions.session;
    session
        .write_channel(Chan::Tabs)
        .open_or_focus(name, sql, Origin::View(name.to_string()));
}

/// **Open in new tab** — the saved query's SQL in a tab bound to it by `id`, which is a saved
/// query's identity (so a later rename can't dangle the binding). Also what pressing the row
/// itself does, per the canvas.
pub fn open_saved_query(actions: &CatalogActions, id: Uuid) {
    let Some(SavedQuery { name, sql, .. }) = actions
        .project
        .peek()
        .saved_queries
        .iter()
        .find(|q| q.id == id)
        .cloned()
    else {
        return;
    };
    let mut session = actions.session;
    session
        .write_channel(Chan::Tabs)
        .open_or_focus(&name, sql, Origin::SavedQuery(id));
}

/// The handles a **connection** row's menu acts through, gathered once per row — this module's
/// [`CatalogActions`] shape, with only what a connection's three items need.
#[derive(Clone, Copy)]
pub struct ConnectionActions {
    /// The remove-confirm slot provided at the window root. Setting it *is* Forget: the dialog
    /// owns the store mutation, the persist, the keystore entry and the `Engine::disconnect`
    /// behind it.
    drop_target: State<Option<DropTarget>>,
    /// The editor-window request slot, on the same terms: setting it *is* Edit, and
    /// `ConnectionLauncher` at the project root opens the window.
    editor: ConnectionRequest,
    /// The schemas-picker slot, likewise — the picker is a dialog at the window root.
    schemas: SchemasRequest,
    /// The destructive tone, resolved here because a menu is built from an event handler, where
    /// no hook — `use_theme` included — may run.
    danger: Color,
}

/// Gather a connection row's action handles.
pub fn use_connection_actions() -> ConnectionActions {
    ConnectionActions {
        drop_target: use_consume::<State<Option<DropTarget>>>(),
        editor: use_consume::<ConnectionRequest>(),
        schemas: use_consume::<SchemasRequest>(),
        danger: tones().error,
    }
}

/// A **connection** row's menu: edit it · pick its schemas · forget it.
///
/// Every item **sets a slot and stops** — the editor window, the schemas picker and the remove
/// confirm are all the project root's, and a menu built inside an event handler can run no hook
/// to reach what any of them needs.
///
/// *Schemas…* is absent on an object store rather than parked, for [`table_menu`]'s reason: a
/// bucket has no schemas to scope, ever, where parking means "not this second".
pub fn connection_menu(actions: &ConnectionActions, name: String, provider: ProviderId) -> Menu {
    let actions = *actions;
    Menu::new()
        .min_width(Size::px(CONTEXT_MENU_WIDTH))
        .child(
            MenuButton::new()
                .on_press({
                    let name = name.clone();
                    move |_| {
                        let mut slot = actions.editor;
                        slot.set(Some(ConnectionTarget::Edit(name.clone())));
                        ContextMenu::close();
                    }
                })
                .child(menu_row(IconName::Pencil, "Edit connection")),
        )
        .maybe_child((provider == ProviderId::Source).then(|| {
            let name = name.clone();
            MenuButton::new()
                .on_press(move |_| {
                    let mut slot = actions.schemas;
                    slot.set(Some(name.clone()));
                    ContextMenu::close();
                })
                .child(menu_row(IconName::Folder, "Schemas…"))
                .into_element()
        }))
        .child(Divider::menu())
        .child(
            MenuButton::new()
                .theme(MenuItemThemePartial::default().color(actions.danger))
                .on_press(move |_| {
                    let mut slot = actions.drop_target;
                    slot.set(Some(DropTarget::Connection {
                        name: name.clone(),
                        provider,
                    }));
                    ContextMenu::close();
                })
                .child(menu_row(IconName::Trash, "Forget connection")),
        )
}

/// Commit a saved-query rename: relabel the row and persist the defs, since a def mutation
/// persists at the mutation point (like save-as-view and the drop).
///
/// The write goes through the same funnel every other def mutation uses (P4-15). It used to
/// hold `persisted_defs`'s body **minus the reporting line** — the rename was written before the
/// funnel existed and never switched to it — so a rename that failed to reach disk showed the new
/// name in the sidebar all session and came back under the old one at the next open, with nothing
/// said. Nothing to gate here: the row is already relabelled and there is no success event to
/// withhold, so the answer is deliberately dropped.
pub fn rename_saved_query(actions: &CatalogActions, id: Uuid, name: &str) {
    let mut project = actions.project;
    let mut p = project.write_channel(ProjChan::Queries);
    p.rename_saved_query(id, name);
    persisted_defs(&p, actions.report);
}
