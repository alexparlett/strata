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
use crate::menu::MenuScope;
use crate::platform::{self, WindowKind};
use crate::state::{use_share_config, use_updates, AppCtx};
use crate::theme::{peek_selection, use_roles, use_strata_theme, window_background, Role};
use crate::updater::{UpdateAsk, UpdateConfirm};

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
        /// Hover for a row's Remove action (its glyph goes to the shared ramp's `error`).
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
        let background = {
            let id = peek_selection(app.config, app.preview).effective(os_is_dark());
            window_background(app.themes.get_or_default(&id))
        };
        WindowConfig::new_app(LauncherApp { app })
            .with_title("Welcome to Strata")
            .with_size(760., 560.)
            .with_min_size(640., 460.)
            .with_background(background)
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
        use_strata_theme(self.app.themes.clone(), self.app.config, self.app.preview);
        use_share_config(self.app.config);
        use_provide_context({
            let app = self.app.clone();
            move || app
        });
        let update_ask = use_provide_context(|| State::create(None::<UpdateAsk>));
        platform::use_register_window(
            &self.app,
            || WindowKind::Launcher,
            MenuScope::Launcher(update_ask),
        );
        use_agent_server(self.app.agent.clone(), self.app.config);
        use_updates(self.app.updates, self.app.config);

        let theme = get_theme!(
            &None::<LauncherThemePartial>,
            LauncherThemePreference,
            "launcher"
        );
        let config = self.app.config;
        let platform = use_hook(Platform::get);

        rect()
            .expanded()
            .vertical()
            .content(Content::Flex)
            .background(theme.background)
            .color(use_roles().get(Role::Text))
            .child(UpdateConfirm {
                ask: update_ask,
                status: self.app.updates,
            })
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
            .child(rect().on_global_key_down(on_commands(config, {
                let app = self.app.clone();
                move |cmd| match cmd {
                    Command::OpenProject => {
                        pick_and_open(app.clone());
                        true
                    }
                    Command::OpenSettings => {
                        platform::open_settings(platform.clone(), app.clone());
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
