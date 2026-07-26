//! The project window **root shell** (rail · sidebar · workbench · drawer), in two layers.
//!
//! [`ProjectApp`] is the **window**: its theme, the app-globals it shares into the tree, the
//! close bridge, the menubar it points at itself, and the open path that decides where the
//! next project lands. None of that changes when the window changes project.
//!
//! [`ProjectRoot`] is the **open project**: the engine, the Project / Session / History
//! stores, autosave, the catalog, and every feature view. It is **keyed on the project
//! folder**, so "open in this window" ([`OpenPref::This`](strata_core::config::OpenPref)) is
//! a plain `State` write — the key change unmounts this subtree (flushing the session,
//! dropping the engine, leaving the open-set) and mounts the next project exactly as launch
//! does. There is no reopen-in-place path to keep in step with the mount path, because they
//! are the same path.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::apps::project::close::{close_bridge, CloseBridge, CloseGuard, CloseTarget, Veto};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{
    use_autosave, use_init_catalog_selection, use_init_history, use_init_project, use_init_session,
    Chan, SessionState,
};
use crate::apps::project::views::{
    CloseConfirm, DropConfirm, DropTarget, HeaderBar, OpenPrompt, ProfileConfirm, ProfileTarget,
    Shell,
};
use crate::keymap::on_commands;
use crate::menu::use_file_menu;
use crate::platform::{self, OpenCtx, WindowKind};
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
    /// The project folder the window **opens at** — decided by the caller (`main`'s startup
    /// routing, or whoever opened this window) before the window exists, so its saved
    /// geometry can seed the window. The project the window *shows* is [`OpenCtx::root`]
    /// from here on, which starts as this and moves with an open-in-this-window.
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
        };
        use_provide_context(move || open);

        // Join the app's live window registry for this window's lifetime: it's what makes
        // "this project is already open" a focus instead of a second window, and what tells
        // this window whether it is the last one. Reactive on the open project, so a
        // re-rooted window is listed under what it actually shows.
        let windows = self.app.windows;
        platform::use_register_window(windows, move || {
            WindowKind::Project(open.root.read().to_string_lossy().into_owned())
        });
        // While this window is focused the File menu is *its* File menu: the recents, Close
        // Project — which this window, unlike the launcher, has something to close — and the
        // open path Open Recent resolves through.
        use_file_menu(&self.app, Some(open));

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
            let platform = platform.clone();
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
        // Set while the header's double-press is what filled this window — the flag the session
        // reads to tell *our* transient fill from a window the user sized to the screen himself,
        // which does persist. Owned here rather than in the project subtree because it is a fact
        // about the *window*: re-rooting doesn't unfill it.
        let filled_by_app = use_state(|| false);

        let root = open.root.read().clone();
        // The autosave seed is the launch project's alone — the window was *created* at that
        // geometry. A re-root leaves the window exactly where it is, so the project that
        // arrives simply records the geometry it finds itself at on its first save.
        let geometry = (root == self.root).then_some(self.geometry).flatten();

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
                geometry,
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
                let platform = platform.clone();
                move |cmd| match cmd {
                    // ⌘O / File ▸ Open… — pick a folder, then the open path decides which
                    // window it lands in (this one / a new one / ask).
                    Command::OpenProject => {
                        open.pick(platform.clone(), app.clone());
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

/// Everything that belongs to the **open project** rather than to the window: its engine,
/// its stores, its autosave, its catalog and every feature view.
///
/// Keyed on [`root`](Self::root) — see the module doc. Nothing in here is written to
/// re-open a project; it is only ever mounted at one.
#[derive(PartialEq)]
struct ProjectRoot {
    /// The project folder this subtree is standing up. **Its diff key**, so a different
    /// folder is a different subtree.
    root: PathBuf,
    /// The geometry the window was created at, when this is the project it was created for
    /// — the autosave seed. `None` for a project opened into an existing window.
    geometry: Option<WindowGeom>,
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
            move || {
                let engine = EngineCtx::new();
                engine.watch_inflight(running);
                engine
            }
        });
        // This project's store: loads `.strata/project.json` (scaffolding one when the folder
        // has none) and registers its defs on the engine as a background task — rows flip
        // Loading → Ready/Failed as answers land.
        let project = use_init_project(&engine, self.root.clone());
        // Register the project in the app-global config for as long as this subtree lives: it
        // heads the recents (so the launcher / project picker can offer it) and joins the
        // open-set (so they can tell open from merely recent) until the window closes — or
        // until it opens something else, which drops this entry and adds that one.
        use_open_project(config, &project.peek().name, &self.root);
        // This project's Session store: restore its `.strata/session.json` (tabs / order /
        // active / layout), else one blank tab. Pulls the root from the store above.
        use_init_session();
        // Debounced autosave of that session back to `.strata/session.json`. Its subscription
        // is inside the effect's own scope, so it never re-renders this root; its `use_drop`
        // is what makes a close — or a re-root — keep the last few hundred milliseconds.
        use_autosave(self.geometry, self.filled_by_app);
        // The project's query-history satellite: loads `.strata/history.jsonl` and holds
        // recent runs (capped by `Settings::max_history`, hence the config); the results pane
        // appends to it as runs complete.
        use_init_history(config);
        // The inspected-column slot (P3-02): the catalog sidebar writes it, the inspector
        // (P3-08) reads it. A context signal, not a store — see `state/catalog.rs`.
        use_init_catalog_selection();
        // The drop-confirm slot (P3-05): the row a drop is being confirmed for. Provided here
        // like the close target above, because the dialog is mounted at this root and its
        // trigger is elsewhere — a catalog row's context menu sets it (P3-06).
        let drop_target = use_provide_context(|| State::create(None::<DropTarget>));
        // The profile-cost slot (P3-10), on the same terms: the entry a *first* scan is being
        // confirmed for. Its triggers are the catalog row menus and the inspector's scan card;
        // a re-scan never fills it (`ProfileActions::ask`).
        let profile_target = use_provide_context(|| State::create(None::<ProfileTarget>));

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
            // The profile-cost confirm (P3-10). Last of the three, in the order their questions
            // outrank each other: a running query, then a destructive catalog change, then a
            // question about work the user is about to start.
            .child(ProfileConfirm {
                target: profile_target,
            })
            .child(HeaderBar::new(self.filled_by_app))
            .child(Shell::new())
    }

    /// **The re-root mechanism.** The subtree's identity is the project folder, so opening
    /// another project in this window (`OpenPref::This`) diffs as a removal + an addition:
    /// the old project's scope is dropped — flushing its session, cancelling its tasks,
    /// dropping its engine and leaving the open-set — and the new one mounts through the very
    /// same hooks that run at launch. Without the key, Freya would keep the scope and its
    /// hooks, and every store would still hold the old project.
    fn render_key(&self) -> DiffKey {
        DiffKey::from(&self.root)
    }
}

#[cfg(test)]
mod tests {
    use freya_testing::TestingRunner;

    use super::*;

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
