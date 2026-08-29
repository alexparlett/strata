//! The **Configure** window (P4-11 / design `Configure.dc.html`) — register a new external
//! table, or edit an existing one: its name, its reader and that reader's options, its source
//! paths, and its Hive partition columns.
//!
//! **A window, not a modal.** The canvas is a 620 × 640 frame with traffic lights, a drag bar
//! and its own footer, so this is a child window of the project window that asked, pinned above
//! it ([`crate::platform::configure`]) — the export window's shape. What it closes *with* is not
//! that window but the **project subtree** whose store, log, catalog and scan counter it borrows,
//! since a re-root or an engine restart frees those while leaving the window open
//! ([`crate::platform::owner`]).
//!
//! Where it differs from Export is **single-instance per target**: each Export window carries a
//! different run, while a Configure window carries a *def*, and two on one def would both
//! `upsert_table` and both persist, so the second would silently revert the first.
//!
//! **A table reads from the local disk or from one of the project's object stores.** That is the
//! LOCATION toggle, and behind its second answer the TYPE / CONNECTION pair (`views::location`) —
//! an explicit choice, never inferred from a typed path. It changes the source list below to one
//! bucket-relative path. The def records the data source's URL and nothing else about it, and
//! `register::table_spec` composes the two.
//!
//! **No theme of its own.** A window is not a component: its chrome is the app's role vocabulary
//! and everything form-shaped is the shared `form` theme. A per-window block of sixteen fields
//! resolving to the same handful of roles is four blocks to keep in step for one reskin.
//!
//! **Closing discards the draft, deliberately without asking.** Nothing is written until Save, so a
//! close costs a form rather than data — true mid-registration too, where the pass belongs to the
//! project window's scan driver and answers on the catalog row either way.
//!
//! **Save does not register anything itself.** It writes the def, persists it, and asks the project
//! window's one scan driver for a pass over that table — the same pass project open and the
//! sidebar's ↻ use. This window then *watches* its row: `Loading` is validating, `Ready` closes it,
//! `Failed` keeps it open with the reason. A reconciliation over shared state, not a second
//! registration path.

#[cfg(test)]
mod interaction;
mod model;
mod views;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use freya::prelude::*;
use freya::radio::{use_share_radio, RadioStation};
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use freya::winit::window::WindowId;
use strata_core::config::Command;

use crate::apps::configure::views::{use_watch_registration, ConfigureBody, Footer, TitleBar};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::ReportCtx;
use crate::apps::project::SourceRequest;
use crate::apps::project::{Catalog, CatalogRescan, ProjChan, ProjectState};
use crate::components::window::window_theme;
use crate::keymap::on_commands;
use crate::menu::MenuScope;
use crate::platform::{quit, use_owner_pin, use_register_window, Subtree, WindowKind};
use crate::state::{use_share_config, AppCtx};
use crate::theme::{peek_selection, use_roles, use_strata_theme, window_background, Role};

pub use model::{ConfigureDraft, ConfigureTarget, Probes};

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
    /// The **open project** every handle here belongs to — what ties the window's lifetime to
    /// that subtree rather than to the owner's window id, which outlives it both across a re-root
    /// and across an engine restart (see [`crate::platform::owner`]).
    pub subtree: Subtree,
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
    /// Where this window **reports a failed `.strata` write** — the project window's event log
    /// and its write-fault satellite, together (P4-15). Both halves belong to *that* window for
    /// the same reason: a registration is a fact about its catalog, and this window closes on
    /// success, so anything shown here would vanish with it.
    ///
    /// Carried as a launch value because a separate OS window inherits no context. Forgetting
    /// this is not a subtle bug — `use_report` consumes both halves and **panics** when one is
    /// missing, which is how the first version of this crashed the moment Configure opened.
    pub report: ReportCtx,
    /// The project window's **data-source editor request** (W7 · 04) — what the CONNECTION
    /// picker's *New data source…* sets.
    ///
    /// The slot rather than a second `open_source` call: that window needs the project
    /// window's handles and belongs to *its* lifetime, and there is deliberately one open path
    /// (`project::views::source_launch`). A `State` carries a press across the window
    /// boundary exactly as [`rescan`](Self::rescan) already does.
    pub editor: SourceRequest,
}

/// Compares on the **target** alone. Everything else is fixed for the window's life — one store,
/// one counter, one report (P4-15: the project window's log *and* its write-fault satellite), and
/// one `Subtree` that can only change by remounting the very subtree this is built in — so none
/// of it carries information a diff could act on. The same reasoning as `ExportLaunch`.
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
    ///
    /// The work belongs to the **project** window's scan driver, which lands its answer on the
    /// catalog row whether this window is here to watch or not — which is why this state does
    /// not hold the window open.
    Registering(String),
    /// **This window is running the statement that creates `name`** (IT-01) — the one state in
    /// which the work is *this* window's own.
    ///
    /// It is held apart from [`Registering`](Self::Registering) because it answers a different
    /// question about closing. An internal table is created by a task spawned here, and the fold
    /// that writes the def, the catalog row and the log entry runs **after** that task's await —
    /// so a window closed now drops the task, and `ddl::tables::create` has already published its
    /// spool by rename before its last await. The result would be a data directory under
    /// `.strata/tables/` that no def points at and no sweep collects. So this state refuses the
    /// window's own close paths (Cancel and Esc), and clears into `Registering` the moment the
    /// fold has landed.
    Creating(String),
    /// The last attempt failed. Kept on screen, because this window is the only place that can
    /// explain it while the user still has the draft that caused it.
    Failed(String),
}

impl Status {
    /// Whether the window may **not** be dismissed right now, because the work in flight is this
    /// window's own.
    ///
    /// One predicate, because two surfaces answer with it — the footer's Cancel button and the
    /// root's Esc — and a window whose button said one thing while its key did another would be
    /// the drift `save_note` exists to prevent one row up. `Registering` is deliberately *not*
    /// included: that pass belongs to the project window's scan driver and lands on the catalog
    /// row whether this window is watching or not, so refusing to close for it would only mean a
    /// window nobody can dismiss if the pass never answers.
    pub fn holds_window(&self) -> bool {
        matches!(self, Status::Creating(_))
    }

    /// Whether the window is busy at all — either kind of work, which is what disables Save and
    /// what [`ConfigureCtx::edit`] refuses against.
    pub fn busy(&self) -> bool {
        matches!(self, Status::Registering(_) | Status::Creating(_))
    }
}

/// The window's shared state, provided at the root and consumed by every view.
#[derive(Clone, Copy)]
pub struct ConfigureCtx {
    /// Every control's edits land here.
    pub draft: State<ConfigureDraft>,
    /// What this window opened on. A `State` only so the context stays `Copy` — nothing writes it.
    pub target: State<ConfigureTarget>,
    pub status: State<Status>,
    /// Which source-path row the toolbar acts on.
    ///
    /// **Window state, not draft state.** It is a way of looking, not part of the def — and
    /// keeping it on the draft meant every click on a row counted as an edit, which cleared the
    /// engine's failure message out from under a user who was still reading it.
    pub selected_path: State<usize>,
    /// Which column row the COLUMNS toolbar acts on (IT-01) — `selected_path`'s twin, window
    /// state for the same reason.
    pub selected_column: State<usize>,
    /// What the planner has said about each SQL type spelling typed into a column box, keyed by
    /// the text. Filled by [`use_probes`] and read by the rows and the footer.
    pub probes: State<Probes>,
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
        if self.status.peek().busy() {
            return;
        }
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
    pub subtree: Subtree,
    pub rescan: CatalogRescan,
    pub catalog: Catalog,
    pub engine: EngineCtx,
    pub target: ConfigureTarget,
    pub report: ReportCtx,
    /// The project window's data-source editor request — see [`ConfigureLaunch::editor`].
    pub editor: SourceRequest,
    /// The window this one belongs to. Carried rather than looked up because the root's own
    /// `use_register_window` re-reports its kind, and an entry that forgot its owner would stop
    /// this window closing with the project window it configures.
    pub owner: WindowId,
    /// Whether this window is refusing to close — the `on_close` half of
    /// [`Status::holds_window`], mirrored into a flag the winit hook can read.
    ///
    /// Esc and the Cancel button are in-app presses this window answers itself, but the native
    /// traffic-light button and ⌘Q are **winit's**: both route through `process_close_request`,
    /// which closes unconditionally when a window registered no `on_close` hook. So the same
    /// predicate has to be reachable from outside the component tree, and an `Arc<AtomicBool>`
    /// built with the window is what reaches — the shape `project::close::close_bridge` already
    /// uses for the same job.
    pub close_hold: Arc<AtomicBool>,
}

impl ConfigureApp {
    #[allow(clippy::too_many_arguments)]
    pub fn window(
        app: AppCtx,
        project: RadioStation<ProjectState, ProjChan>,
        subtree: Subtree,
        rescan: CatalogRescan,
        catalog: Catalog,
        engine: EngineCtx,
        target: ConfigureTarget,
        report: ReportCtx,
        editor: SourceRequest,
        owner: WindowId,
    ) -> WindowConfig {
        let background = {
            let sel = peek_selection(app.config, app.preview);
            let id = sel.effective(strata_core::theme::os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        let close_hold = Arc::new(AtomicBool::new(false));
        let hook_hold = close_hold.clone();
        WindowConfig::new_app(ConfigureApp {
            app,
            project,
            subtree,
            rescan,
            catalog,
            engine,
            target,
            report,
            editor,
            owner,
            close_hold,
        })
        .with_on_close(move |_ctx: RendererContext<'_>, _id: WindowId| {
            match hook_hold.load(Ordering::Relaxed) {
                true => CloseDecision::KeepOpen,
                false => CloseDecision::Close,
            }
        })
        .with_title("Table configuration")
        .with_size(620., 640.)
        .with_min_size(480., 420.)
        .with_background(background)
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
        use_strata_theme(self.app.themes.clone(), self.app.config, self.app.preview);
        use_share_config(self.app.config);
        use_provide_context({
            let app = self.app.clone();
            move || app
        });
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
        let report = self.report;
        use_provide_context(move || report.log);
        use_provide_context(move || report.faults);
        let editor = self.editor;
        use_provide_context(move || editor);

        let owner = self.owner;
        let target = self.target.clone();
        use_register_window(
            &self.app,
            {
                move || WindowKind::Configure {
                    owner,
                    target: target.clone(),
                }
            },
            MenuScope::Panel,
        );
        use_owner_pin(self.app.clone(), owner, self.subtree.clone());

        let ctx = use_provide_context({
            let target = self.target.clone();
            let project = self.project;
            move || {
                let draft = match target.editing() {
                    None => ConfigureDraft::default(),
                    Some(name) => {
                        let store = project.peek();
                        let sources: Vec<_> = store.sources.iter().map(|c| c.def.clone()).collect();
                        store
                            .tables
                            .iter()
                            .find(|t| ProjectState::same_name(&t.def.name, name))
                            .map(|row| ConfigureDraft::of(&row.def, &sources))
                            .unwrap_or_else(|| {
                                panic!("configure '{name}': no such table in this project")
                            })
                    }
                };
                ConfigureCtx {
                    draft: State::create(draft),
                    target: State::create(target),
                    status: State::create(Status::Idle),
                    selected_path: State::create(0),
                    selected_column: State::create(0),
                    probes: State::create(Probes::new()),
                }
            }
        });

        use_watch_registration(ctx);
        let close_hold = self.close_hold.clone();
        use_side_effect(move || {
            close_hold.store(ctx.status.read().holds_window(), Ordering::Relaxed);
        });

        let win = window_theme();
        let text = use_roles().get(Role::Text);
        let config = self.app.config;
        let platform = use_hook(Platform::get);

        rect()
            .expanded()
            .vertical()
            .content(Content::Flex)
            .background(win.background)
            .color(text)
            .child(TitleBar)
            .child(ConfigureBody)
            .child(Footer)
            .child(rect().on_global_key_down(on_commands(config, {
                move |cmd| match cmd {
                    Command::Cancel => {
                        if !ctx.status.peek().holds_window() {
                            platform.close_current_window();
                        }
                        true
                    }
                    Command::Quit => {
                        quit();
                        true
                    }
                    _ => false,
                }
            })))
    }
}
