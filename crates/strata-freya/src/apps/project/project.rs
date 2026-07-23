//! The project window **root shell** (rail · sidebar · workbench · drawer).
//!
//! Initialises this window's per-window Session store + theme, spawns the engine into context
//! (ready for the freya-query layer), and mounts the real `Workbench` (editor). The tab strip
//! here is still the **throwaway** harness to create/switch tabs — the real DS strip is a later
//! slice.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use crate::apps::project::close::{close_bridge, CloseBridge, CloseTarget};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{use_init_project, use_init_session, Chan, SessionState, TabId};
use crate::apps::project::views::{CloseConfirm, HeaderBar, Workbench};
use crate::theme::ThemesCtx;
use futures::StreamExt;
use strata_core::config::{Command, Settings};
use freya::prelude::*;
use freya::radio::use_radio;
use freya::winit::platform::macos::WindowAttributesExtMacOS;

pub struct ProjectApp {
    /// The shared theme registry (discovered once in `main`, the same `Arc` in every
    /// window) and the app-global reactive [`Settings`] (any write repaints/reflows every
    /// window that reads the changed field). The window's theme is **derived** from the
    /// settings selection by [`use_strata_theme`] — no stored applied-theme id.
    ///
    /// [`use_strata_theme`]: crate::theme::use_strata_theme
    pub themes: ThemesCtx,
    pub settings: State<Settings>,
    /// The UI half of this window's close bridge (T2): the guard the winit `on_close`
    /// hook reads + the veto-signal receiver the root drains into the confirm dialog.
    pub close: CloseBridge,
}

impl ProjectApp {
    pub fn window(themes: ThemesCtx, settings: State<Settings>) -> WindowConfig {
        // Match the theme's window body so a resize doesn't flash the default white.
        // Pre-launch there's no `Platform`, so the one-shot OS probe stands in for
        // Sync-with-OS.
        let background = {
            let s = settings.peek();
            let id = strata_core::theme::effective_id(
                &s.theme,
                s.sync_os,
                strata_core::theme::os_is_dark(),
            );
            crate::theme::window_background(themes.get_or_default(&id))
        };
        // This window's close bridge (T2): the hook vetoes an OS close while a query
        // runs (and the confirm pref is on) and pings the UI to show the dialog.
        let (close, on_close) = close_bridge(settings.peek().confirm_close_running);
        WindowConfig::new_app(ProjectApp { themes, settings, close })
            .with_title("Strata")

            .with_size(880., 600.)
            .with_min_size(880., 600.)
            .with_background(background)
            .with_on_close(on_close)
            .with_window_attributes(|attrs, _| {
                attrs
                    .with_titlebar_transparent(true)
                    .with_fullsize_content_view(true)
                    .with_title_hidden(true)
            })
    }
}

impl App for ProjectApp {
    fn render(&self) -> impl IntoElement {
        // The shared theme registry into context (Settings' theme list, future switching),
        // then this window's theme resolved through it.
        let themes = use_provide_context({
            let themes = self.themes.clone();
            move || themes
        });
        // This window's theme: installed + kept derived from the reactive settings
        // selection (+ OS appearance while syncing). Every window computes the same pure
        // derivation of the same globals, so they repaint consistently.
        crate::theme::use_strata_theme(themes.clone(), self.settings);
        // The settings handle into context so deep consumers (shortcut listeners, keymap
        // hints) reach it without prop-threading. `State` is `Copy` — this shares the one
        // global, it doesn't fork it.
        let settings = self.settings;
        use_provide_context(move || settings);

        // ── T2: the close bridge's UI half ─────────────────────────────────────────────
        // The close guard + the confirm-dialog target into context (the workbench's ⌘W
        // gate needs both), then the two mirrors and the veto drain.
        let guard = use_provide_context({
            let guard = self.close.guard.clone();
            move || guard
        });
        let mut confirm = use_provide_context(|| State::create(None::<CloseTarget>));
        // Mirror the confirm-close-running pref into the hook's atomic (subscribes, so a
        // settings change reaches the next OS close immediately).
        {
            let guard = guard.clone();
            use_side_effect(move || {
                guard
                    .confirm
                    .store(settings.read().confirm_close_running, Ordering::Relaxed);
            });
        }
        // Drain the hook's veto pings into the dialog. The receiver is taken exactly
        // once; the task is scope-bound to this root.
        let rx = self.close.take_rx();
        use_hook(move || {
            if let Some(mut rx) = rx {
                spawn(async move {
                    while rx.next().await.is_some() {
                        confirm.set(Some(CloseTarget::Window));
                    }
                });
            }
        });
        // Spawn this window's engine into context — the direct-call facade the query
        // layer's capabilities await (state-arch §7).
        let engine = use_provide_context(|| EngineCtx::new());
        // This window's Project store: opens the launch project (argv[1], default the
        // committed `sample/`) and registers its defs on the engine as a background
        // task — rows flip Loading → Ready/Failed as answers land (P4-13 internals;
        // the launcher / open-dialog UI is a later slice).
        let _project = use_init_project(&engine);
        // This window's Session store (opens one blank tab), provided via context.
        let session = use_init_session();

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
            .child(CloseConfirm { confirm })
            .child(HeaderBar::new())
            .child(Workbench)
            // ⌘Q + the shortcuts whose targets aren't built yet (palette P6, settings
            // window + cycle-windows P4, find-in-results P2-09): the chords are live now —
            // consumed with a note, so a press can't fall through to something else once
            // those land. Deliberately the LAST child: same-name global listeners fire in
            // document (pre-order) order, so every real consumer — and the close-confirm
            // modal barrier — outranks this catch-all. (The root rect itself would fire
            // FIRST.)
            .child(rect().on_global_key_down(crate::keymap::on_commands(
                self.settings,
                move |cmd| match cmd {
                    Command::CloseProject => {
                        // The same predicate as the on_close hook: red button, dock quit
                        // and ⌘Q share one dialog. Otherwise close now, bypassing the
                        // veto (this *is* the deliberate close).
                        if guard.running.load(Ordering::Relaxed)
                            && settings.peek().confirm_close_running
                        {
                            confirm.set(Some(CloseTarget::Window));
                        } else {
                            Platform::get().close_current_window();
                        }
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
                },
            )))
    }
}
