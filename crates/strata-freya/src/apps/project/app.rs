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
    ProjectLoadFailed, ProjectLoading, RequestKeepers, ShapeDialog, ShapeTarget, Shell,
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
        // Match the theme's window body so a resize doesn't flash the default white.
        // Pre-launch there's no `Platform`, so the one-shot OS probe stands in for
        // Sync-with-OS.
        let background = {
            let id = peek_selection(app.config, app.preview).effective(os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        // This window's close bridge: the hook holds an OS close while a query runs (and
        // the confirm pref is on), or while this is the last window and the launcher has
        // to come up first, and pings the UI either way.
        let (close, on_close) = close_bridge(app.config.peek().settings.confirm_close_running);
        // First-run default is roomy enough to show the whole rail · sidebar · workbench ·
        // inspector · drawer frame without cramping the workbench; a saved geometry (once the
        // window has been sized) wins. A project that has never been saved — or whose geometry
        // could not be read in time — opens here, OS-placed.
        let (width, height) = geometry.map_or((1200., 780.), |g| (g.width as f64, g.height as f64));
        WindowConfig::new_app(ProjectApp { app, close, root })
            .with_title("Strata")
            .with_size(width, height)
            // A nominal stop, not a usability claim: the shell has no minimum worth the name, and
            // squeezing it is meant to degrade (panels give in order, chrome folds into its
            // overflow menu) rather than be refused. Below roughly this the header bar is shorter
            // than its own traffic-light gutter and there is nothing left to lay out.
            .with_min_size(360., 240.)
            .with_background(background)
            .with_on_close(on_close)
            // Offset from AppKit's default (≈7, 6): close button lands at (13, 16) —
            // x matches the Dioxus app's placement, y centers the 16px buttons in the
            // 48px header bar.
            .with_traffic_light_inset(6., 10.)
            .with_window_attributes(move |attrs, _| {
                let attrs = attrs
                    .with_titlebar_transparent(true)
                    .with_fullsize_content_view(true)
                    .with_title_hidden(true);
                // Reopen where it was last left; a fresh project lets the OS place it.
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
        // The shared theme registry into context (Settings' theme list, future switching),
        // then this window's theme resolved through it.
        let themes = use_provide_context({
            let themes = self.app.themes.clone();
            move || themes
        });
        // This window's theme: installed + kept derived from the reactive settings
        // selection (+ OS appearance while syncing). Every window computes the same pure
        // derivation of the same globals, so they repaint consistently.
        use_strata_theme(themes, self.app.config, self.app.preview);
        // The app-global config into context so deep consumers (shortcut listeners, keymap
        // hints, the confirm dialog's "don't ask again") reach it without prop-threading.
        // `RadioStation` is `Copy` — this shares the one global, it doesn't fork it.
        let config = self.app.config;
        use_share_config(config);
        // The app-globals bundle into context too: they are DI handles, so deep consumers
        // (the header's project switcher) reach them without prop-threading through the shell.
        use_provide_context({
            let app = self.app.clone();
            move || app
        });

        // ── The close bridge's UI half ─────────────────────────────────────────────────
        // The close guard + the confirm-dialog target into context (the workbench's ⌘W
        // gate needs both, and so does the open path's re-root gate), then the mirrors and
        // the veto drain.
        let guard = use_provide_context({
            let guard = self.close.guard.clone();
            move || guard
        });
        let mut confirm = use_provide_context(|| State::create(None::<CloseTarget>));

        // The engine generation this window is on. A window fact (like the fill flag below):
        // the thing it keys must survive the remount it causes. Stood up before the open
        // path, which carries it as the retry mechanism for a faulted window.
        let engine_restart = use_engine_restart();

        // This window's **open path**: the project it shows, the This/New question it is
        // asking, and the close-while-running gate a re-root has to pass — opening in place
        // aborts whatever is executing, exactly as closing the window would. Window-scoped
        // rather than part of the project subtree, precisely because writing `root` is what
        // replaces that subtree. Into context for the header switcher and the confirm dialog.
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

        // Join the app's live window registry for this window's lifetime: it's what makes
        // "this project is already open" a focus instead of a second window, and what tells
        // this window whether it is the last one. Reactive on the open project, so a
        // re-rooted window is listed under what it actually shows.
        //
        // The same call points the menubar here while this window is focused. A project window
        // is the one scope where every File and Window item applies — and it hands over its
        // open path, so Open Recent honours this window's "Opening a project" preference.
        // …and it hands back this window's own id, which is the one thing a window cannot
        // learn any other way and `Command::CycleWindow` needs to name itself in the ring.
        let window_id = use_register_window(
            &self.app,
            move || WindowKind::Project(open.root.read().to_string_lossy().into_owned()),
            MenuScope::Project(open),
        );
        // Keep the agent-access server in step with its setting. On the **window** layer, not
        // the project subtree: a re-root or an engine restart must not stop a server the app
        // is running, and this window is one of the two kinds that is always around to
        // reconcile it (see `agent::server`).
        use_agent_server(self.app.agent.clone(), config);
        // …and the updater's one startup check, on the same layer and for the same reason
        // (`state::updates`): it is app-global, and this is one of the two window kinds that is
        // always around to run it.
        use_updates(self.app.updates, config);

        // Mirror the confirm-close-running pref into the hook's atomic (subscribes to the
        // config's Settings channel, so a change reaches the next OS close immediately).
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
        // …and whether this is the app's last window, which a window opening or closing
        // anywhere changes.
        {
            let guard = guard.clone();
            let windows = self.app.windows;
            use_side_effect(move || {
                guard
                    .last
                    .store(windows.read().is_last(), Ordering::Relaxed);
            });
        }
        // Drain the hook's vetoes: a running query raises the T2 dialog, and the last
        // window out puts the launcher up before it goes. The receiver is taken exactly
        // once; the task is scope-bound to this root.
        let rx = self.close.take_rx();
        // The one deliberate-close path (⇧⌘W, the confirm's "Stop & exit", and the OS close
        // once its veto lands here): the launcher takes this window's place when it is the
        // last one. The window handle is taken here, in the render scope, so the closure can
        // be called from a task.
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
        // Set while the header's double-press is what filled this window — the flag the session
        // reads to tell *our* transient fill from a window the user sized to the screen himself,
        // which does persist. Owned here rather than in the project subtree because it is a fact
        // about the *window*: re-rooting doesn't unfill it.
        let filled_by_app = use_state(|| false);

        let root = open.root.read().clone();
        // Whether this is the project the window was *created* for, on its first mount — which is
        // the only case with an autosave seed to offer (`ProjectLoaded` takes it from the session
        // it loaded). A re-root leaves the window exactly where it is, so the project that
        // arrives simply records the geometry it finds itself at on its first save; an engine
        // restart is the same case even though the folder has not changed, because by then the
        // window may have been moved or resized. Seeding either would resurrect a geometry this
        // window was never at, the next time it saves while filled (the pass that keeps the last
        // *non*-transient geometry rather than reading the screen).
        let first_mount = root == self.root && engine_restart.generation() == 0;

        rect()
            .expanded()
            .theme_background()
            .vertical()
            // The per-window context-menu host (provides the ROOT `ContextMenu` state + renders the
            // floating menu). Mounted high so the menu inherits the app's styling; hugs to nothing
            // until a menu is open, so it doesn't disturb the header / workbench layout.
            .child(ContextMenuViewer::new())
            // The open-target prompt (B10). Above the project subtree in document order, so
            // while it is up its key barrier precedes every feature listener — Esc answers the
            // question rather than cancelling the query behind it.
            .child(OpenPrompt {
                open,
                app: self.app.clone(),
            })
            .child(ProjectRoot {
                root,
                generation: engine_restart.generation(),
                first_mount,
                confirm,
                filled_by_app,
                app: self.app.clone(),
            })
            // Window lifecycle + the shortcuts whose targets aren't built yet (palette P6,
            // settings window + cycle-windows P4, find-in-results P2-09): the chords are
            // live now — consumed with a note, so a press can't fall through to something
            // else once those land. Deliberately the LAST child: same-name global listeners
            // fire in document (pre-order) order, so every real consumer — and the
            // close-confirm modal barrier — outranks this catch-all. (The root rect itself
            // would fire FIRST.)
            .child(rect().on_global_key_down(on_commands(config, {
                let app = self.app.clone();
                move |cmd| match cmd {
                    // ⌘O / File ▸ Open… — pick a folder, then the open path decides which
                    // window it lands in (this one / a new one / ask).
                    Command::OpenProject => {
                        open.pick(platform.clone(), app.clone());
                        true
                    }
                    Command::CloseProject => {
                        // The same predicate as the on_close hook: red button, menu Close
                        // Project, ⇧⌘W and the palette's Close project share one dialog and
                        // one gate (`close::close_project`). Otherwise it closes now,
                        // bypassing the veto — this *is* the deliberate close — through the
                        // shared path, so the launcher takes over if this was the last window.
                        close_project(&guard, config, confirm, platform.clone(), app.clone());
                        true
                    }
                    // Quit closes every window — and, unlike closing them by hand, leaves
                    // the projects in the persisted open-set so the next launch reopens
                    // them. Each window's own close guard still gets its say.
                    Command::Quit => {
                        quit();
                        true
                    }
                    // ⌘, — the same window the header's gear opens, pinned above this one
                    // (or re-pinned here, if another window has it).
                    Command::OpenSettings => {
                        open_settings(platform.clone(), app.clone());
                        true
                    }
                    // ⌘K is gone from here: the palette owns it, from a node inside the project
                    // subtree that fires first. The two engineless arms have nothing to search,
                    // so it does nothing there — which is the honest answer, not a stub.
                    //
                    // ⌘` — move focus to the next workspace window. Declines (falls through)
                    // when this is the only one, since there is nowhere to go; the menubar
                    // greys Window ▸ Cycle Windows on the same fact.
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
        // **The window's claim on this project, whatever the load turns out to say.** Here rather
        // than in an arm because it is true of the mount and not of the outcome: a window
        // loading a project, showing one, or reporting that it could not be loaded is in every
        // case a window on that project, which is what makes a quit reopen it and a deliberate
        // close drop it from reopen-on-startup. Hoisting it also settles the question an arm
        // could not: two arms with an add-on-mount / remove-on-drop pair apiece would depend on
        // which way the diff orders the swap between them. The recents promotion stays in the
        // loaded arm, because that half is earned rather than claimed.
        use_claim_open(self.app.config, &self.root);

        // Once per mount: the subtree is keyed on (folder, generation), so a re-root into a
        // broken project — or an engine restart, or the fault dialog's Try again — re-runs the
        // load in a fresh scope. Detection therefore lives at every way a project arrives in a
        // window, not only at launch.
        //
        // `use_future` rather than a hand-rolled state + `spawn`: `FutureState`'s three cases
        // *are* the three arms, the task is scope-bound (a remount abandons the read in flight
        // rather than letting it write into a subtree that has gone), and the read itself sits
        // on a thread of its own — see `load_project`.
        let load = use_future({
            let root = self.root.clone();
            move || load_project(root.clone())
        });
        match &*load.state() {
            // Pending is the first render, before the task has been polled; Loading is the read
            // out on its thread. Nothing distinguishes them to a user, and neither has anything
            // loaded, so they are one arm.
            FutureState::Pending | FutureState::Loading => ProjectLoading {
                root: self.root.clone(),
                confirm: self.confirm,
                filled_by_app: self.filled_by_app,
                app: self.app.clone(),
            }
            .into_element(),
            // The `Rc` is cloned per render, not the catalog behind it — and it is minted once
            // per mount, which is what lets `ProjectLoaded` compare by pointer.
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
        // `loaded` by pointer identity: it is built exactly once per (root, generation)
        // mount, so two values are equal iff they are the same allocation — a deep compare
        // of every def and tab buffer could only answer the same thing more slowly.
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
        // Spawn this project's engine into context — the direct-call facade the query
        // layer's capabilities await (state-arch §7) — and hand it the close guard's
        // in-flight flag on the way. The engine is the only thing that knows what is
        // executing across *all* tabs (the UI mounts only the active one's results), and
        // the `on_close` hook can read nothing but an atomic; from here on the engine
        // publishes into it on every dispatch / settle / cancel / cleanup. A re-root drops
        // the old engine, whose `Drop` clears that same flag before the new one takes over.
        let guard = use_consume::<Arc<CloseGuard>>();
        let engine = use_provide_context({
            let running = guard.running.clone();
            // The app's `datafusion.*` overrides are a launch value (Settings ▸ Engine, W2): the
            // `RuntimeEnv` half is fixed the moment the context is built, so an engine is only
            // ever *born* with a full set. `use_engine_config` below keeps the rest in step.
            let overrides = config.peek().settings.engine.clone();
            let root = self.root.clone();
            move || {
                let engine = EngineCtx::new(overrides);
                engine.watch_inflight(running);
                // Which project this engine belongs to — where a `CREATE TABLE` spools its data
                // and what an internal def's source path is relative to (ED-04). A launch value
                // like the overrides, and for a stronger reason: the subtree is *keyed* on the
                // folder, so a re-root builds a new engine rather than re-pointing this one.
                engine.set_data_dir(&root);
                engine
            }
        });
        // **What this subtree is, for the windows that borrow from it.** Its two halves are the
        // diff key above, so a value built here is only ever true of the mount that built it —
        // which is exactly what a child window holding these handles has to be able to check
        // (`platform::owner`). Provided before the handles themselves, so nothing can be handed
        // out without it.
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
        // The loaded arm tells the *window* that its subtree is up — the mirror of the fault
        // arm's own flag, and mount/drop for the same reason: the flag belongs to `OpenCtx`,
        // which outlives every arm. What reads it is the menubar, which is built above this
        // subtree and so cannot otherwise tell a window with a workbench in it from one showing
        // a load error: File ▸ New Query and Save Query have their listeners *here*, while
        // Close Project and Open… are the window's and work in every arm (`menu::MenuScope`).
        {
            let mut loaded = use_consume::<OpenCtx>().loaded;
            use_hook(move || loaded.set(true));
            use_drop(move || {
                let mut loaded = loaded;
                loaded.set(false);
            });
        }
        // This project's event log (P3-13) — the drawer's Events tab. First, because the open
        // below is its first entry: every later observer (Save, the drop confirm, a tab's request
        // keeper) reaches it from context.
        let log = use_init_log();
        // And which of its `.strata` files are currently behind the screen (P4-15) — the standing
        // half of the same report, behind the Problems drawer's Project tab. Stood up beside the
        // log because every writer that appends to one records into the other.
        use_init_faults();
        // This project's store, from the defs the load already read, and the engine
        // registration pass over them as a background task — rows flip Loading →
        // Ready/Failed as answers land, and each answer is recorded in the log.
        let project = use_init_project(&engine, log, self.root.clone(), self.loaded.clone());
        // Now that it has actually opened, the project heads the recents — the half of the config
        // claim a project earns by loading, as against the open-set claim every arm of the
        // subtree makes (`ProjectRoot`). It stays there after the window goes, which is the whole
        // point of a recent.
        use_promote_recent(config, &project.peek().name, &self.root);
        // This project's Session store, from the snapshot the load already restored (tabs /
        // order / active / layout), else one blank tab.
        use_init_session(self.loaded.clone());
        // Which agents are working in this project and what they hold (AA-03b) — the window's
        // own bookkeeping, stood up before the bridge that records into it.
        use_init_agents();
        // Lend this project to the agent-access service directory for as long as *this mount*
        // lasts, and drive the asks that come back (AA-03). Here rather than on the window
        // layer because everything it lends — the engine, the two stores, the log — belongs to
        // the mount: a re-root or an engine restart has to deregister and re-register, and
        // mounting it here is what makes that the same path an open and a close take.
        use_agent_bridge(
            self.app.agent.clone(),
            self.root.clone(),
            project.peek().name.clone(),
        );
        // The assistant's own handles (AS-04). Its `StrataTools` is minted **once per mount**
        // and `in_app`, so every conversation in this window is the same agent holding the same
        // query sessions — and the close confirm names it as the assistant by construction
        // rather than by comparing an identity. It reaches this project the way any agent
        // does: through the
        // directory, scoped by the project **root**, which is the identity a name may collide
        // with.
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
        // This window's conversations, seeded from Settings' defaults through the one funnel
        // that drops a provider which is no longer enabled. The stored ones load here too
        // (AS-07): heads only, rotated down to the user's cap, with the transcripts read when a
        // switcher row is actually pressed.
        use_init_chats(
            seed_pick(&config.peek().settings.ai),
            self.root.clone(),
            chats_cap(config),
            use_report(),
        );
        // Debounced autosave of that session back to `.strata/session.json`. Its subscription
        // is inside the effect's own scope, so it never re-renders this root; its `use_drop`
        // is what makes a close — or a re-root — keep the last few hundred milliseconds.
        //
        // The geometry seed is **this session's own**, and only on the window's first mount (see
        // `ProjectApp::render`): the same field `window_geometry` read to place the window, but
        // taken from the load rather than from that read, which has a deadline and may have come
        // back empty. Seeding `None` there would let the first save replace a perfectly good
        // remembered size with whatever default the window opened at.
        let restored = self
            .first_mount
            .then(|| self.loaded.session.as_ref().and_then(|s| s.window))
            .flatten();
        use_autosave(restored, self.filled_by_app);
        // The project's query-history satellite: loads `.strata/history.jsonl` and holds
        // recent runs (capped by `Settings::max_history`, hence the config); the results pane
        // appends to it as runs complete.
        use_init_history(config);
        // The window's one validation driver: every open tab's diagnostics kept in step with
        // its text and the catalog (`state/diagnostics.rs`). Mounted here, after the engine,
        // the project (which provides the catalog state it gates on) and the session, because
        // it reconciles all three. Nothing else in the app writes diagnostics.
        use_diagnostics();
        // Keep that engine pointed at the app's engine overrides for as long as the project is
        // open: a `ConfigOptions` change lands on the live session, and a changed
        // `datafusion.runtime.*` asks for a restart through this window's own close confirm.
        use_engine_config(&engine, self.confirm);
        // The inspected-column slot (P3-02): the catalog sidebar writes it, the inspector
        // (P3-08) reads it. A context signal, not a store — see `state/catalog.rs`.
        use_init_catalog_selection();
        // The drop-confirm slot (P3-05): the row a drop is being confirmed for. Provided here
        // like the close target above, because the dialog is mounted at this root and its
        // trigger is elsewhere — a catalog row's context menu sets it (P3-06).
        let drop_target = use_provide_context(|| State::create(None::<DropTarget>));
        // The chat pane's own destructive questions (AS-07), in a slot for the same reason the
        // catalog's are: a confirm mounted inside the pane it belongs to is a key barrier over
        // nothing, because listeners fire in document order.
        let chat_target = use_provide_context(|| State::create(None::<ChatDrop>));
        // The profile-cost slot (P3-10), on the same terms: the entry a *first* scan is being
        // confirmed for. Its triggers are the catalog row menus and the inspector's scan card;
        // a re-scan never fills it (`ProfileActions::ask`).
        let profile_target = use_provide_context(|| State::create(None::<ProfileTarget>));
        // The Shape panel's slot (Chart 09), on the same terms: the settled run the composer
        // is open over. Its trigger is the results toolbar's Shape action, on both bodies.
        let shape_target = use_provide_context(|| State::create(None::<ShapeTarget>));
        // The Configure-window request slot (P4-11) — the same shape as the two above, though
        // what it opens is a window rather than a dialog. Its triggers (a catalog row's
        // Configure, the TABLES section's `+`) set it and stop; `ConfigureLauncher` below holds
        // the app-globals and the engine a window needs, so no row has to.
        use_provide_context(|| State::create(None::<ConfigureTarget>));
        // The connection-editor request slot (W7 · 03), on identical terms — set by the
        // Connections pane's `+`, its empty-state CTA and a row's Edit, acted on by
        // `ConnectionLauncher` below.
        use_provide_context(|| State::create(None::<ConnectionTarget>));
        // Whether the command palette is up (P6-01). A slot on the same terms as the three
        // above: the surface is mounted at this root, where every store it acts through
        // actually lives, and its other trigger is elsewhere — the header's ⌘K button.
        let palette_open: PaletteOpen = use_provide_context(|| State::create(false));

        // Tab-close cleanup (SNAPSHOT_SPEC §4): diff the open tab set on every
        // structural change and retire the engine state of tabs that are gone. One
        // funnel for every close path (close / close-others / close-right / close-all);
        // a reopened tab simply starts with no engine state, like a fresh one.
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
            // The close-while-running confirm (T2). Mounted first on purpose: while
            // open, its barrier consumes keys before every listener below it in document
            // order — including the ⌘Q/stub rect at the window root, so the dialog can't be
            // re-triggered or bypassed from the keyboard.
            .child(CloseConfirm {
                confirm: self.confirm,
                app: self.app.clone(),
            })
            // The catalog drop confirm (P3-05), on the same terms as the close confirm above
            // and after it: if both were somehow open, the running-query question outranks the
            // catalog one in document order.
            .child(DropConfirm {
                target: drop_target,
            })
            // The chat pane's delete / clear (AS-07), beside the catalog's and after it: both
            // destroy a project's work, and the catalog's question is about something the engine
            // is holding.
            .child(ChatConfirm {
                target: chat_target,
            })
            // The profile-cost confirm (P3-10). Last of the three, in the order their questions
            // outrank each other: a running query, then a destructive catalog change, then a
            // question about work the user is about to start.
            .child(ProfileConfirm {
                target: profile_target,
            })
            // The Shape panel (Chart 09) — a working modal, not a confirm, so it sits after
            // every question that could outrank it and before the palette, whose barrier
            // must not swallow this panel's keys while it is up.
            .child(ShapeDialog {
                target: shape_target,
            })
            // The command palette (P6-01). Under the three confirms, because a question about
            // work in flight outranks a search box, and above every feature, so while it is up
            // its barrier precedes their listeners in document order. It draws only its ⌘K
            // listener until it is opened.
            .child(CommandPalette { open: palette_open })
            // Not a dialog and not a barrier: it draws nothing and only watches the request
            // slot. Mounted here because this is where the handles opening a window needs
            // actually live.
            .child(ConfigureLauncher)
            // The connection editor's, on the same terms and for the same reason.
            .child(ConnectionLauncher)
            // Invisible, zero-size: every open tab's current press keeps a query
            // subscriber mounted for this project's whole life, so backgrounded runs
            // neither lose their cache entry nor miss their history settle. Root-level
            // on purpose — the invariant is session-scoped, like the tab funnel above,
            // not a property of whichever layout shows the workbench (see `views::keeper`).
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
            // A scope-owned `State` read back in the drop — what the session flush does with
            // the Project and Session stations. It works because `ScopeStorage` declares
            // `values` (where the `use_drop` guard lives) before `owner` (where the state's
            // value lives), so the guard runs while the value is still there. That is a field
            // order in the fork, so it is worth a test rather than a comment.
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

        // The re-root: exactly what `OpenCtx::reroot` does.
        let mut root = root;
        root.set("/data/ml_features".to_string());
        runner.sync_and_update();
        assert_eq!(
            *log.peek(),
            [
                "mount /data/sales",
                // The outgoing project goes first — which is what lets its session flush
                // land before the arriving one touches anything.
                "drop /data/sales",
                "mount /data/ml_features",
            ]
        );

        // A render that does *not* change the root leaves the subtree alone: a re-root must
        // cost a remount, but nothing else may.
        root.set("/data/ml_features".to_string());
        runner.sync_and_update();
        assert_eq!(log.peek().len(), 3, "an unchanged root must not remount");
    }
}
