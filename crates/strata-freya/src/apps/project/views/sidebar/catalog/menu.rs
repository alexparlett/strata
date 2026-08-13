//! The catalog rows' **context menus** (P3-06) — one [`Menu`] per row kind, opened either by
//! right-clicking the row or by pressing its ⋮ button. One item list per kind, built here, so the
//! two triggers can't drift apart (the Dioxus sidebar had exactly this pair, sharing one
//! `catalog_menu_items`).
//!
//! ## The actions are direct calls, not cache invalidations
//!
//! **The store is the catalog** (P3-02): there is no `FetchCatalog` query to invalidate. Every
//! item here calls the engine and/or mutates [`ProjectState`] on the matching [`ProjChan`], and
//! the rows subscribed to that channel re-render. Nothing refetches.
//!
//! ## Drop opens the confirm; it does not drop
//!
//! P3-05 landed the whole drop flow — the dialog, its "N views will be left invalid" consequence
//! line, and the drop itself (store + persist + engine + tab unbinding). The item here sets the
//! [`DropTarget`] slot that dialog watches, and that is *all* it does. There is deliberately no
//! second drop path.
//!
//! ## Profile asks the same question the inspector's card does
//!
//! `Profile table` / `Profile view` route through [`ProfileActions::ask`] — the *one* entry point
//! the inspector's scan card also uses — so a first scan raises the cost confirm (P3-10) and a
//! re-scan doesn't. The item leaves a request on the row and nothing else: the scan itself is the
//! freya-query entry that request keys, and the row's own spinner is what says it is running.
//!
//! ## A menu is a snapshot
//!
//! These builders run inside an event handler, which has no reactive context — every read is a
//! `peek`. That is the same trade the tab strip's menu makes: a transient menu's contents are
//! whatever was true when it opened, and acting on it dismisses it. The rows themselves stay
//! live (a row re-answering swaps its own status glyph while the menu is up); only the labels in
//! the open card are frozen.

use freya::components::MenuItemThemePartial;
use freya::prelude::*;
use freya::radio::{use_radio_station, RadioStation};
use strata_model::{CatalogKind, Origin, SavedQuery};
use uuid::Uuid;

use crate::apps::configure::ConfigureTarget;
use crate::apps::project::state::{
    persisted_defs, refresh_table, use_catalog, use_catalog_rescan, use_report, Anchor, Catalog,
    CatalogRescan, Chan, ChatsCtx, ProjChan, ProjectState, Reg, ReportCtx, SessionState,
};
use crate::apps::project::views::{
    ask_about, use_profile_actions, ConfigureRequest, DropTarget, ProfileActions, ProfileTarget,
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

// ---- the menus -------------------------------------------------------------------------------

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
    // Snapshotted at open (see the module doc). `loading` is this row's own state, which is what
    // makes "Refreshing…" mean *this* table rather than "some pass is running": the row's status
    // glyph says the same thing from the other side.
    let scanning = actions.catalog.peek().is_scanning();
    // The origin travels with the drop gesture (`DropTarget::Table`) as well as gating Configure:
    // it is what decides whether the confirm says the data goes, and this is the last place the
    // def is in hand.
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
                    ProfileTarget::verb(CatalogKind::Table),
                    move |a| a.profile.ask(CatalogKind::Table, &name),
                )
                // There is nothing to scan until the engine has answered for this row: a table it
                // refused has no provider, so a scan of it can only fail — and it would fail out
                // of sight, since the inspector has no column of a failed row to show it on.
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
        // A view has no footer facts of its own, so a scan is the only way it learns anything —
        // worth more here than on a table. Offered only once the view has actually planned: a view
        // whose SQL didn't plan has nothing to scan (see `registered`).
        .child({
            let name = name.clone();
            actions
                .item(
                    IconName::Chart,
                    ProfileTarget::verb(CatalogKind::View),
                    move |a| a.profile.ask(CatalogKind::View, &name),
                )
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
/// `renaming` is the row's own inline-rename flag; the item just flips it on and the row reacts
/// in its own scope (seeds the draft, focuses the input, commits), so it survives this menu
/// closing — the tab strip's rename works the same way.
pub fn query_menu(
    actions: &CatalogActions,
    id: Uuid,
    name: String,
    mut renaming: State<bool>,
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
        // The pencil is Strata's "edit the name or the definition", which is what makes it the
        // right glyph here and on a view's Edit query — the canvas spends it on Open in new tab,
        // which it can afford only because it has no Rename.
        .child(
            MenuButton::new()
                .on_press(move |_| {
                    renaming.set(true);
                    ContextMenu::close();
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

// ---- the actions -----------------------------------------------------------------------------

/// **View table / View view** — put `SELECT * FROM <row>` in a tab, ready to run but not run:
/// the row was clicked to look at the data, and pressing Run is the user's call (a full-width
/// scan of a big table shouldn't start itself).
///
/// The `LIMIT` is the row-limit setting, as in the Dioxus app; `0` means no limit, so the clause
/// is dropped rather than written as `LIMIT 0`.
///
/// `pub` because the command palette's TABLES and VIEWS rows are the same gesture as this menu
/// item — that is what makes the two agree on the generated SQL and its `LIMIT` rather than
/// happening to.
pub fn view_row(actions: &CatalogActions, name: &str) {
    let limit = actions.config.peek().settings.row_limit;
    let sql = if limit > 0 {
        format!("SELECT *\nFROM {name}\nLIMIT {limit};")
    } else {
        format!("SELECT *\nFROM {name};")
    };
    let mut session = actions.session;
    session
        .write_channel(Chan::Tabs)
        .open_or_focus(name, sql, Origin::Scratch);
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
