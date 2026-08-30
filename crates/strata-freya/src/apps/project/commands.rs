//! The window's **command registry** (P6-01) — what the command palette's ACTIONS group offers,
//! and what running one does.
//!
//! **One declaration site, and no second implementation.** Each command is a method carrying its
//! own metadata in rmcp's shape: the id is the method's name, the subtext its doc comment, and
//! `strata_command_macro` turns the block into the [`Action`] enum, the [`ROUTES`] slice and a
//! total `Action::run`. A command that renders but does nothing is not expressible.
//!
//! **Every body here is one call into a funnel that already exists** — a palette row is a second
//! way to ask for something, never a second implementation. Where a piece of that logic was inline
//! somewhere the palette cannot reach, it *moved* to the funnel rather than being copied.
//!
//! **The palette is not a function of the keymap.** [`key`](CommandRoute::key) names the chord a
//! command also answers to and is used only to render the row's hint. Synthesizing the chord — the
//! trick `menu.rs` uses, because a muda handler has no stores — would make a command the user
//! unbound unreachable from the one surface that exists so you need not know the chord.
//!
//! **Deliberately absent:** *Check for updates* was built and removed, because the updater already
//! has two surfaces and a third to keep in step buys a gesture nobody reaches for by name. And the
//! canvas's *Export results…* is not built — an
//! [`ExportLaunch`](crate::apps::export::ExportLaunch) is assembled from the results pane's live
//! sort and the page in hand, so this registry can neither build one nor tell whether there is
//! anything to export.

use std::sync::Arc;

use freya::prelude::*;
use freya::radio::{use_radio, Radio};
use strata_command_macro::command_router;
use strata_core::config::Command;
use strata_model::{DrawerTab, SidebarPane, TabId};

use crate::apps::configure::ConfigureTarget;
use crate::apps::project::close::{close_project, CloseGuard, CloseTarget};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{
    use_catalog_selection, use_settle, CatalogSelection, Chan, SessionState, Settle,
};
use crate::apps::project::views::{actions, use_catalog_actions, CatalogActions, SourceRequest};
use crate::apps::source::SourceTarget;
use crate::components::icon::IconName;
use crate::platform::{open_settings, OpenCtx};
use crate::state::AppCtx;

/// One command: its metadata beside the function that performs it.
///
/// The field set is `strata_command_macro`'s contract — the macro names these fields and nothing
/// about their types, which is what keeps the registration mechanism free of Strata's vocabulary.
pub struct CommandRoute {
    /// The method's own name — a stable id for keying a row.
    pub id: &'static str,
    pub label: &'static str,
    /// The method's doc comment: what the row says under its name.
    pub sub: &'static str,
    pub icon: IconName,
    /// The chord this command also answers to, for the row's shortcut hint. Never how it runs
    /// — see the module doc.
    pub key: Option<Command>,
    /// Words that should find it but appear in neither its label nor its subtext.
    pub keywords: &'static str,
    pub call: fn(&PaletteCtx),
}

/// The handles every command acts through, gathered once per render of the palette overlay.
///
/// It **embeds** [`CatalogActions`] rather than re-listing its handles, because a table, view or
/// saved-query row in the palette is the same gesture as its row in the catalog — that is what
/// makes the two agree by construction rather than by inspection.
///
/// Two handles onto the session store, which is not redundancy but the two funnels this feeds:
/// [`CatalogActions`] carries a station (a catalog row must never subscribe to a keystroke in
/// some tab), and the editor's `actions` take a [`Radio`]. Neither subscribes anything here —
/// a `Radio` listens only when it is `read()` inside a render, and nothing in the palette reads
/// one there.
#[derive(Clone)]
pub struct PaletteCtx {
    pub catalog: CatalogActions,
    /// The session store as the editor's `actions` take it. See the note above.
    pub session: Radio<SessionState, Chan>,
    /// Run's in-flight gate and Save-as-view's engine.
    pub engine: EngineCtx,
    /// Where Save as view folds what the engine answered.
    pub settle: Settle,
    /// Where a COLUMNS row lands: the inspected column (P3-08).
    pub selection: CatalogSelection,
    /// The data source editor's request slot. The pane's `+` folds under panel pressure and has
    /// no second entry point once there is one source, so this row is what makes that fold
    /// cost nothing.
    pub source: SourceRequest,
    /// The window's open path — Switch project… is ⌘O by another name.
    pub open: OpenCtx,
    /// The close-while-running gate's two halves, for Close project.
    pub guard: Arc<CloseGuard>,
    pub confirm: State<Option<CloseTarget>>,
    pub app: AppCtx,
    pub platform: Platform,
}

/// Gather the palette's action handles from the window's stores + context.
pub fn use_palette_ctx() -> PaletteCtx {
    PaletteCtx {
        catalog: use_catalog_actions(),
        session: use_radio::<SessionState, Chan>(Chan::Tabs),
        engine: use_consume::<EngineCtx>(),
        settle: use_settle(),
        selection: use_catalog_selection(),
        source: use_consume::<SourceRequest>(),
        open: use_consume::<OpenCtx>(),
        guard: use_consume::<Arc<CloseGuard>>(),
        confirm: use_consume::<State<Option<CloseTarget>>>(),
        app: use_consume::<AppCtx>(),
        platform: use_hook(Platform::get),
    }
}

impl PaletteCtx {
    /// The tab a command that acts on "the current query" addresses. `peek`, because a command
    /// runs from an event handler — there is no reactive context to subscribe.
    fn active_tab(&self) -> Option<TabId> {
        self.catalog.session.peek().active
    }
}

/// The palette's commands. See the module doc — this block is the whole vocabulary, and
/// [`Action`] / [`ROUTES`] are generated from it.
pub struct PaletteCommands;

#[command_router]
impl PaletteCommands {
    /// Execute current SQL
    #[command(label = "Run query", icon = IconName::Play, key = Command::RunQuery,
              keywords = "execute press")]
    fn run_query(ctx: &PaletteCtx) {
        let Some(id) = ctx.active_tab() else { return };
        actions::run_query(&ctx.engine, ctx.session, id);
    }

    /// Open a blank editor tab
    #[command(label = "New query tab", icon = IconName::Plus, key = Command::NewTab,
              keywords = "add blank scratch")]
    fn new_tab(ctx: &PaletteCtx) {
        let mut session = ctx.catalog.session;
        session.write_channel(Chan::Tabs).open_blank();
    }

    /// Persist current SQL to the catalog
    #[command(label = "Save query as view", icon = IconName::Eye,
              keywords = "create persist")]
    fn save_as_view(ctx: &PaletteCtx) {
        let Some(id) = ctx.active_tab() else { return };
        actions::save_as_view(ctx.session, ctx.engine.clone(), ctx.settle, id);
    }

    /// Register files, folders or globs
    #[command(label = "New table / source…", icon = IconName::Database,
              keywords = "add register import parquet csv json")]
    fn new_table(ctx: &PaletteCtx) {
        ctx.catalog.configure(ConfigureTarget::New);
    }

    /// Read tables from S3, GCS or an HTTP(S) endpoint
    #[command(label = "New data source…", icon = IconName::Sources,
              keywords = "add object store bucket s3 gcs http remote")]
    fn new_source(ctx: &PaletteCtx) {
        let mut slot = ctx.source;
        slot.set(Some(SourceTarget::New));
    }

    /// Browse and re-run past queries
    #[command(label = "Query history", icon = IconName::Clock, keywords = "recent past runs")]
    fn query_history(ctx: &PaletteCtx) {
        let mut session = ctx.catalog.session;
        session
            .write_channel(Chan::Layout)
            .open_drawer(DrawerTab::History);
    }

    /// Show or hide the catalog
    #[command(label = "Toggle sidebar", icon = IconName::Lines,
              keywords = "collapse catalog panel")]
    fn toggle_sidebar(ctx: &PaletteCtx) {
        let mut session = ctx.catalog.session;
        session
            .write_channel(Chan::Layout)
            .toggle_pane(SidebarPane::Catalog);
    }

    /// Open or create a project
    #[command(label = "Switch project…", icon = IconName::Folder, key = Command::OpenProject,
              keywords = "open new folder")]
    fn switch_project(ctx: &PaletteCtx) {
        ctx.open.pick(ctx.platform.clone(), ctx.app.clone());
    }

    /// Return to the welcome window
    #[command(label = "Close project", icon = IconName::LogOut, key = Command::CloseProject,
              keywords = "exit quit launcher")]
    fn close_project(ctx: &PaletteCtx) {
        close_project(
            &ctx.guard,
            ctx.catalog.config,
            ctx.confirm,
            ctx.platform.clone(),
            ctx.app.clone(),
        );
    }

    /// Theme, appearance and data display
    #[command(label = "Settings…", icon = IconName::Gear, key = Command::OpenSettings,
              keywords = "preferences theme customize appearance keymap engine")]
    fn settings(ctx: &PaletteCtx) {
        open_settings(ctx.platform.clone(), ctx.app.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated enum and the generated slice describe the same set, **in the same order** —
    /// the one thing a reader has to take on trust from the macro, since `route()` indexes one by
    /// the other and a shift would silently give every command its neighbour's label and body.
    #[test]
    fn every_action_has_its_own_route() {
        assert_eq!(Action::ALL.len(), ROUTES.len());
        for (index, action) in Action::ALL.iter().enumerate() {
            assert_eq!(
                action.route().id,
                ROUTES[index].id,
                "ROUTES is out of order"
            );
            assert_eq!(*action as usize, index, "Action::ALL is out of order");
        }
    }

    /// Two commands with the same id would be two rows nothing could tell apart — the rule the
    /// settings index and the History drawer's collapse key both hold.
    #[test]
    fn no_two_commands_share_an_id_or_a_label() {
        for (field, mut values) in [
            ("id", Action::ALL.iter().map(|a| a.id()).collect::<Vec<_>>()),
            ("label", Action::ALL.iter().map(|a| a.label()).collect()),
        ] {
            values.sort_unstable();
            let count = values.len();
            values.dedup();
            assert_eq!(count, values.len(), "duplicate command {field}");
        }
    }

    /// Every command says something about itself. A blank subtext is a missing doc comment,
    /// which the macro cannot refuse — it is what the row reads under the command's name.
    #[test]
    fn every_command_describes_itself() {
        for action in Action::ALL {
            assert!(!action.sub().is_empty(), "{action:?} has no doc comment");
            assert!(!action.label().is_empty(), "{action:?} has no label");
        }
    }
}
