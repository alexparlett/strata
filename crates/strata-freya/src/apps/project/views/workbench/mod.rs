//! The workbench: the active tab's editor pane — the query toolbar over the `CodeEditor`.
//!
//! The editor is the Valin pattern: a `Writable` slice into the active [`QueryTab`]'s
//! `CodeEditorData`, which lives in the store keyed by `TabId`, so switching tabs re-binds and
//! each tab's cursor / undo / scroll travel with it.
//!
//! The toolbar is built to the `Editor.dc.html` comp from reusable `IconButton`s (accent Run +
//! outlined Explain · Analyze │ Format · Clear │ Save-as-view · Save). Run / Explain / Analyze
//! drive freya-query through the `request` slot (Run flips to Cancel mid-run via the `running`
//! mirror); Format / Clear / Save-as-view / Save go through `editor::actions` — buffer
//! rewrites plus the dispatch-on-origin save into the Project store (⌘S lands with the keymap).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::apps::project::close::{CloseGuard, CloseTarget, TabCloser};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryMode, QuerySpec, RunId};
use crate::apps::project::state::{Chan, ProjChan, ProjectState, SessionState};
use editor::actions;
use editor::tab::EditorTab;
use empty::EmptyState;
use freya::prelude::*;
use freya::radio::{use_radio, use_radio_station};
use results::Results;
use strata_core::config::{Command, Settings};
use tab_bar::bar::TabBar;

mod empty;
mod results;
mod tab_bar;
pub mod editor;

pub use results::{
    CancelButtonThemePartial, CancelButtonThemePreference, DataGridThemePreference,
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

        // The window's Run trigger (state-arch §6): the latest Run press, component-local —
        // written by the editor toolbar, read by the results pane, threaded as plain props.
        // Editing never touches it; only a press rebuilds it (fresh nonce → new execution).
        let mut request = use_state(|| None::<QuerySpec>);

        // The in-flight press's nonce, mirrored out of the results body's query lifecycle
        // (see `ResultsBody`) so the toolbar can wear Run→Cancel without subscribing the
        // query itself — freya-query re-runs stale entries on subscribe, and an in-flight
        // entry reads as stale, so a second enabled subscriber would double-execute the run.
        let running = use_state(|| None::<RunId>);

        // A press outlives its tab only until the close funnel runs: if the pressed tab is
        // gone (close / close-others / …), drop the slot so a reopened tab starts with no
        // results, like a fresh one — matching the engine-side cleanup (SNAPSHOT_SPEC §4).
        use_side_effect(move || {
            let gone = request
                .peek()
                .as_ref()
                .is_some_and(|spec| !radio.read().tabs.contains_key(&spec.tab));
            if gone {
                request.set(None);
            }
        });

        // Mirror "a query is in flight" into the close guard's atomic, where the winit
        // `on_close` hook (T2) reads it synchronously — the hook runs outside any
        // component scope and can't touch reactive state.
        let close_guard = use_consume::<Arc<CloseGuard>>();
        {
            let close_guard = close_guard.clone();
            use_side_effect(move || {
                let in_flight = request
                    .read()
                    .as_ref()
                    .is_some_and(|s| *running.read() == Some(s.run));
                close_guard.running.store(in_flight, Ordering::Relaxed);
            });
        }
        let confirm = use_consume::<State<Option<CloseTarget>>>();
        // The single-tab close gate, shared by every close path (⌘W here; the tab's ×,
        // the tab context menu, and the nav dropdown consume it from context).
        let closer = use_provide_context(move || TabCloser {
            request,
            running,
            confirm,
        });

        // The workbench-owned shortcuts (one keyboard handler per node — see
        // `keymap::on_commands`). Tab commands write the session store; ⌘↵ and ⌘S share
        // the toolbar buttons' `actions`. Handlers peek and derive the active id at call
        // time — never a mount-time snapshot. This rect is an ancestor of the whole
        // workbench, so these fire before any Esc consumer below (fine: no Esc here, and
        // each of these chords has a single consumer).
        let settings = use_consume::<State<Settings>>();
        let engine = use_consume::<EngineCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let mut cmd_radio = radio;
        let shortcuts = crate::keymap::on_commands(settings, move |cmd| {
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
                    closer.close(cmd_radio, settings, id);
                    true
                }
                Command::RunQuery => {
                    let Some(id) = active else { return false };
                    // In flight → consume but do nothing: Esc is the cancel, and a
                    // second press must not double-run.
                    let in_flight = request
                        .peek()
                        .as_ref()
                        .is_some_and(|s| s.tab == id && *running.peek() == Some(s.run));
                    if !in_flight {
                        actions::press_query(cmd_radio, id, request, QueryMode::Run);
                    }
                    true
                }
                Command::SaveQuery => {
                    let Some(id) = active else { return false };
                    actions::save(cmd_radio, project, engine.clone(), id);
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
                                .child(EditorTab::new(id, request, running)),
                        )
                        .panel(
                            ResizablePanel::new(PanelSize::percent(100.))
                                .child(Results::new(id, request, running)),
                        ),
                )
            })
            // Empty state: a filling body under the pinned 38px `TabBar`. (Centring the *root* would
            // float the whole strip into the middle, since with no editor pane there's no space-filling
            // sibling to hold it up.)
            .maybe(active.is_none(), |el| el.child(EmptyState::new()))
    }
}
