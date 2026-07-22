//! The workbench: the active tab's editor pane — the query toolbar over the `CodeEditor`.
//!
//! The editor is the Valin pattern: a `Writable` slice into the active [`QueryTab`]'s
//! `CodeEditorData`, which lives in the store keyed by `TabId`, so switching tabs re-binds and
//! each tab's cursor / undo / scroll travel with it.
//!
//! The toolbar is built to the `Editor.dc.html` comp from reusable `IconButton`s (accent Run +
//! outlined Explain · Analyze │ Format · Clear │ Save-as-view · Save). Actions are stubbed until
//! their layer lands (Run/Explain → freya-query, Save → the Project store, Format/Clear → editor
//! commands), and the running / dirty / validation states that gate them come with those.

use crate::apps::project::query::QuerySpec;
use crate::apps::project::state::{Chan, SessionState};
use editor::tab::EditorTab;
use empty::EmptyState;
use freya::prelude::*;
use freya::radio::use_radio;
use results::Results;
use tab_bar::bar::TabBar;

mod empty;
mod results;
mod tab_bar;
pub mod editor;

pub use results::{CancelButtonThemePreference, DataGridThemePreference, StatusBarThemePreference};
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

        rect()
            .expanded()
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
                                .child(EditorTab::new(id, request)),
                        )
                        .panel(
                            ResizablePanel::new(PanelSize::percent(100.))
                                .child(Results::new(id, request)),
                        ),
                )
            })
            // Empty state: a filling body under the pinned 38px `TabBar`. (Centring the *root* would
            // float the whole strip into the middle, since with no editor pane there's no space-filling
            // sibling to hold it up.)
            .maybe(active.is_none(), |el| el.child(EmptyState::new()))
    }
}
