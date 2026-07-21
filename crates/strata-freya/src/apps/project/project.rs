//! The project window **root shell** (rail · sidebar · workbench · drawer).
//!
//! Initialises this window's per-window Session store + theme, spawns the engine into context
//! (ready for the freya-query layer), and mounts the real `Workbench` (editor). The tab strip
//! here is still the **throwaway** harness to create/switch tabs — the real DS strip is a later
//! slice.

use crate::apps::project::contexts::engine_ctx::EngineCtx;
use crate::apps::project::state::use_init_session;
use crate::apps::project::views::{HeaderBar, Workbench};
use freya::prelude::*;
use freya::winit::platform::macos::WindowAttributesExtMacOS;

pub struct ProjectApp;

impl ProjectApp {
    pub fn window() -> WindowConfig {
        WindowConfig::new_app(ProjectApp)
            .with_title("Strata")

            .with_size(880., 600.)
            .with_min_size(880., 600.)
            // Match the Midnight window body so a resize doesn't flash the default white.
            .with_background(Color::from_rgb(21, 24, 30))
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
        use_init_theme(|| crate::theme::strata_theme("midnight"));
        // Spawn this window's engine into context, ready for the query layer to consume.
        use_provide_context(|| EngineCtx::new());
        // This window's Session store (opens one blank tab), provided via context.
        let session = use_init_session();

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
