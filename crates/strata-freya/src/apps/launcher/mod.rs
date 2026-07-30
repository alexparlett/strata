//! The **launcher** window (P4-02 / design `Launcher.dc.html`) — Strata's welcome screen:
//! the app's entry point when no project is open, and where a project is picked.
//!
//! Its data is the app-global config store, live: the recents it lists and the pins it
//! writes are the same `AppConfig` every project window reads, so a project opening
//! anywhere re-orders this list without any cross-window plumbing of ours, and a pin here
//! is on screen in the header's switcher immediately (the Dioxus launcher's point-in-time
//! `config::load()` copy is exactly what let a window's `push_recent` and a pin overwrite
//! each other).
//!
//! Opening is [`crate::platform::open_project`] — the shared window path, so a project that
//! already has a window is *focused*, not opened twice. The launcher then closes: it exists
//! only while there is nothing else to look at.

mod model;
mod views;

use freya::prelude::*;
use freya::winit::platform::macos::WindowAttributesExtMacOS;
use strata_core::config::Command;
use strata_core::theme::os_is_dark;

use crate::agent::use_agent_server;
use crate::apps::launcher::views::{pick_and_open, LauncherRail, ProjectsPane, TitleBar};
use crate::keymap::on_commands;
use crate::menu::use_file_menu;
use crate::platform::{self, WindowKind};
use crate::state::{use_share_config, AppCtx};
use crate::theme::{peek_selection, use_strata_theme, window_background};

// `%[no_ext]`: the window's dress is read by four sibling views (title bar · rail · pane ·
// row) rather than by one `Launcher` component, so there's no type for the generated
// `…ThemePartialExt` builder to hang off.
define_theme!(
    %[no_ext]
    %[component]
    pub Launcher {
        %[fields]
        /// The card body — the title bar and the right pane.
        background: Color,
        /// The left rail's raised surface.
        rail_background: Color,
        /// The title-bar rule and the rail's right edge.
        border_fill: Color,
        /// "Welcome to Strata" and the ghost OPEN action.
        title_color: Color,
        /// PINNED / RECENT eyebrows and the version under the wordmark.
        label_color: Color,
        /// The active nav pill's accent tint (no left accent bar — V26). The rail rows'
        /// resting and hover fills are `sidebar_item`'s, which is the dress they wear.
        nav_background: Color,
        /// Hover for a project row.
        row_hover_background: Color,
        /// Hover for a row's Remove action (its glyph goes to the sheet's `error`).
        remove_hover_background: Color,
    }
);

/// The launcher's window: the canvas's 760×560 card, with the title bar drawn by
/// [`TitleBar`] rather than AppKit (same transparent-titlebar treatment as the project
/// window, so the traffic lights float in our own 38px strip).
pub struct LauncherApp {
    pub app: AppCtx,
}

impl LauncherApp {
    pub fn window(app: AppCtx) -> WindowConfig {
        // Match the theme's card body so a resize doesn't flash the default white. Pre-launch
        // there's no `Platform`, so the one-shot OS probe stands in for Sync-with-OS.
        let background = {
            let id = peek_selection(app.config, app.preview).effective(os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        WindowConfig::new_app(LauncherApp { app })
            .with_title("Welcome to Strata")
            .with_size(760., 560.)
            // The canvas's card is the minimum too: the rail is fixed at 200px and the rows
            // need room for a full path.
            .with_min_size(640., 460.)
            .with_background(background)
            // The 38px strip centres macOS's 16px buttons at y = 11; AppKit's default origin is
            // (7, 6), so the inset is the difference (the canvas's x = 14).
            .with_traffic_light_inset(7., 5.)
            .with_window_attributes(move |attrs, _| {
                attrs
                    .with_titlebar_transparent(true)
                    .with_fullsize_content_view(true)
                    .with_title_hidden(true)
            })
    }
}

impl App for LauncherApp {
    fn render(&self) -> impl IntoElement {
        // The same two window-root steps every app takes: this window's theme derived from
        // the shared settings, and the app-global config into context for the views below.
        use_strata_theme(self.app.themes.clone(), self.app.config, self.app.preview);
        use_share_config(self.app.config);
        use_provide_context({
            let app = self.app.clone();
            move || app
        });
        // Join the live window registry, so "open the launcher" finds this one instead of
        // opening a second, and a project window can tell whether it is the last one.
        platform::use_register_window(self.app.windows, || WindowKind::Launcher);
        // While this window is focused the File menu is *its* File menu: the recents it
        // lists, and neither Close Project nor an open path — there is no project here to
        // close, and nothing to open *into*, so a recent opens a window and this one stands
        // down.
        use_file_menu(&self.app, None);
        // The agent-access server's other reconciler. There is always at least one *workspace*
        // window alive — the launcher takes the last project's place — so mounting it on both
        // kinds is what makes the setting still live when every project is closed. Idempotent,
        // so the second window to run it does nothing (see `agent::server`).
        use_agent_server(self.app.agent.clone(), self.app.config);

        let theme = get_theme!(
            &None::<LauncherThemePartial>,
            LauncherThemePreference,
            "launcher"
        );
        let config = self.app.config;
        // Taken in the render scope so the key handler below can open a window from an event
        // handler, where there is no scope left to read it from.
        let platform = use_hook(Platform::get);

        rect()
            .expanded()
            .vertical()
            .content(Content::Flex)
            .background(theme.background)
            // The window's ambient text colour. Every run that doesn't name one — the
            // wordmark, the nav pill's label and its glyph — inherits it; without it they
            // fall back to Freya's base-theme default rather than this sheet's ramp.
            .color(use_theme().read().colors().text_primary)
            .child(TitleBar)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .horizontal()
                    .content(Content::Flex)
                    .child(LauncherRail)
                    .child(ProjectsPane {
                        app: self.app.clone(),
                    }),
            )
            // ⌘Q quits the app; the shortcuts whose targets aren't built yet are consumed
            // with a note so a press can't fall through to something else once they land.
            // Deliberately the LAST child — same-name global listeners fire in document
            // order, so every real consumer above outranks this catch-all.
            .child(rect().on_global_key_down(on_commands(config, {
                let app = self.app.clone();
                move |cmd| match cmd {
                    // ⌘O / File ▸ Open… — the same picker the OPEN action runs, which is
                    // why the menu item synthesizes this chord rather than acting itself.
                    Command::OpenProject => {
                        pick_and_open(app.clone());
                        true
                    }
                    // ⌘, — the same window the rail's Settings row opens, pinned above this
                    // one (or re-pinned here, if another window has it).
                    Command::OpenSettings => {
                        platform::open_settings(platform.clone(), app.clone());
                        true
                    }
                    Command::Quit => {
                        platform::quit();
                        true
                    }
                    Command::CommandPalette | Command::CycleWindow => {
                        tracing::debug!("launcher: shortcut {cmd:?} target not built yet (stub)");
                        true
                    }
                    _ => false,
                }
            })))
    }
}
