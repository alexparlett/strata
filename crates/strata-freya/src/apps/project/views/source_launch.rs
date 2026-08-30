//! **Opening the data source editor** (W7 · 03) — the slot a trigger sets, and the one place that
//! acts on it.
//!
//! [`configure_launch`](super::configure_launch)'s shape exactly, and for its reason. A
//! data source row's ⋮ menu is built inside an event handler, where no hook may run, so every
//! handle it will need has to be resolved at the *row's* render — and opening a window needs the
//! window's own handles (the app-globals, the engine, this window's log), none of which a sidebar
//! row has any business holding. Threading them onto every row so one menu item can use them is
//! how a leaf ends up depending on half the app.
//!
//! So a trigger sets [`Data sourceTarget`] into a slot and does nothing else, and this component —
//! mounted once at the project root, where those handles actually live — watches the slot and
//! opens the window. All three triggers (the pane header's `+`, its empty-state CTA, and a row's
//! *Edit data source*) set the slot; there is deliberately no second open path.

use freya::prelude::*;
use freya::radio::use_radio_station;

use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{use_catalog, use_catalog_rescan, ProjChan, ProjectState};
use crate::apps::project::use_report;
use crate::apps::source::{SourceLaunch, SourceTarget};
use crate::platform::{open_source, Subtree};
use crate::state::AppCtx;

/// The slot a trigger sets to ask for the data source editor. Provided at the project root.
pub type SourceRequest = State<Option<SourceTarget>>;

/// Watch the slot and open the window. Mount once, at the project root.
#[derive(PartialEq)]
pub struct SourceLauncher;

impl Component for SourceLauncher {
    fn render(&self) -> impl IntoElement {
        let mut slot = use_consume::<SourceRequest>();
        let app = use_consume::<AppCtx>();
        let engine = use_consume::<EngineCtx>();
        let report = use_report();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let rescan = use_catalog_rescan();
        let catalog = use_catalog();
        let subtree = use_consume::<Subtree>();
        let platform = use_hook(Platform::get);

        use_side_effect(move || {
            let Some(target) = slot.read().clone() else {
                return;
            };
            slot.set(None);
            open_source(
                platform.clone(),
                SourceLaunch {
                    target,
                    app: app.clone(),
                    project,
                    subtree: subtree.clone(),
                    rescan,
                    catalog,
                    engine: engine.clone(),
                    report,
                },
            );
        });

        rect()
    }
}
