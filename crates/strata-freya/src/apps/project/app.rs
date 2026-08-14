//! The project window **root shell** (rail · sidebar · workbench · drawer), in two layers.
//!
//! [`ProjectApp`] is the **window**: its theme, the app-globals it shares into the tree, the
//! close bridge, the menubar it points at itself, and the open path that decides where the
//! next project lands. None of that changes when the window changes project.
//!
//! [`ProjectRoot`] is the **open project**: it runs the fallible load once per mount and is one
//! of three arms — [`ProjectLoading`] while the read is out, then [`ProjectLoaded`] (the engine,
//! the Project / Session / History stores, autosave, the catalog, and every feature view, built
//! from the loaded values) or [`ProjectLoadFailed`] (the fault dialog that closes the window).
//! The read is `std::fs` on files the user named, and Freya draws every window from one thread,
//! so it runs **off** that thread ([`load_project`]) and the loading arm is what the subtree is
//! while it is out there. It is **keyed on the project folder**, so "open in this window"
//! ([`OpenPref::This`](strata_core::config::OpenPref)) is a plain `State` write — the key
//! change unmounts this subtree (flushing the session, dropping the engine, leaving the
//! open-set) and mounts the next project exactly as launch does. There is no
//! reopen-in-place path to keep in step with the mount path, because they are the same
//! path.

use std::collections::HashSet;
use std::path::PathBuf;
use std::pin::pin;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::agent::use_agent_server;
use crate::apps::configure::ConfigureTarget;
use crate::apps::connection::ConnectionTarget;
use crate::apps::project::close::{
    close_bridge, close_project, CloseBridge, CloseGuard, CloseTarget, Veto,
};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{
    chats_cap, load_project, seed_pick, use_agent_bridge, use_autosave, use_diagnostics,
    use_engine_config, use_engine_restart, use_init_agents, use_init_catalog_selection,
    use_init_chats, use_init_faults, use_init_history, use_init_log, use_init_project,
    use_init_session, use_report, AssistantCtx, Chan, EngineRestart, Loaded, SessionState,
};
use crate::apps::project::views::{
    ChatConfirm, ChatDrop, CloseConfirm, CommandPalette, ConfigureLauncher, ConnectionLauncher,
    DropConfirm, DropTarget, HeaderBar, OpenPrompt, PaletteOpen, ProfileConfirm, ProfileTarget,
    ProjectLoadFailed, ProjectLoading, RequestKeepers, SchemasPicker, SchemasRequest, ShapeDialog,
    ShapeTarget, Shell,
};
use crate::keymap::on_commands;
use crate::menu::MenuScope;
use crate::platform::{
    close_this_window, open_settings, quit, use_register_window, OpenCtx, Subtree, WindowKind,
};
use crate::state::{
    use_claim_open, use_config, use_promote_recent, use_share_config, use_updates, AppCtx,
    ConfigChan,
};
use crate::task::offload;
use crate::theme::{peek_selection, use_strata_theme, window_background};
use crate::updater::{AskSlot, UpdateConfirm};
use async_io::Timer;
use freya::prelude::*;
use freya::radio::use_radio;
use freya::winit::dpi::LogicalPosition;
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use futures::executor::block_on;
use futures::future::{select, Either};
use futures::StreamExt;
use strata_agent::assistant::Scope;
use strata_agent::StrataTools;
use strata_core::config::Command;
use strata_core::project as project_io;
use strata_core::theme::os_is_dark;
use strata_model::{TabId, WindowGeom};

pub struct ProjectApp {
    /// The app-globals `main` created once: the shared theme registry, the reactive config
    /// (settings · recents · open-set — a write on a channel repaints every window
    /// subscribed to it), the live window registry this window joins for its lifetime, and
    /// the menubar handles it points at itself while focused. The window's theme is
    /// **derived** from the settings selection by [`use_strata_theme`] — no stored
    /// applied-theme id.
    ///
    /// [`use_strata_theme`]: crate::theme::use_strata_theme
    pub app: AppCtx,
    /// The UI half of this window's close bridge: the guard the winit `on_close` hook reads
    /// + the veto receiver the root drains into the confirm dialog / the launcher hand-off.
    pub close: CloseBridge,
    /// The project folder the window **opens at** — decided by the caller (`main`'s startup
    /// routing, or whoever opened this window) before the window exists. The project the
    /// window *shows* is [`OpenCtx::root`] from here on, which starts as this and moves with
    /// an open-in-this-window.
    pub root: PathBuf,
}

impl ProjectApp {
    /// This window's config for `root` — the project folder, already chosen by the caller
    /// ([`crate::platform::open_project`] or `main`'s startup routing) — opened at `geometry`,
    /// which the caller resolved with [`window_geometry`] for the same reason it resolved the
    /// folder: a window's size and position can only be set as it is created, so both are
    /// launch inputs and neither may be read from here (this runs on the thread that draws
    /// every window).
    pub fn window(app: AppCtx, root: PathBuf, geometry: Option<WindowGeom>) -> WindowConfig {
        let background = {
            let id = peek_selection(app.config, app.preview).effective(os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        let (close, on_close) = close_bridge(app.config.peek().settings.confirm_close_running);
        let (width, height) = geometry.map_or((1200., 780.), |g| (g.width as f64, g.height as f64));
        WindowConfig::new_app(ProjectApp { app, close, root })
            .with_title("Strata")
            .with_size(width, height)
            .with_min_size(360., 240.)
            .with_background(background)
            .with_on_close(on_close)
            .with_traffic_light_inset(6., 10.)
            .with_window_attributes(move |attrs, _| {
                let attrs = attrs
                    .with_titlebar_transparent(true)
                    .with_fullsize_content_view(true)
                    .with_title_hidden(true);
                match geometry {
                    Some(g) => attrs.with_position(LogicalPosition::new(g.x as f64, g.y as f64)),
                    None => attrs,
                }
            })
    }
}

/// How long a window waits for its remembered geometry before opening at the default size.
///
/// A deadline because the read is `std::fs` on a folder the user named, and the reason for it is
/// the reason the load itself moved off the render thread: a mount that stopped answering blocks
/// in the kernel with no timeout. Every other blocking read on this path became something a
/// window can *report* ([`ProjectLoading`]), but geometry cannot — Freya has no runtime
/// resize/move, so a window is placed as it is created or not at all, and a read that is still
/// out has to be given up on for the window to exist at all. That trade is the right way round:
/// a remembered size is a nicety, a window is not, and a window is where every other truth about
/// this project gets told.
///
/// 250ms is far beyond a small local read and short enough to be imperceptible against opening a
/// window at all.
const GEOMETRY_DEADLINE: Duration = Duration::from_millis(250);

/// The geometry a project window should open at: what its session remembers, read **off the
/// render thread** and given up on after [`GEOMETRY_DEADLINE`].
///
/// A launch input, like the project folder beside it — resolved before the window exists because
/// that is the only moment Freya can act on it (AGENTS.md §2). Giving up yields `None`, which is
/// the same answer a project that has never been saved gives, and it costs nothing durable: the
/// autosave seed is taken from the session the project actually loads
/// ([`use_autosave`]), not from this, so a window that opened at the default size still keeps the
/// size the user chose.
pub async fn window_geometry(root: PathBuf) -> Option<WindowGeom> {
    let named = root.display().to_string();
    let read = pin!(offload(move || project_io::load_session(&root)
        .ok()
        .flatten()
        .and_then(|snapshot| snapshot.window)));
    let deadline = pin!(Timer::after(GEOMETRY_DEADLINE));
    match select(read, deadline).await {
        Either::Left((geometry, _)) => geometry.flatten(),
        Either::Right(_) => {
            tracing::warn!(
                "{named}: could not read the window geometry in time, opening at the default size"
            );
            None
        }
    }
}

/// [`window_geometry`] for a caller with no executor to await it on: `main`'s startup routing,
/// which runs before `launch`, and the menubar's Open Recent, which runs on the renderer thread
/// inside a muda event handler and has a `RendererContext` rather than a `Platform`.
///
/// This is the one place a project folder is still read on a thread that matters — and what makes
/// that acceptable is [`GEOMETRY_DEADLINE`]: the wait is bounded and brief where it used to be
/// unbounded. Prefer the async form wherever there is somewhere to await it.
pub fn window_geometry_blocking(root: PathBuf) -> Option<WindowGeom> {
    block_on(window_geometry(root))
}

impl App for ProjectApp {
    fn render(&self) -> impl IntoElement {
        let themes = use_provide_context({
            let themes = self.app.themes.clone();
            move || themes
        });
        use_strata_theme(themes, self.app.config, self.app.preview);
        let config = self.app.config;
        use_share_config(config);
        use_provide_context({
            let app = self.app.clone();
            move || app
        });

        let guard = use_provide_context({
            let guard = self.close.guard.clone();
            move || guard
        });
        let mut confirm = use_provide_context(|| State::create(None::<CloseTarget>));

        let engine_restart = use_engine_restart();

        let open = OpenCtx {
            root: use_state(|| self.root.clone()),
            prompt: use_state(|| None),
            guard: use_state({
                let guard = guard.clone();
                move || guard
            }),
            confirm,
            faulted: use_state(|| false),
            loaded: use_state(|| false),
            restart: engine_restart,
        };
        use_provide_context(move || open);
        let update_ask: AskSlot = use_state(|| None);

        let window_id = use_register_window(
            &self.app,
            move || WindowKind::Project(open.root.read().to_string_lossy().into_owned()),
            MenuScope::Project(open, update_ask),
        );
        use_agent_server(self.app.agent.clone(), config);
        use_updates(self.app.updates, config);

        {
            let guard = guard.clone();
            let settings = use_config(ConfigChan::Settings);
            use_side_effect(move || {
                guard.confirm.store(
                    settings.read().settings.confirm_close_running,
                    Ordering::Relaxed,
                );
            });
        }
        {
            let guard = guard.clone();
            let windows = self.app.windows;
            use_side_effect(move || {
                guard
                    .last
                    .store(windows.read().is_last(), Ordering::Relaxed);
            });
        }
        let rx = self.close.take_rx();
        let platform = use_hook(Platform::get);
        let close_window = {
            let app = self.app.clone();
            let platform = platform.clone();
            move || close_this_window(platform.clone(), app.clone())
        };
        use_hook({
            move || {
                if let Some(mut rx) = rx {
                    spawn(async move {
                        while let Some(veto) = rx.next().await {
                            match veto {
                                Veto::Confirm => confirm.set(Some(CloseTarget::Window)),
                                Veto::Launcher => close_window().await,
                            }
                        }
                    });
                }
            }
        });
        let filled_by_app = use_state(|| false);

        let root = open.root.read().clone();
        let first_mount = root == self.root && engine_restart.generation() == 0;

        rect()
            .expanded()
            .theme_background()
            .vertical()
            .child(ContextMenuViewer::new())
            .child(OpenPrompt {
                open,
                app: self.app.clone(),
            })
            .child(UpdateConfirm {
                ask: update_ask,
                status: self.app.updates,
            })
            .child(ProjectRoot {
                root,
                generation: engine_restart.generation(),
                first_mount,
                confirm,
                filled_by_app,
                app: self.app.clone(),
            })
            .child(rect().on_global_key_down(on_commands(config, {
                let app = self.app.clone();
                move |cmd| match cmd {
                    Command::OpenProject => {
                        open.pick(platform.clone(), app.clone());
                        true
                    }
                    Command::CloseProject => {
                        close_project(&guard, config, confirm, platform.clone(), app.clone());
                        true
                    }
                    Command::Quit => {
                        quit();
                        true
                    }
                    Command::OpenSettings => {
                        open_settings(platform.clone(), app.clone());
                        true
                    }
                    Command::CycleWindow => match window_id
                        .peek()
                        .and_then(|here| app.windows.peek().cycle_from(here))
                    {
                        Some(next) => {
                            platform.focus_window(Some(next));
                            true
                        }
                        None => false,
                    },
                    Command::Find => {
                        tracing::debug!("shortcut {cmd:?}: target not built yet (stub)");
                        true
                    }
                    _ => false,
                }
            })))
    }
}

/// Everything that belongs to the **open project** rather than to the window — in three
/// arms. The fallible IO (defs + session) runs once per mount, off the render thread, and what
/// it finds decides what this subtree *is*: [`ProjectLoading`] while the answer is out, then
/// [`ProjectLoaded`] (the engine, the stores, autosave, the catalog and every feature view,
/// built from the loaded values) or [`ProjectLoadFailed`] (the fault, surfaced and closed — a
/// project that can't load has no window). No store is ever built from anything but a successful
/// load.
///
/// Keyed on [`root`](Self::root) — see the module doc. Nothing in here is written to
/// re-open a project; it is only ever mounted at one.
#[derive(PartialEq)]
struct ProjectRoot {
    /// The project folder this subtree is standing up. Half of **its diff key**, so a different
    /// folder is a different subtree.
    root: PathBuf,
    /// The window's engine generation — the other half of the key. Bumping it (P4-07's restart)
    /// is a remount of this same project, which is the only way to apply a changed
    /// `datafusion.runtime.*` property: the `RuntimeEnv` is fixed when the engine is built.
    generation: u64,
    /// Whether this is the project the window was created for, on its first mount — the one case
    /// with an autosave geometry seed to offer. See `ProjectApp::render`.
    first_mount: bool,
    /// The window's close-confirm slot: this subtree owns the dialog (it needs the project
    /// and session stores to name what it is closing), the window owns the slot.
    confirm: State<Option<CloseTarget>>,
    /// Whether the app filled the window (the header's double-press) — a window fact the
    /// header writes and autosave reads.
    filled_by_app: State<bool>,
    app: AppCtx,
}

impl Component for ProjectRoot {
    fn render(&self) -> impl IntoElement {
        use_claim_open(self.app.config, &self.root);

        let load = use_future({
            let root = self.root.clone();
            move || load_project(root.clone())
        });
        match &*load.state() {
            FutureState::Pending | FutureState::Loading => ProjectLoading {
                root: self.root.clone(),
                confirm: self.confirm,
                filled_by_app: self.filled_by_app,
                app: self.app.clone(),
            }
            .into_element(),
            FutureState::Fulfilled(Ok(loaded)) => ProjectLoaded {
                loaded: loaded.clone(),
                root: self.root.clone(),
                generation: self.generation,
                first_mount: self.first_mount,
                confirm: self.confirm,
                filled_by_app: self.filled_by_app,
                app: self.app.clone(),
            }
            .into_element(),
            FutureState::Fulfilled(Err(error)) => ProjectLoadFailed {
                root: self.root.clone(),
                error: error.clone(),
                confirm: self.confirm,
                filled_by_app: self.filled_by_app,
                app: self.app.clone(),
            }
            .into_element(),
        }
    }

    /// **The re-root mechanism.** The subtree's identity is the project folder, so opening
    /// another project in this window (`OpenPref::This`) diffs as a removal + an addition:
    /// the old project's scope is dropped — flushing its session, cancelling its tasks,
    /// dropping its engine and leaving the open-set — and the new one mounts through the very
    /// same hooks that run at launch. Without the key, Freya would keep the scope and its
    /// hooks, and every store would still hold the old project.
    fn render_key(&self) -> DiffKey {
        DiffKey::from(&(self.root.clone(), self.generation))
    }
}

/// [`ProjectRoot`]'s loaded arm: the open project proper, mounted with everything it needs
/// off disk already in hand. Its fields are the parent's, plus that [`Loaded`] value — the
/// store initializers consume it, so no hook here can fail. No `render_key`: the parent's
/// key is its identity.
struct ProjectLoaded {
    /// The defs and restored session the stores are built from.
    loaded: Rc<Loaded>,
    root: PathBuf,
    generation: u64,
    first_mount: bool,
    confirm: State<Option<CloseTarget>>,
    filled_by_app: State<bool>,
    app: AppCtx,
}

impl PartialEq for ProjectLoaded {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.loaded, &other.loaded)
            && self.root == other.root
            && self.generation == other.generation
            && self.first_mount == other.first_mount
            && self.confirm == other.confirm
            && self.filled_by_app == other.filled_by_app
            && self.app == other.app
    }
}

impl Component for ProjectLoaded {
    fn render(&self) -> impl IntoElement {
        let config = self.app.config;
        let guard = use_consume::<Arc<CloseGuard>>();
        let engine = use_provide_context({
            let running = guard.running.clone();
            let overrides = config.peek().settings.engine.clone();
            let root = self.root.clone();
            move || {
                let engine = EngineCtx::new(overrides);
                engine.watch_inflight(running);
                engine.set_data_dir(&root);
                engine
            }
        });
        let restart = use_consume::<EngineRestart>();
        use_provide_context({
            let project = self.root.to_string_lossy().into_owned();
            let generation = self.generation;
            move || Subtree {
                project,
                generation,
                restart,
            }
        });
        {
            let mut loaded = use_consume::<OpenCtx>().loaded;
            use_hook(move || loaded.set(true));
            use_drop(move || {
                let mut loaded = loaded;
                loaded.set(false);
            });
        }
        let log = use_init_log();
        use_init_faults();
        let project = use_init_project(&engine, log, self.root.clone(), self.loaded.clone());
        use_promote_recent(config, &project.peek().name, &self.root);
        use_init_session(self.loaded.clone());
        use_init_agents();
        use_agent_bridge(
            self.app.agent.clone(),
            self.root.clone(),
            project.peek().name.clone(),
        );
        use_provide_context({
            let assistant = self.app.assistant.clone();
            let directory = Arc::clone(&self.app.agent.directory);
            let root = self.root.to_string_lossy().into_owned();
            move || AssistantCtx {
                assistant,
                tools: StrataTools::in_app(directory),
                scope: Scope {
                    project: Some(root),
                },
            }
        });
        use_init_chats(
            seed_pick(&config.peek().settings.ai),
            self.root.clone(),
            chats_cap(config),
            use_report(),
        );
        let restored = self
            .first_mount
            .then(|| self.loaded.session.as_ref().and_then(|s| s.window))
            .flatten();
        use_autosave(restored, self.filled_by_app);
        use_init_history(config);
        use_diagnostics();
        use_engine_config(&engine, self.confirm);
        use_init_catalog_selection();
        let drop_target = use_provide_context(|| State::create(None::<DropTarget>));
        let chat_target = use_provide_context(|| State::create(None::<ChatDrop>));
        let profile_target = use_provide_context(|| State::create(None::<ProfileTarget>));
        let shape_target = use_provide_context(|| State::create(None::<ShapeTarget>));
        use_provide_context(|| State::create(None::<ConfigureTarget>));
        use_provide_context(|| State::create(None::<ConnectionTarget>));
        let schemas_target: SchemasRequest = use_provide_context(|| State::create(None::<String>));
        let palette_open: PaletteOpen = use_provide_context(|| State::create(false));

        let radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let mut known = use_state(HashSet::<TabId>::new);
        use_side_effect(move || {
            let open: HashSet<TabId> = radio.read().tabs.keys().copied().collect();
            for tab in known.peek().difference(&open) {
                engine.cleanup(*tab);
            }
            if *known.peek() != open {
                known.set(open);
            }
        });

        rect()
            .expanded()
            .vertical()
            .child(CloseConfirm {
                confirm: self.confirm,
                app: self.app.clone(),
            })
            .child(DropConfirm {
                target: drop_target,
            })
            .child(ChatConfirm {
                target: chat_target,
            })
            .child(ProfileConfirm {
                target: profile_target,
            })
            .child(ShapeDialog {
                target: shape_target,
            })
            .child(SchemasPicker {
                target: schemas_target,
            })
            .child(CommandPalette { open: palette_open })
            .child(ConfigureLauncher)
            .child(ConnectionLauncher)
            .child(RequestKeepers)
            .child(HeaderBar::new(self.filled_by_app))
            .child(Shell::new())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process;

    use freya_testing::TestingRunner;
    use strata_model::SessionSnapshot;

    use super::*;

    /// **The offloaded geometry read actually comes back** — the half a deadline can swallow in
    /// silence. A `select` that always resolved to its timer would lose every project's
    /// remembered window size and nothing in the app would look wrong, because a window opening
    /// at the default size is exactly what a fresh project does. Worth a test for that reason
    /// alone: the failure has no symptom other than the feature quietly not working.
    #[test]
    fn a_saved_geometry_survives_the_offloaded_read() {
        let root = std::env::temp_dir().join(format!("strata-geometry-{}", process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(project_io::strata_dir(&root)).unwrap();
        project_io::save_session(
            &root,
            &SessionSnapshot {
                window: Some(WindowGeom {
                    x: 120.,
                    y: 64.,
                    width: 1440.,
                    height: 900.,
                }),
                ..Default::default()
            },
        )
        .unwrap();

        let read = block_on(window_geometry(root.clone())).expect("the read answered in time");

        assert_eq!(
            (read.x, read.y, read.width, read.height),
            (120., 64., 1440., 900.)
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// [`ProjectRoot`]'s keying and nothing else — the mechanism the whole re-root rests on,
    /// with the stores and the engine replaced by a log of what mounted and what went.
    #[derive(PartialEq)]
    struct Keyed {
        root: String,
    }

    impl Component for Keyed {
        fn render(&self) -> impl IntoElement {
            let log = use_consume::<State<Vec<String>>>();
            let mine = use_state({
                let root = self.root.clone();
                move || root
            });
            use_hook(move || {
                let mut log = log;
                log.write().push(format!("mount {}", mine.peek()));
            });
            use_drop(move || {
                let mut log = log;
                log.write().push(format!("drop {}", mine.peek()));
            });
            rect()
        }

        fn render_key(&self) -> DiffKey {
            DiffKey::from(&self.root)
        }
    }

    fn app() -> impl IntoElement {
        let root = use_consume::<State<String>>();
        let root = root.read().clone();
        rect().child(Keyed { root })
    }

    /// **The re-root is a remount, not a re-render.** Writing the window's project root has
    /// to tear the old project's subtree down — its `use_drop`s are what flush the session
    /// and drop it from the open-set — and stand the new one up through the same hooks that
    /// run at launch. Freya only does that because the subtree is *keyed*; without the key it
    /// would keep the scope, and every store would still hold the old project.
    ///
    /// A characterization test as much as a unit one: it pins the two framework behaviours the
    /// design depends on — the keyed remount, and a `use_drop` still being able to read its
    /// scope's own state (which is what lets the outgoing project flush its session) — so a
    /// fork update that changed either fails here rather than silently leaving re-rooted
    /// windows showing the old project's tabs.
    #[test]
    fn changing_the_root_remounts_the_project_subtree() {
        let (mut runner, (root, log)) = TestingRunner::new(
            app,
            (200., 200.).into(),
            |r| {
                (
                    r.provide_root_context(|| State::create("/data/sales".to_string())),
                    r.provide_root_context(|| State::create(Vec::<String>::new())),
                )
            },
            1.,
        );
        runner.sync_and_update();
        assert_eq!(*log.peek(), ["mount /data/sales"]);

        let mut root = root;
        root.set("/data/ml_features".to_string());
        runner.sync_and_update();
        assert_eq!(
            *log.peek(),
            [
                "mount /data/sales",
                "drop /data/sales",
                "mount /data/ml_features",
            ]
        );

        root.set("/data/ml_features".to_string());
        runner.sync_and_update();
        assert_eq!(log.peek().len(), 3, "an unchanged root must not remount");
    }
}
