//! The workbench: the active tab's editor pane — the query toolbar over the `CodeEditor`.
//!
//! The editor is the Valin pattern: a `Writable` slice into the active `QueryTab`'s
//! `CodeEditorData`, which lives in the store keyed by `TabId`, so switching tabs re-binds and
//! each tab's cursor / undo / scroll travel with it.
//!
//! The toolbar is built to the `Editor.dc.html` comp from reusable `IconButton`s (accent Run +
//! outlined Explain · Analyze │ Format · Clear │ Save-as-view · Save). Run / Explain / Analyze
//! drive freya-query through the tab's own `request` slot (`QueryTab::request`, on
//! `Chan::Request(id)`; Run flips to Cancel mid-run via the `running` mirror); Format / Clear /
//! Save-as-view / Save go through `editor::actions` — buffer rewrites plus the
//! dispatch-on-origin save into the Project store (⌘S lands with the keymap).

use crate::apps::project::close::{CloseTarget, TabCloser};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::RunId;
use crate::apps::project::state::{
    use_catalog, use_report, Chan, ProjChan, ProjectState, SessionState,
};
use crate::keymap::on_commands;
use editor::actions;
use editor::tab::EditorTab;
use empty::EmptyState;
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station};
use results::Results;
use strata_core::config::Command;

use crate::state::use_config_station;
use tab_bar::bar::TabBar;

pub mod editor;
mod empty;
mod results;
mod tab_bar;

/// The tab strip's fixed height (`tab_bar::bar`).
const TAB_BAR_H: f32 = 38.;
/// The shortest the editor pane may become: its toolbar, the rule under it, and one line of SQL.
/// Like every floor in the shell this is a stub, not a usable size — see `views::shell`.
const EDITOR_STUB_H: f32 = 60.;
/// The canvas's editor-pane clamp (`Strata.dc.html` `onResizeEditor`).
const EDITOR_MAX_H: f32 = 480.;
/// The shortest the results pane may become: its toolbar over its status bar, with no body.
const RESULTS_STUB_H: f32 = 78.;

/// The shortest the whole workbench may become, which is what the drawer stops taking from.
/// Derived rather than typed again, so adding a bar to either pane moves it.
pub const WORKBENCH_STUB_H: f32 = TAB_BAR_H + EDITOR_STUB_H + 1. + RESULTS_STUB_H;

pub use results::{
    CancelButtonThemePartial, CancelButtonThemePreference, CellViewThemePreference,
    ChartThemePreference, DataGridThemePreference, ExplainPlanThemePreference,
    RecordViewThemePreference, ShapeDialog, ShapeTarget, StatusBarThemePreference,
};
pub use tab_bar::bar::TabBarThemePreference;
pub use tab_bar::tab::TabThemePreference;

/// The central editing area: renders the active tab's editor pane, or an empty state when no tab
/// is open. Subscribes to `Chan::Tabs` for the active id only — the editor drives its own
/// per-`Chan::Tab(id)` reactivity.
#[derive(PartialEq)]
pub struct Workbench;

impl Component for Workbench {
    fn render(&self) -> impl IntoElement {
        let radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let active = radio.read().active;
        let engine = use_consume::<EngineCtx>();

        let running = use_state(|| None::<RunId>);

        let confirm = use_consume::<State<Option<CloseTarget>>>();
        let closer_engine = use_state({
            let engine = engine.clone();
            move || engine
        });
        let closer = use_provide_context(move || TabCloser {
            engine: closer_engine,
            confirm,
        });

        let config = use_config_station();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let catalog = use_catalog();
        let report = use_report();
        let mut cmd_radio = radio;
        let shortcuts = on_commands(config, move |cmd| {
            let active = cmd_radio.read().active;
            match cmd {
                Command::NewTab => {
                    cmd_radio.write().open_blank();
                    true
                }
                Command::ReopenTab => {
                    cmd_radio.write().reopen_last();
                    true
                }
                Command::CloseActiveTab => {
                    let Some(id) = active else { return false };
                    closer.close(cmd_radio, config, id);
                    true
                }
                Command::RunQuery => {
                    let Some(id) = active else { return false };
                    actions::run_query(&engine, cmd_radio, id);
                    true
                }
                Command::SaveQuery => {
                    let Some(id) = active else { return false };
                    actions::save(cmd_radio, project, engine.clone(), catalog, report, id);
                    true
                }
                _ => false,
            }
        });

        rect()
            .expanded()
            .on_global_key_down(shortcuts)
            .child(TabBar::new())
            .map(active, |el, id| {
                el.child(
                    ResizableContainer::new()
                        .direction(Direction::Vertical)
                        .handle_size(1.)
                        .panel(
                            ResizablePanel::new(PanelSize::px(240.))
                                .min_size(EDITOR_STUB_H)
                                .max_size(EDITOR_MAX_H)
                                .child(EditorTab::new(id, running)),
                        )
                        .panel(
                            ResizablePanel::new(PanelSize::percent(100.))
                                .min_pixels(RESULTS_STUB_H)
                                .min_size(0.)
                                .child(Results::new(id, running)),
                        ),
                )
            })
            .maybe(active.is_none(), |el| el.child(EmptyState::new()))
    }
}
