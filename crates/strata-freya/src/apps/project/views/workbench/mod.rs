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
use crate::apps::project::query::{QueryMode, RunId};
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

pub use results::{
    CancelButtonThemePartial, CancelButtonThemePreference, CellViewThemePreference,
    DataGridThemePreference, ExplainPlanThemePreference, RecordViewThemePreference,
    StatusBarThemePreference,
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

        // The in-flight press's nonce, mirrored out of the results body's query lifecycle
        // (see `ResultsBody`) so the toolbar can wear Run→Cancel. One resolver, one slot,
        // read by props — the toolbar and ⌘↵ below both ask the same question and neither
        // has to know what a query is.
        //
        // It used to be a *correctness* requirement: freya-query re-ran an entry a mounting
        // subscriber found stale, and an in-flight entry read as stale, so a second enabled
        // subscriber double-executed the run. Our fork fixed that at the source — an execution
        // in flight is counted, and a mounting subscriber attaches to it (freya-query's
        // `RunningGuard` / `query_inflight_dedup.rs`). So the toolbar *could* subscribe now; it
        // still doesn't, because `.enable(false)` is part of `Query`'s cache identity (a
        // disabled subscriber reads a different, never-running entry, so there is no way to
        // "watch without running"), and because the toolbar would then re-render on every step
        // of a lifecycle it only wants one bit of.
        // (The Run trigger itself lives on each tab — `QueryTab::request`, state-arch §6.)
        //
        // It answers for the **active tab only**, which is all the toolbar and ⌘↵ need
        // since both only ever address that tab. The close-while-running guard (T2) does
        // *not* use it — that question spans every tab, so it goes to the engine (see
        // `close::CloseGuard` / `TabCloser`).
        let running = use_state(|| None::<RunId>);

        let confirm = use_consume::<State<Option<CloseTarget>>>();
        // The engine as a `Copy` slot for `TabCloser`, which is passed by value into the
        // tab strip's closures.
        let closer_engine = use_state({
            let engine = engine.clone();
            move || engine
        });
        // The single-tab close gate, shared by every close path (⌘W here; the tab's ×,
        // the tab context menu, and the nav dropdown consume it from context).
        let closer = use_provide_context(move || TabCloser {
            engine: closer_engine,
            confirm,
        });

        // The workbench-owned shortcuts (one keyboard handler per node — see
        // `keymap::on_commands`). Tab commands write the session store; ⌘↵ and ⌘S share
        // the toolbar buttons' `actions`. Handlers peek and derive the active id at call
        // time — never a mount-time snapshot. This rect is an ancestor of the whole
        // workbench, so these fire before any Esc consumer below (fine: no Esc here, and
        // each of these chords has a single consumer).
        let config = use_config_station();
        let project = use_radio_station::<ProjectState, ProjChan>();
        // ⌘S can create a view, which moves the engine's catalog — the driver re-derives every
        // tab's verdict against it (`state::diagnostics`).
        let catalog = use_catalog();
        // …and records its outcome in the event log (P3-13), like the toolbar's Save button.
        let report = use_report();
        let mut cmd_radio = radio;
        let shortcuts = on_commands(config, move |cmd| {
            // `read()` is peek-equivalent here: event handlers have no reactive context.
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
                    // Through the shared gate: the T2 confirm when the tab's query is
                    // in flight (and the pref is on) — same dialog as the window close.
                    closer.close(cmd_radio, config, id);
                    true
                }
                Command::RunQuery => {
                    let Some(id) = active else { return false };
                    // In flight → consume but do nothing: Esc is the cancel, and a
                    // second press must not double-run.
                    let in_flight = cmd_radio
                        .read()
                        .request(id)
                        .is_some_and(|s| *running.peek() == Some(s.run));
                    if !in_flight {
                        actions::press_query(cmd_radio, id, QueryMode::Run);
                    }
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
                        // Match the app's 1px rules (the handle's colour comes from the
                        // `resizable_handle` theme; bump this if it reads too thin to grab).
                        .handle_size(1.)
                        .panel(
                            ResizablePanel::new(PanelSize::px(240.))
                                .min_size(92.)
                                .child(EditorTab::new(id, running)),
                        )
                        .panel(
                            ResizablePanel::new(PanelSize::percent(100.))
                                .child(Results::new(id, running)),
                        ),
                )
            })
            // Empty state: a filling body under the pinned 38px `TabBar`. (Centring the *root* would
            // float the whole strip into the middle, since with no editor pane there's no space-filling
            // sibling to hold it up.)
            .maybe(active.is_none(), |el| el.child(EmptyState::new()))
    }
}
