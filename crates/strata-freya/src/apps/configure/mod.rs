//! The **Configure** window (P4-11 / design `Configure.dc.html`) — register a new external
//! table, or edit an existing one: its name, its reader and that reader's options, its source
//! paths, and its Hive partition columns.
//!
//! **A window, not a modal.** The canvas is a 620 × 640 frame with traffic lights, a drag bar
//! and its own footer, so this is a child window of the project window that asked, pinned above
//! it and closing with it — the export window's shape ([`crate::platform::configure`]).
//!
//! Where it differs from Export is **single-instance per target**. Export deliberately has no
//! such rule because each of its windows carries a different run. A Configure window carries a
//! *def*, and two windows on one def would both `upsert_table` and both persist, so the second
//! would silently revert the first — the same reason two windows cannot share a project.
//!
//! **Location is local disk only.** The canvas opens with a LOCATION toggle (Local disk ·
//! Object store) and a connection picker behind it. Connections do not exist yet, so the toggle
//! would offer one option, and a one-option toggle is a control that cannot be operated. The
//! whole section is left out rather than shipped disabled; **W7 ▸ 04** adds it and the remote
//! branch back.
//!
//! **No theme of its own.** A window is not a component: its chrome — body, rules, panels,
//! text — is the app's *sheet*, and everything form-shaped is the shared `form` theme. A
//! per-window block of sixteen fields that all resolve to the same handful of sheet slots is
//! four blocks to keep in step for one reskin, which is the drift a shared vocabulary exists to
//! prevent. (The three windows that still carry one predate this and should follow.)
//!
//! **Save does not register anything itself.** It writes the def, persists it, and asks the
//! project window's one scan driver for a pass over that table
//! ([`refresh_table`](crate::apps::project::state::refresh_table)) — the same pass project open
//! and the sidebar's ↻ use. So there is one implementation of "make the engine match the defs",
//! the per-def event-log entries come from it as they always have, and a failure lands on the
//! catalog row wearing P3-07's message. This window then *watches* its row: `Loading` is the
//! validating state, `Ready` closes it, `Failed` keeps it open with the reason. A reconciliation
//! over shared state, not a second registration path.

mod model;
mod views;

use freya::prelude::*;
use freya::radio::{use_share_radio, RadioStation};
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use freya::winit::window::WindowId;
use strata_core::config::Command;

use crate::apps::configure::views::{ConfigureBody, Footer, TitleBar};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::LogCtx;
use crate::apps::project::{Catalog, CatalogRescan, ProjChan, ProjectState};
use crate::keymap::on_commands;
use crate::platform::{self, WindowKind};
use crate::state::{use_share_config, AppCtx};
use crate::theme::{peek_selection, use_strata_theme, window_background};

pub use model::{ConfigureDraft, ConfigureTarget, Edit};

/// Everything a press of the catalog's **Configure** (or New table) needs, resolved where the
/// stores and the DI handles both live and carried to the trigger as a prop.
///
/// **Props, not context.** The catalog row's menu is a shallow, known consumer — the
/// state-architecture's rule for exactly this (AGENTS §4: context is for DI handles and deep,
/// open-ended trees).
#[derive(Clone)]
pub struct ConfigureLaunch {
    pub target: ConfigureTarget,
    pub app: AppCtx,
    /// The project window's catalog store, shared into this window. A `RadioStation` is `Copy`
    /// and every window runs on the one renderer thread, which is what makes the app-global
    /// config store shareable too (`state::config`) — this is the same move, scoped to one
    /// project.
    pub project: RadioStation<ProjectState, ProjChan>,
    /// The project window's re-scan request. Save bumps it; the driver over there runs the pass.
    pub rescan: CatalogRescan,
    /// …and that driver's gate. A request raised while a pass is already in flight is **dropped**
    /// (`claim_scan`), and nothing retries it — the sidebar's ↻ lives with that by disabling
    /// itself for the duration, so Save has to do the same or it would leave a row `Loading` for
    /// good and this window watching it for ever.
    pub catalog: Catalog,
    /// The project window's engine. Registration is the scan driver's, not this window's — the
    /// engine is here for the one thing the driver cannot do, which is drop the table a
    /// **rename** left behind under its old name.
    pub engine: EngineCtx,
    /// The **project window's** event log. A registration is a fact about that window's
    /// catalog, and this window closes on success, so a line shown here would vanish with it.
    pub log: LogCtx,
}

/// Compares on the **target** alone. The rest are handles — one store, one counter, one log,
/// fixed for the window's life — so they carry no information a diff could act on. The same
/// reasoning as `ExportLaunch`.
impl PartialEq for ConfigureLaunch {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target
    }
}

/// How the save is going.
#[derive(Clone, PartialEq)]
pub enum Status {
    /// Waiting for the user.
    Idle,
    /// The def is written and a registration pass is in flight for `name`. The window watches
    /// that row: `Ready` closes it, `Failed` brings the reason back here.
    Registering(String),
    /// The last attempt failed. Kept on screen, because this window is the only place that can
    /// explain it while the user still has the draft that caused it.
    Failed(String),
}

/// The window's shared state, provided at the root and consumed by every view.
#[derive(Clone, Copy)]
pub struct ConfigureCtx {
    /// Every control's edits land here.
    pub draft: State<ConfigureDraft>,
    /// What this window opened on. A `State` only so the context stays `Copy` — nothing writes it.
    pub target: State<ConfigureTarget>,
    pub status: State<Status>,
    /// Whether the ADVANCED disclosure is open. Window state rather than the disclosure's own,
    /// so switching format and back does not fold it up again.
    pub advanced_open: State<bool>,
}

impl ConfigureCtx {
    /// Mutate the draft — **the one write path**, used by the data-driven option groups and by
    /// the bespoke controls (the name box, the path list, the partition types) alike.
    ///
    /// **Idempotent**: an edit that changes nothing neither writes nor clears the failure. That
    /// is what lets a control just report what the user typed instead of guarding the write
    /// itself — a guard the text controls got *wrong* before, because `use_side_effect` builds
    /// its closure once, so any captured comparison value froze at the first render and
    /// reverting a field silently did nothing.
    ///
    /// Clearing the failure here means no control has to remember to: a message describes the
    /// draft that produced it, so any change to the draft makes it a lie.
    pub fn edit(mut self, f: impl FnOnce(&mut ConfigureDraft)) {
        {
            let mut next = self.draft.peek().clone();
            f(&mut next);
            if next == *self.draft.peek() {
                return;
            }
            self.draft.set(next);
        }
        if matches!(*self.status.peek(), Status::Failed(_)) {
            self.status.set(Status::Idle);
        }
    }
}

/// The Configure window: the canvas's 620 × 640 frame, with the title bar drawn by [`TitleBar`]
/// rather than AppKit — the same transparent-titlebar treatment as every other window here.
pub struct ConfigureApp {
    pub app: AppCtx,
    pub project: RadioStation<ProjectState, ProjChan>,
    pub rescan: CatalogRescan,
    pub catalog: Catalog,
    pub engine: EngineCtx,
    pub target: ConfigureTarget,
    pub log: LogCtx,
    /// The window this one belongs to. Carried rather than looked up because the root's own
    /// `use_register_window` re-reports its kind, and an entry that forgot its owner would stop
    /// this window closing with the project window it configures.
    pub owner: WindowId,
}

impl ConfigureApp {
    pub fn window(
        app: AppCtx,
        project: RadioStation<ProjectState, ProjChan>,
        rescan: CatalogRescan,
        catalog: Catalog,
        engine: EngineCtx,
        target: ConfigureTarget,
        log: LogCtx,
        owner: WindowId,
    ) -> WindowConfig {
        // Match the theme's window body so a resize doesn't flash the default white — through
        // `peek_selection`, since Settings may be previewing a theme right now.
        let background = {
            let sel = peek_selection(app.config, app.preview);
            let id = sel.effective(strata_core::theme::os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        WindowConfig::new_app(ConfigureApp {
            app,
            project,
            rescan,
            catalog,
            engine,
            target,
            log,
            owner,
        })
        .with_title("Table configuration")
        // The OS title is hidden (this window draws its own bar), so it names the *kind* of
        // window rather than the table: `with_title` takes a `&'static str`, and the real name —
        // "New table" / "Configure events" — is on the bar the user is actually looking at.
        .with_size(620., 640.)
        // Below this the source-path toolbar and the delimiter pill stop fitting on one row.
        .with_min_size(480., 420.)
        .with_background(background)
        // The 50px strip centres macOS's 16px buttons at y = 17; AppKit's default origin is
        // (7, 6), so the inset is the difference.
        .with_traffic_light_inset(9., 11.)
        .with_window_attributes(move |attrs, _| {
            attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true)
        })
    }
}

impl App for ConfigureApp {
    fn render(&self) -> impl IntoElement {
        // The window-root steps every app takes: this window's theme derived from the shared
        // settings, and the app-globals into context.
        use_strata_theme(self.app.themes.clone(), self.app.config, self.app.preview);
        use_share_config(self.app.config);
        use_provide_context({
            let app = self.app.clone();
            move || app
        });
        // The project window's catalog store, its scan request and its log — shared, not
        // forked, so what this window writes is what that window shows.
        let project = self.project;
        use_share_radio(move || project);
        let rescan = self.rescan;
        use_provide_context(move || rescan);
        let catalog = self.catalog;
        use_provide_context(move || catalog);
        use_provide_context({
            let engine = self.engine.clone();
            move || engine
        });
        let log = self.log;
        use_provide_context(move || log);

        // Join the live window registry, so a second Configure on this table focuses this
        // window rather than opening another, and so it closes with its project window.
        let owner = self.owner;
        let target = self.target.clone();
        platform::use_register_window(self.app.windows, {
            let target = target.clone();
            move || WindowKind::Configure {
                owner,
                target: target.clone(),
            }
        });
        platform::use_configure_pin(self.app.clone());

        let ctx = use_provide_context({
            let target = self.target.clone();
            let project = self.project;
            move || {
                // Seeded from the def this window opened on, field by field, so opening an
                // existing table and pressing Save without touching anything writes back the
                // def that was already there.
                let draft = match target.editing() {
                    None => ConfigureDraft::default(),
                    Some(name) => project
                        .peek()
                        .tables
                        .iter()
                        .find(|t| ProjectState::same_name(&t.def.name, name))
                        .map(|row| ConfigureDraft::of(&row.def))
                        .unwrap_or_default(),
                };
                ConfigureCtx {
                    draft: State::create(draft),
                    target: State::create(target),
                    status: State::create(Status::Idle),
                    advanced_open: State::create(false),
                }
            }
        });

        // The registration this window is waiting on, watched on the catalog store rather than
        // awaited: the pass belongs to the project window's driver, and its answer arrives on
        // the row. See the module doc.
        views::use_watch_registration(ctx);

        let colors = use_theme().read().colors().clone();
        let config = self.app.config;
        let platform = use_hook(Platform::get);

        rect()
            .expanded()
            .vertical()
            .content(Content::Flex)
            .background(colors.background)
            // The window's ambient text colour, like every other window root's: runs that don't
            // name one inherit it rather than Freya's base-theme default.
            .color(colors.text_primary)
            .child(TitleBar)
            .child(ConfigureBody)
            .child(Footer)
            // Esc and ⌘Q. Deliberately the LAST child — same-name global listeners fire in
            // document order, so anything a view mounts outranks this.
            .child(rect().on_global_key_down(on_commands(config, {
                move |cmd| match cmd {
                    // Esc closes, always. The def is written only by Save, so there is nothing
                    // to undo — and a registration in flight belongs to the *project* window's
                    // scan driver, which lands its answer on the catalog row whether this window
                    // is watching or not. Refusing to close here would only mean a window that
                    // cannot be dismissed if that pass never answers.
                    Command::Cancel => {
                        platform.close_current_window();
                        true
                    }
                    Command::Quit => {
                        platform::quit();
                        true
                    }
                    _ => false,
                }
            })))
    }
}
