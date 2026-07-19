//! The project window **root shell** (rail · sidebar · workbench · drawer). Phase 1a: a
//! placeholder body; 1b mounts the workbench (SQL input → Run → rows) on the shared
//! engine, and later phases add the rail, sidebar, inspector, and drawer around it.

use freya::prelude::*;

pub struct ProjectApp;

impl App for ProjectApp {
    fn render(&self) -> impl IntoElement {
        rect()
            .expanded()
            .center()
            .child("strata-freya — project window")
    }
}
