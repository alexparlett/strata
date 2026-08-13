//! The **connection editor** (W7 · 03 / design `Connections.dc.html`) — add a remote object
//! store to this project, or edit one: its provider, its bucket, and where that provider's
//! credentials are to be found.
//!
//! **A window, not a modal**, and the Configure window's shape throughout. The canvas is a
//! 480 × 588 frame with traffic lights, a drag bar and its own footer, so this is a child window
//! of the project window that asked, pinned above it ([`crate::platform::connection`]) and
//! closing with the **project subtree** whose store, log, catalog and scan counter it borrows
//! rather than with the window id ([`crate::platform::owner`]) — a re-root or an engine restart
//! frees those while leaving that window open.
//!
//! **Single-instance per target**, for Configure's reason: this window carries a *def*, and two
//! windows on one def would both `upsert_connection` and both persist, so the second would
//! silently revert the first.
//!
//! **Nothing here holds a secret, and there is no field that could.** The auth controls choose
//! between the host's own chain, a named `~/.aws` profile and a service-account key **file
//! path** — [`strata_model::S3Auth`] and [`strata_model::GcsAuth`] have no variant carrying a
//! key, so a form built from them cannot grow one by accident.
//!
//! **No theme of its own** (Configure's rule): the chrome is `components::window`, everything
//! form-shaped is the shared `form` theme, and the semantic tones come through `tones()`.
//!
//! **Save writes the def and asks for the pass; it registers nothing itself.** It writes onto the
//! shared project store, persists through `persisted_defs`, and asks the window's one scan driver
//! for a **whole-catalog** re-scan. That width is the honest one: the case needing a re-connect is
//! the one this window exists for, and every table over the bucket was registered against the store
//! this save replaces. So Save *is* the ↻ the user would otherwise press, with the def written
//! first.
//!
//! It then **watches its row**: `Loading` is the connecting state, `Ready` closes it, `Failed`
//! keeps it open carrying the engine's own reason — worth staying open for here more than anywhere
//! else, because that reason describes the very field the user still has in front of them.
//!
//! **An edit that moves the bucket or the provider moves the connection's identity**, and the
//! store registered under the old URL survives it: `engine::store::connect` only ever sees the
//! def it is given. Deregistering the old one is this window's ([`views::Footer`]) — the same
//! `Engine::disconnect` call Forget makes.
//!
//! **Closing discards the draft, deliberately without asking** — nothing is written until Save,
//! so a close costs a form rather than data (Configure settled this; a dirty-close confirm was
//! considered and declined).

#[cfg(test)]
mod interaction;
mod model;
mod views;

use freya::prelude::*;
use freya::radio::{use_share_radio, RadioStation};
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use freya::winit::window::WindowId;
use strata_core::config::Command;

use crate::apps::connection::views::{use_watch_connection, ConnectionBody, Footer, TitleBar};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::ReportCtx;
use crate::apps::project::{Catalog, CatalogRescan, ProjChan, ProjectState};
use crate::components::window::window_theme;
use crate::keymap::on_commands;
use crate::menu::MenuScope;
use crate::platform::{quit, use_owner_pin, use_register_window, Subtree, WindowKind};
use crate::state::{use_share_config, AppCtx};
use crate::theme::{peek_selection, use_roles, use_strata_theme, window_background, Role};

pub use model::{ConnectionDraft, ConnectionTarget};

/// Everything a press of the pane's `+`, its empty-state CTA or a row's **Edit connection**
/// needs, resolved where the stores and the DI handles both live and carried to the trigger as a
/// prop — [`ConfigureLaunch`](crate::apps::configure::ConfigureLaunch)'s field set exactly,
/// because the two windows write to the same store through the same funnels.
#[derive(Clone)]
pub struct ConnectionLaunch {
    pub target: ConnectionTarget,
    pub app: AppCtx,
    /// The project window's catalog store, shared into this window rather than forked, so what
    /// this writes is what the Connections pane shows.
    pub project: RadioStation<ProjectState, ProjChan>,
    /// The **open project** every handle here belongs to — what ties this window's lifetime to
    /// that subtree rather than to the owner's window id.
    pub subtree: Subtree,
    /// The project window's re-scan request. Save bumps it; the driver over there runs the pass.
    pub rescan: CatalogRescan,
    /// …and that driver's gate. A request raised while a pass is already in flight is dropped
    /// (`claim_scan`) and nothing retries it, so Save disables itself for the duration exactly
    /// as the sidebar's ↻ and the Configure window's Save do.
    pub catalog: Catalog,
    /// The project window's engine — for the one call the scan driver cannot make: dropping the
    /// object store that an edit which moved the bucket or the provider left behind.
    pub engine: EngineCtx,
    /// Where this window **reports a failed `.strata` write** (P4-15): the project window's event
    /// log and its write-fault satellite, together. Carried as a launch value because a separate
    /// OS window inherits no context, and `use_report` panics rather than degrading when a half
    /// is missing.
    pub report: ReportCtx,
}

/// Compares on the **target** alone, for `ConfigureLaunch`'s reason: everything else is fixed for
/// the window's life, so none of it carries information a diff could act on.
impl PartialEq for ConnectionLaunch {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target
    }
}

/// How the save is going.
#[derive(Clone, PartialEq, Debug)]
pub enum Status {
    /// Waiting for the user.
    Idle,
    /// The def is written and a registration pass is in flight for this URL. The window watches
    /// that row: `Ready` closes it, `Failed` brings the engine's reason back here.
    Connecting(String),
    /// The last attempt failed. Kept on screen, because this window is the only place that can
    /// explain it while the user still has the draft that caused it.
    Failed(String),
}

/// The window's shared state, provided at the root and consumed by every view.
#[derive(Clone, Copy)]
pub struct ConnectionCtx {
    /// Every control's edits land here.
    pub draft: State<ConnectionDraft>,
    /// What this window opened on. A `State` so the context stays `Copy`, and because Save
    /// re-points it at what it just wrote (see [`views::Footer`]).
    pub target: State<ConnectionTarget>,
    pub status: State<Status>,
    /// The AWS profile names this machine defines — the **Named profile** picker's options,
    /// read once at mount (`Engine::aws_profiles`). `None` until that read answers, which is
    /// what lets the picker say "looking" rather than "you have none".
    pub profiles: State<Option<Vec<String>>>,
    /// Which client-option row the table's toolbar acts on.
    ///
    /// **Window state, not draft state** — `ConfigureCtx::selected_path`'s rule, and for its
    /// reason: it is a way of looking rather than part of the def, and on the draft every click
    /// on a row would count as an edit and clear the engine's failure message out from under a
    /// user who was still reading it.
    pub selected_option: State<Option<u64>>,
}

impl ConnectionCtx {
    /// Mutate the draft — **the one write path**, used by every control here.
    ///
    /// Idempotent, and it clears a failure: a message describes the draft that produced it, so
    /// any change to that draft makes it a lie. Refused while a pass is in flight, because the
    /// window closes on that pass's answer and an edit accepted now would be silently discarded.
    /// (`ConfigureCtx::edit`'s contract, down to why the comparison lives in here rather than in
    /// each control: `use_side_effect` builds its closure once, so a captured comparison value
    /// freezes at the first render and reverting a field silently does nothing.)
    pub fn edit(mut self, f: impl FnOnce(&mut ConnectionDraft)) {
        if matches!(*self.status.peek(), Status::Connecting(_)) {
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

/// The connection editor window: the canvas's 480 × 588 frame, with the title bar drawn by
/// [`TitleBar`] rather than AppKit — the same transparent-titlebar treatment as every other
/// window here.
pub struct ConnectionApp {
    pub app: AppCtx,
    pub project: RadioStation<ProjectState, ProjChan>,
    pub subtree: Subtree,
    pub rescan: CatalogRescan,
    pub catalog: Catalog,
    pub engine: EngineCtx,
    pub target: ConnectionTarget,
    pub report: ReportCtx,
    /// The window this one belongs to. Carried rather than looked up, because the root's own
    /// `use_register_window` re-reports its kind and an entry that forgot its owner would stop
    /// this window closing with the project window it edits.
    pub owner: WindowId,
}

impl ConnectionApp {
    #[allow(clippy::too_many_arguments)]
    pub fn window(
        app: AppCtx,
        project: RadioStation<ProjectState, ProjChan>,
        subtree: Subtree,
        rescan: CatalogRescan,
        catalog: Catalog,
        engine: EngineCtx,
        target: ConnectionTarget,
        report: ReportCtx,
        owner: WindowId,
    ) -> WindowConfig {
        let background = {
            let sel = peek_selection(app.config, app.preview);
            let id = sel.effective(strata_core::theme::os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        WindowConfig::new_app(ConnectionApp {
            app,
            project,
            subtree,
            rescan,
            catalog,
            engine,
            target,
            report,
            owner,
        })
        .with_title("Connection")
        .with_size(480., 588.)
        .with_min_size(420., 400.)
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

impl App for ConnectionApp {
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

        let owner = self.owner;
        let target = self.target.clone();
        use_register_window(
            &self.app,
            {
                move || WindowKind::Connection {
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
                    None => ConnectionDraft::default(),
                    Some(url) => project
                        .peek()
                        .connections
                        .iter()
                        .find(|c| c.def.url() == url)
                        .map(|row| ConnectionDraft::of(&row.def))
                        .unwrap_or_else(|| {
                            panic!("edit '{url}': no such connection in this project")
                        }),
                };
                ConnectionCtx {
                    draft: State::create(draft),
                    target: State::create(target),
                    status: State::create(Status::Idle),
                    profiles: State::create(None),
                    selected_option: State::create(None),
                }
            }
        });

        use_hook({
            let engine = self.engine.clone();
            move || {
                let mut profiles = ctx.profiles;
                spawn(async move {
                    let found = engine.aws_profiles().await;
                    profiles.set(Some(found));
                });
            }
        });

        use_watch_connection(ctx);

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
            .child(ConnectionBody)
            .child(Footer)
            .child(rect().on_global_key_down(on_commands(config, {
                move |cmd| match cmd {
                    Command::Cancel => {
                        platform.close_current_window();
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
