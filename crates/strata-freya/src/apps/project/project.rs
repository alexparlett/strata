//! The project window **root shell** (rail · sidebar · workbench · drawer).
//!
//! Initialises this window's per-window Session store + theme, spawns the engine into context
//! (ready for the freya-query layer), and mounts the real `Workbench` (editor). The tab strip
//! here is still the **throwaway** harness to create/switch tabs — the real DS strip is a later
//! slice.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crate::apps::project::close::{close_bridge, CloseBridge, CloseTarget, Veto};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{
    use_autosave, use_init_history, use_init_project, use_init_session, Chan, SessionState,
};
use crate::apps::project::views::{CloseConfirm, HeaderBar, Shell};
use crate::keymap::on_commands;
use crate::menu::use_file_menu;
use crate::platform::{self, WindowKind};
use crate::state::{use_config, use_open_project, use_share_config, AppCtx, ConfigChan};
use crate::theme::{use_strata_theme, window_background};
use freya::prelude::*;
use freya::radio::use_radio;
use freya::winit::dpi::LogicalPosition;
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use futures::StreamExt;
use strata_core::config::Command;
use strata_core::project as project_io;
use strata_core::theme::{effective_id, os_is_dark};
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
    /// The open project's folder — decided by the caller (`main`'s startup routing, or
    /// whoever opened this window) before the window exists, so its saved geometry can seed
    /// the window.
    pub root: PathBuf,
    /// The geometry the window was created with (`None` for a project that has never been
    /// saved). Handed to `use_autosave` as the seed for the last *normal* geometry, so a
    /// window filled before it is ever resized still persists a real size.
    pub geometry: Option<WindowGeom>,
}

impl ProjectApp {
    /// This window's config for `root` — the project folder, already chosen by the caller
    /// ([`crate::platform::open_project`] or `main`'s startup routing).
    pub fn window(app: AppCtx, root: PathBuf) -> WindowConfig {
        // Match the theme's window body so a resize doesn't flash the default white.
        // Pre-launch there's no `Platform`, so the one-shot OS probe stands in for
        // Sync-with-OS.
        let background = {
            let s = &app.config.peek().settings;
            let id = effective_id(&s.theme, s.sync_os, os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        // This window's close bridge: the hook holds an OS close while a query runs (and
        // the confirm pref is on), or while this is the last window and the launcher has
        // to come up first, and pings the UI either way.
        let (close, on_close) = close_bridge(app.config.peek().settings.confirm_close_running);
        // The project's saved geometry seeds the window — Freya has no runtime resize/move
        // from the app, so restore must happen at creation. A fresh / never-saved project
        // has no geometry yet → the built-in default size, OS-placed.
        let geom = project_io::load_session(&root)
            .ok()
            .flatten()
            .and_then(|snapshot| snapshot.window);
        // First-run default is roomy enough to show the whole rail · sidebar · workbench ·
        // inspector · drawer frame without cramping the workbench; a saved geometry (once the
        // window has been sized) wins, and `min_size` still honours the small-window story.
        let (width, height) = geom.map_or((1200., 780.), |g| (g.width as f64, g.height as f64));
        WindowConfig::new_app(ProjectApp {
            app,
            close,
            root,
            geometry: geom,
        })
        .with_title("Strata")
        .with_size(width, height)
        .with_min_size(880., 600.)
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
            match geom {
                Some(g) => attrs.with_position(LogicalPosition::new(g.x as f64, g.y as f64)),
                None => attrs,
            }
        })
    }
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
        use_strata_theme(themes.clone(), self.app.config);
        // The app-global config into context so deep consumers (shortcut listeners, keymap
        // hints, the confirm dialog's "don't ask again") reach it without prop-threading.
        // `RadioStation` is `Copy` — this shares the one global, it doesn't fork it.
        let config = self.app.config;
        use_share_config(config);

        // Join the app's live window registry for this window's lifetime: it's what makes
        // "this project is already open" a focus instead of a second window, and what tells
        // this window whether it is the last one.
        let windows = self.app.windows;
        platform::use_register_window(
            windows,
            WindowKind::Project(self.root.to_string_lossy().into_owned()),
        );
        // While this window is focused the File menu is *its* File menu: the recents, and
        // Close Project — which this window, unlike the launcher, has something to close.
        use_file_menu(self.app.menu, true);

        // ── The close bridge's UI half ─────────────────────────────────────────────────
        // The close guard + the confirm-dialog target into context (the workbench's ⌘W
        // gate needs both), then the mirrors and the veto drain.
        let guard = use_provide_context({
            let guard = self.close.guard.clone();
            move || guard
        });
        let mut confirm = use_provide_context(|| State::create(None::<CloseTarget>));
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
            move || platform::close_this_window(platform.clone(), app.clone())
        };
        use_hook({
            let close_window = close_window.clone();
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
        // Spawn this window's engine into context — the direct-call facade the query
        // layer's capabilities await (state-arch §7).
        let engine = use_provide_context(|| EngineCtx::new());
        // This window's Project store: opens the launch project (argv[1], default the
        // committed `sample/`) and registers its defs on the engine as a background
        // task — rows flip Loading → Ready/Failed as answers land (P4-13 internals;
        // the launcher / open-dialog UI is a later slice).
        let project = use_init_project(&engine, self.root.clone());
        // Register the project in the app-global config for this window's lifetime: it
        // heads the recents (so the launcher / project picker can offer it) and joins the
        // open-set (so they can tell open from merely recent) until the window closes.
        use_open_project(config, &project.peek().name, &self.root);
        // This window's Session store: restore the open project's `.strata/session.json`
        // (its tabs / order / active — P4-14), else one blank tab. Pulls the project root
        // from the store just provided above; provided via context.
        use_init_session();
        // Set while the header's double-press is what filled this window — the flag the session
        // reads to tell *our* transient fill from a window the user sized to the screen himself,
        // which does persist. Owned here because both the header and autosave need it.
        let filled_by_app = use_state(|| false);
        // Debounced autosave of that session back to `.strata/session.json` (P4-14). Its
        // subscription is inside the effect's own scope, so it never re-renders this root.
        // Seeded with the geometry the window opened at — it persists the last size the window
        // had while neither app-filled nor fullscreen (see the hook).
        use_autosave(self.geometry, filled_by_app);
        // The window's query-history satellite (P4-14): loads `.strata/history.jsonl` and
        // holds recent runs; the results pane appends to it as runs complete.
        use_init_history();

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
            .theme_background()
            .vertical()
            // The per-window context-menu host (provides the ROOT `ContextMenu` state + renders the
            // floating menu). Mounted high so the menu inherits the app's styling; hugs to nothing
            // until a menu is open, so it doesn't disturb the header / workbench layout.
            .child(ContextMenuViewer::new())
            // The close-while-running confirm (T2). Mounted second on purpose: while
            // open, its barrier consumes keys before every listener below it in document
            // order — including the ⌘Q/stub rect at the bottom, so the dialog can't be
            // re-triggered or bypassed from the keyboard.
            .child(CloseConfirm {
                confirm,
                app: self.app.clone(),
            })
            .child(HeaderBar::new(filled_by_app))
            .child(Shell::new())
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
                    // ⌘O / File ▸ Open… — pick a folder and open it in its own window.
                    // Which window it lands in is `OpenPref`'s question (this / new / ask),
                    // and that prompt is P4-13's; new-window is the honest default until then.
                    Command::OpenProject => {
                        crate::platform::pick_and_open_project(app.clone());
                        true
                    }
                    Command::CloseProject => {
                        // The same predicate as the on_close hook: red button, menu Close
                        // Project and ⇧⌘W share one dialog. Otherwise close now, bypassing
                        // the veto (this *is* the deliberate close) — through the shared
                        // path, so the launcher takes over if this was the last window.
                        if guard.running.load(Ordering::Relaxed)
                            && config.peek().settings.confirm_close_running
                        {
                            confirm.set(Some(CloseTarget::Window));
                        } else {
                            spawn(close_window());
                        }
                        true
                    }
                    // Quit closes every window — and, unlike closing them by hand, leaves
                    // the projects in the persisted open-set so the next launch reopens
                    // them. Each window's own close guard still gets its say.
                    Command::Quit => {
                        platform::quit();
                        true
                    }
                    Command::CommandPalette
                    | Command::OpenSettings
                    | Command::CycleWindow
                    | Command::Find => {
                        tracing::debug!("shortcut {cmd:?}: target not built yet (stub)");
                        true
                    }
                    _ => false,
                }
            })))
    }
}
