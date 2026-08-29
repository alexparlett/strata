//! **Opening the Configure window** (P4-11) — the slot a trigger sets, and the one place that
//! acts on it.
//!
//! The same shape as the drop confirm and the profile confirm, for the same reason. A catalog
//! row's ⋮ menu is built inside an event handler, where no hook may run, so every handle it will
//! need has to be resolved at the *row's* render — and opening a window needs the window's own
//! handles (the app-globals, the engine, this window's log), none of which a row has any other
//! business holding. Threading them onto every row so that one menu item can use them is how a
//! leaf ends up depending on half the app.
//!
//! So a trigger sets [`ConfigureTarget`] into a slot and does nothing else, and this component —
//! mounted once at the project root, where those handles actually live — watches the slot and
//! opens the window. Adding a trigger means setting the slot; there is deliberately no second
//! open path.

use freya::prelude::*;
use freya::radio::use_radio_station;

use crate::apps::configure::{ConfigureLaunch, ConfigureTarget};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{use_catalog, use_catalog_rescan, ProjChan, ProjectState};
use crate::apps::project::use_report;
use crate::apps::project::views::SourceRequest;
use crate::platform::{open_configure, Subtree};
use crate::state::AppCtx;

/// The slot a trigger sets to ask for the Configure window. Provided at the project root.
pub type ConfigureRequest = State<Option<ConfigureTarget>>;

/// Watch the slot and open the window. Mount once, at the project root.
#[derive(PartialEq)]
pub struct ConfigureLauncher;

impl Component for ConfigureLauncher {
    fn render(&self) -> impl IntoElement {
        let mut slot = use_consume::<ConfigureRequest>();
        let app = use_consume::<AppCtx>();
        let engine = use_consume::<EngineCtx>();
        let report = use_report();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let rescan = use_catalog_rescan();
        let catalog = use_catalog();
        let subtree = use_consume::<Subtree>();
        let editor = use_consume::<SourceRequest>();
        let platform = use_hook(Platform::get);

        use_side_effect(move || {
            let Some(target) = slot.read().clone() else {
                return;
            };
            slot.set(None);
            open_configure(
                platform.clone(),
                ConfigureLaunch {
                    target,
                    app: app.clone(),
                    project,
                    subtree: subtree.clone(),
                    rescan,
                    catalog,
                    engine: engine.clone(),
                    report,
                    editor,
                },
            );
        });

        rect()
    }
}
