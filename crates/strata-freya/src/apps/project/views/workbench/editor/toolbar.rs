use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{QueryMode, RunId};
use crate::apps::project::state::{use_settle, Chan, SessionState};
use crate::apps::project::views::workbench::editor::actions;
use crate::components::divider::Divider;
use crate::components::icon::IconName;
use crate::components::metrics::TOOL_SIZE;
use crate::components::run_button::{RunButton, RunState};
use crate::components::toolbar::{Toolbar, ToolbarAction};
use crate::theme::{use_roles, Role};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_core::config::Command;
use strata_model::TabId;

/// The editor query toolbar, built to the comp. The bar itself only needs the editor surface (its
/// background) and the divider colour. The Run control is its own three-state `RunButton`; the rest
/// are outline [`Button`]s wrapping an icon (the rationalised button model — no bespoke `IconButton`).
///
/// Run / Explain / Analyze are wired (P2-15): a press snapshots the tab's editor text into a
/// fresh-nonce `QuerySpec` in the tab's own `request` slot (`QueryTab::request`, written on
/// `Chan::Request`) — the results pane's `use_query` picks it up (state-arch §6). While that
/// press is in flight (the `running` mirror holds its nonce) Run wears its Cancel dress —
/// pressing it aborts engine-side and drops the trigger, the same action as the Running
/// body's control. A blank buffer disables Run.
///
/// The editing actions are wired to [`actions`] (P2-16): Format / Clear rewrite the buffer
/// (history-tracked); Eye saves the buffer as a new `saved_view_N` catalog view; Save is the
/// dispatch-on-origin (view → `CREATE OR REPLACE VIEW`, saved query → upsert by id,
/// scratch → new saved query under the tab's name).
#[derive(PartialEq)]
pub struct EditorToolbar {
    pub id: TabId,
    /// The in-flight press's nonce, mirrored from the results body's query lifecycle (see
    /// `ResultsBody` — the toolbar doesn't subscribe the query itself, because `.enable(false)`
    /// is a *different* cache entry and so there is no watch-without-running subscription).
    pub running: State<Option<RunId>>,
}

impl Component for EditorToolbar {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let roles = use_roles();
        let (bg, border) = (roles.get(Role::Background), roles.get(Role::Border));
        let radio = use_radio::<SessionState, Chan>(Chan::Tab(id));
        let engine = use_consume::<EngineCtx>();
        let settle = use_settle();
        let request_radio = use_radio::<SessionState, Chan>(Chan::Request(id));

        let in_flight = request_radio
            .read()
            .request(id)
            .filter(|s| *self.running.read() == Some(s.run))
            .map(|s| s.run);

        let blank = radio
            .read()
            .tabs
            .get(&id)
            .is_none_or(|t| t.editor.rope.chars().all(char::is_whitespace));

        let press = move |mode: QueryMode| actions::press_query(radio, id, mode);

        let run_state = if in_flight.is_some() {
            RunState::Running
        } else if blank {
            RunState::Disabled
        } else {
            RunState::Idle
        };

        let save_engine = engine.clone();
        let view_engine = engine.clone();

        let run_press = move |_| match in_flight {
            Some(run) => actions::cancel_run(&engine, radio, settle.report.log, id, run),
            None => press(QueryMode::Run),
        };

        let action = |icon: IconName, label: &'static str| ToolbarAction::new(icon, label);

        let row = Toolbar::new()
            .background(bg)
            .leading(RunButton::new(run_state).on_press(run_press), TOOL_SIZE)
            .item(
                action(IconName::Explain, "Explain plan")
                    .on_press(move |_| press(QueryMode::Explain { analyze: false })),
            )
            .item(
                action(IconName::Analyze, "Explain analyze")
                    .on_press(move |_| press(QueryMode::Explain { analyze: true })),
            )
            .separator()
            .item(
                action(IconName::Format, "Format SQL")
                    .on_press(move |_| actions::format(radio, id)),
            )
            .item(
                action(IconName::Trash, "Clear editor")
                    .on_press(move |_| actions::clear(radio, id)),
            )
            .separator()
            .item(action(IconName::Eye, "Save as view").on_press(move |_| {
                actions::save_as_view(radio, view_engine.clone(), settle, id);
            }))
            .item(
                action(IconName::Save, "Save query")
                    .hint(Command::SaveQuery)
                    .on_press(move |_| {
                        actions::save(radio, save_engine.clone(), settle, id);
                    }),
            );

        rect()
            .width(Size::fill())
            .vertical()
            .child(row)
            .child(Divider::horizontal().color(border))
    }
}
