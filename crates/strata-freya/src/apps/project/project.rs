//! The project window **root shell** (rail · sidebar · workbench · drawer).
//!
//! Initialises this window's per-window Session store + theme, spawns the engine into context
//! (ready for the freya-query layer), and mounts the real `Workbench` (editor). The tab strip
//! here is still the **throwaway** harness to create/switch tabs — the real DS strip is a later
//! slice.

use std::collections::HashSet;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{use_init_session, Chan, SessionState, TabId};
use crate::apps::project::views::{HeaderBar, Workbench};
use crate::theme::ThemesCtx;
use strata_core::config::Settings;
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
        WindowConfig::new_app(ProjectApp { themes, settings })
            .with_title("Strata")

            .with_size(880., 600.)
            .with_min_size(880., 600.)
            .with_background(background)
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
        // Spawn this window's engine into context — the direct-call facade the query
        // layer's capabilities await (state-arch §7).
        let engine = use_provide_context(|| EngineCtx::new());
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
            .child(HeaderBar::new())
            .child(Workbench)
    }
}
